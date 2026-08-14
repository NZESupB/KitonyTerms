//! SFTP 子任务 —— 在独立 tokio 任务中驱动 russh-sftp 的 [`SftpSession`]。
//!
//! SFTP subtask: drives a russh-sftp [`SftpSession`] in its own tokio task so
//! that long transfers never block the interactive shell loop. It consumes
//! [`SftpRequest`]s and emits [`FromCore`] events (listings, progress, done,
//! errors). A single failed operation reports an error but keeps the task alive.

use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::session::{FromCore, SessionId, SftpEntry, SftpOp, SftpRequest, SftpRequestId};
use crate::ssh::SshConnectionGuard;

/// 传输分块大小;同时作为进度上报的步长基准。
/// Transfer chunk size; also the basis for progress-reporting cadence.
const CHUNK: usize = 32 * 1024;
/// 进度上报的最小字节间隔,避免刷屏。
/// Minimum byte interval between progress events to avoid flooding.
const PROGRESS_STEP: u64 = 256 * 1024;
/// 单次快速 SFTP 操作超时,避免 UI 无限 loading。
/// Timeout for quick SFTP operations so the UI never spins forever.
const QUICK_OP_TIMEOUT: Duration = Duration::from_secs(12);
const LOCAL_TEMP_CREATE_ATTEMPTS: usize = 16;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// SFTP 子任务主循环。`rx` 关闭(会话结束)时退出,`session` 随之 drop 关闭通道。
/// Main loop. Exits when `rx` closes (session ended); dropping `session` closes
/// the channel.
pub async fn sftp_task(
    id: SessionId,
    session: SftpSession,
    _connection_guard: Option<SshConnectionGuard>,
    mut rx: mpsc::UnboundedReceiver<(SftpRequestId, SftpRequest)>,
    out: mpsc::Sender<FromCore>,
) {
    while let Some((request_id, req)) = rx.recv().await {
        if let Err(message) = handle(&session, id, request_id, &req, &out).await {
            let _ = out
                .send(FromCore::SftpError {
                    id,
                    request_id,
                    message,
                })
                .await;
        }
    }
    let _ = session.close().await;
    let _ = out.send(FromCore::SftpStopped { id }).await;
}

/// 处理单个请求。返回 `Err(message)` 时由调用方上报 [`FromCore::SftpError`]。
/// Handle one request; `Err(message)` is surfaced as [`FromCore::SftpError`].
async fn handle(
    session: &SftpSession,
    id: SessionId,
    request_id: SftpRequestId,
    req: &SftpRequest,
    out: &mpsc::Sender<FromCore>,
) -> Result<(), String> {
    match req {
        SftpRequest::List { path } => {
            // 规范化为绝对路径,便于 UI 做上级/进入目录的路径拼接。
            // Canonicalize to an absolute path so the UI can join/parent cleanly.
            let abs =
                match tokio::time::timeout(QUICK_OP_TIMEOUT, session.canonicalize(path.clone()))
                    .await
                {
                    Ok(Ok(abs)) => abs,
                    Ok(Err(e)) => {
                        tracing::debug!(
                            "SFTP canonicalize {path} failed, fallback to original path: {e}"
                        );
                        path.clone()
                    }
                    Err(_) => {
                        tracing::debug!(
                            "SFTP canonicalize {path} timed out, fallback to original path"
                        );
                        path.clone()
                    }
                };
            let read_dir =
                match tokio::time::timeout(QUICK_OP_TIMEOUT, session.read_dir(abs.clone())).await {
                    Ok(Ok(read_dir)) => read_dir,
                    Ok(Err(e)) => {
                        return Err(format!("读取目录 {abs} 失败：{e}"));
                    }
                    Err(_) => return Err(timeout_message("读取目录", &abs)),
                };
            let mut entries: Vec<SftpEntry> = read_dir
                .map(|e| {
                    let meta = e.metadata();
                    SftpEntry {
                        name: e.file_name(),
                        is_dir: meta.is_dir(),
                        size: meta.size.unwrap_or(0),
                        modified: meta.mtime,
                        permissions: meta.permissions,
                        user: meta.user,
                        group: meta.group,
                        uid: meta.uid,
                        gid: meta.gid,
                    }
                })
                .collect();
            // 目录在前,随后按名称不区分大小写排序。
            // Directories first, then case-insensitive by name.
            entries.sort_by(|a, b| {
                b.is_dir
                    .cmp(&a.is_dir)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            let _ = out
                .send(FromCore::SftpListing {
                    id,
                    request_id,
                    path: abs,
                    entries,
                })
                .await;
            Ok(())
        }

        SftpRequest::Download { remote, local } => {
            let name = basename(remote);
            let mut src = session
                .open(remote.clone())
                .await
                .map_err(|e| e.to_string())?;
            let total = src.metadata().await.ok().and_then(|m| m.size).unwrap_or(0);
            download_to_local(&mut src, local, id, request_id, &name, total, out).await?;
            let _ = out
                .send(FromCore::SftpDone {
                    id,
                    request_id,
                    op: SftpOp::Download,
                    path: remote.clone(),
                })
                .await;
            Ok(())
        }

        SftpRequest::Upload { local, remote } => {
            let name = basename(remote);
            let mut src = tokio::fs::File::open(local)
                .await
                .map_err(|e| e.to_string())?;
            let total = src.metadata().await.map(|m| m.len()).unwrap_or(0);
            // 先探测目标：目录直接拒绝；已存在的文件记录权限，提交后保持原权限不变。
            let existing = session.metadata(remote.clone()).await.ok();
            if existing.as_ref().is_some_and(|meta| meta.is_dir()) {
                return Err(format!("上传目标 {remote} 是目录，无法覆盖"));
            }
            let existing_permissions = existing.as_ref().and_then(|meta| meta.permissions);
            let target_exists = existing.is_some();

            let temp_path = remote_temp_path(remote, &unique_temp_suffix());
            let mut dst = session
                .open_with_flags(
                    temp_path.clone(),
                    OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
                )
                .await
                .map_err(|e| format!("创建远端临时文件 {temp_path} 失败：{e}"))?;
            let transfer_result = async {
                copy_with_progress(&mut src, &mut dst, id, request_id, &name, total, out).await?;
                // 先关闭远端临时文件，确保服务器提交写入后再执行 rename。
                dst.shutdown().await.map_err(|e| e.to_string())
            }
            .await;
            drop(dst);

            if let Err(error) = transfer_result {
                return Err(cleanup_remote_temp(
                    session,
                    &temp_path,
                    format!("上传 {remote} 失败：{error}"),
                )
                .await);
            }

            // 覆盖上传保持原文件权限（含可执行位）；失败只告警，不影响内容提交。
            if let Some(permissions) = existing_permissions {
                let mut attributes = FileAttributes::empty();
                attributes.permissions = Some(permissions);
                if let Err(error) = session.set_metadata(temp_path.clone(), attributes).await {
                    tracing::warn!("保持 {remote} 原有权限失败: {error}");
                }
            }

            if let Err(error) =
                commit_uploaded_temp(session, &temp_path, remote, target_exists).await
            {
                return Err(cleanup_remote_temp(session, &temp_path, error).await);
            }
            let _ = out
                .send(FromCore::SftpDone {
                    id,
                    request_id,
                    op: SftpOp::Upload,
                    path: remote.clone(),
                })
                .await;
            Ok(())
        }

        SftpRequest::Mkdir { path } => {
            session
                .create_dir(path.clone())
                .await
                .map_err(|e| e.to_string())?;
            let _ = out
                .send(FromCore::SftpDone {
                    id,
                    request_id,
                    op: SftpOp::Mkdir,
                    path: path.clone(),
                })
                .await;
            Ok(())
        }

        SftpRequest::Remove { path, is_dir } => {
            if *is_dir {
                session
                    .remove_dir(path.clone())
                    .await
                    .map_err(|e| e.to_string())?;
            } else {
                session
                    .remove_file(path.clone())
                    .await
                    .map_err(|e| e.to_string())?;
            }
            let _ = out
                .send(FromCore::SftpDone {
                    id,
                    request_id,
                    op: SftpOp::Remove,
                    path: path.clone(),
                })
                .await;
            Ok(())
        }

        SftpRequest::Rename { from, to } => {
            session
                .rename(from.clone(), to.clone())
                .await
                .map_err(|e| e.to_string())?;
            let _ = out
                .send(FromCore::SftpDone {
                    id,
                    request_id,
                    op: SftpOp::Rename,
                    path: to.clone(),
                })
                .await;
            Ok(())
        }
    }
}

/// 提交上传所需的远端最小文件操作，便于按不同服务器的 rename 语义做单元测试。
/// Minimal remote file operations used to commit an upload; keeps the commit
/// strategy testable against different server rename semantics.
trait RemoteCommitOps {
    fn move_file(&self, from: &str, to: &str) -> impl Future<Output = Result<(), String>> + Send;
    fn delete_file(&self, path: &str) -> impl Future<Output = Result<(), String>> + Send;
}

impl RemoteCommitOps for SftpSession {
    async fn move_file(&self, from: &str, to: &str) -> Result<(), String> {
        self.rename(from.to_string(), to.to_string())
            .await
            .map_err(|error| error.to_string())
    }

    async fn delete_file(&self, path: &str) -> Result<(), String> {
        self.remove_file(path.to_string())
            .await
            .map_err(|error| error.to_string())
    }
}

/// 把已写完的远端临时文件提交到正式路径。
///
/// 1. 先直接 rename：新建文件、以及支持 POSIX 覆盖语义的服务器一次即成功。
/// 2. OpenSSH 等服务器的 `SSH_FXP_RENAME` 不允许覆盖已存在文件，此时先把原文件
///    改名为同目录备份再提交临时文件；提交失败立即把备份改回原名，原文件不丢失。
///
/// Commits a fully written remote temp file onto its final path: plain rename
/// first, then (for servers like OpenSSH whose rename refuses to clobber) move
/// the original aside, commit, and roll the original back if the commit fails.
async fn commit_uploaded_temp<O: RemoteCommitOps>(
    ops: &O,
    temp_path: &str,
    remote: &str,
    target_exists: bool,
) -> Result<(), String> {
    let rename_error = match ops.move_file(temp_path, remote).await {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    if !target_exists {
        return Err(format!("提交上传 {remote} 失败：{rename_error}"));
    }

    let backup_path = remote_backup_path(remote, &unique_temp_suffix());
    if let Err(error) = ops.move_file(remote, &backup_path).await {
        return Err(format!(
            "提交上传 {remote} 失败，原文件保持不变：{rename_error}；备份原文件失败：{error}"
        ));
    }
    if let Err(error) = ops.move_file(temp_path, remote).await {
        return match ops.move_file(&backup_path, remote).await {
            Ok(()) => Err(format!("提交上传 {remote} 失败，原文件已恢复：{error}")),
            Err(restore_error) => Err(format!(
                "提交上传 {remote} 失败，原文件仍保存在 {backup_path}，恢复原名失败：{error}；{restore_error}"
            )),
        };
    }
    if let Err(error) = ops.delete_file(&backup_path).await {
        tracing::warn!("上传 {remote} 已提交，但清理备份 {backup_path} 失败: {error}");
    }
    Ok(())
}

async fn download_to_local<R>(
    src: &mut R,
    target: &Path,
    id: SessionId,
    request_id: SftpRequestId,
    name: &str,
    total: u64,
    out: &mpsc::Sender<FromCore>,
) -> Result<(), String>
where
    R: AsyncReadExt + Unpin,
{
    let (temp_path, mut dst) = create_private_download_temp(target)
        .await
        .map_err(|error| format!("创建本地临时文件失败：{error}"))?;
    let transfer_result = async {
        copy_with_progress(src, &mut dst, id, request_id, name, total, out).await?;
        dst.flush().await.map_err(|error| error.to_string())?;
        dst.sync_all().await.map_err(|error| error.to_string())
    }
    .await;
    drop(dst);

    if let Err(error) = transfer_result {
        return Err(cleanup_local_temp(
            &temp_path,
            format!("下载到 {} 失败：{error}", target.display()),
        )
        .await);
    }

    if let Err(error) = tokio::fs::rename(&temp_path, target).await {
        return Err(cleanup_local_temp(
            &temp_path,
            format!(
                "安全提交下载文件 {} 失败，原文件保持不变：{error}",
                target.display()
            ),
        )
        .await);
    }
    Ok(())
}

async fn create_private_download_temp(
    target: &Path,
) -> std::io::Result<(PathBuf, tokio::fs::File)> {
    let target = target.to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        for _ in 0..LOCAL_TEMP_CREATE_ATTEMPTS {
            let temp_path = local_temp_path(&target, &unique_temp_suffix());
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&temp_path) {
                Ok(file) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if let Err(error) = std::fs::set_permissions(
                            &temp_path,
                            std::fs::Permissions::from_mode(0o600),
                        ) {
                            let _ = std::fs::remove_file(&temp_path);
                            return Err(error);
                        }
                    }
                    return Ok((temp_path, file));
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "无法分配唯一的下载临时文件名",
        ))
    })
    .await
    .map_err(std::io::Error::other)??;

    Ok((result.0, tokio::fs::File::from_std(result.1)))
}

fn local_temp_path(target: &Path, suffix: &str) -> PathBuf {
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("download");
    target.with_file_name(format!(".{name}.kitonyterms-download-{suffix}.tmp"))
}

fn remote_temp_path(target: &str, suffix: &str) -> String {
    remote_sibling_path(target, "upload", suffix)
}

fn remote_backup_path(target: &str, suffix: &str) -> String {
    remote_sibling_path(target, "backup", suffix)
}

/// 在目标同目录生成隐藏的辅助文件路径，保证 rename 不跨文件系统。
fn remote_sibling_path(target: &str, kind: &str, suffix: &str) -> String {
    let (directory, name) = match target.rsplit_once('/') {
        Some((directory, name)) => (Some(directory), name),
        None => (None, target),
    };
    let sibling_name = format!(".{name}.kitonyterms-{kind}-{suffix}.tmp");
    match directory {
        Some("") => format!("/{sibling_name}"),
        Some(directory) => format!("{directory}/{sibling_name}"),
        None => sibling_name,
    }
}

fn unique_temp_suffix() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{timestamp}-{counter}", std::process::id())
}

async fn cleanup_local_temp(path: &Path, primary: String) -> String {
    match tokio::fs::remove_file(path).await {
        Ok(()) => primary,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => primary,
        Err(error) => {
            append_cleanup_error(primary, &path.display().to_string(), &error.to_string())
        }
    }
}

async fn cleanup_remote_temp(session: &SftpSession, path: &str, primary: String) -> String {
    match session.remove_file(path.to_string()).await {
        Ok(()) => primary,
        Err(error) => append_cleanup_error(primary, path, &error.to_string()),
    }
}

fn append_cleanup_error(primary: String, temp_path: &str, cleanup_error: &str) -> String {
    format!("{primary}；清理临时文件 {temp_path} 失败：{cleanup_error}")
}

/// 分块拷贝并周期上报进度。完成时补发一条 100% 进度。
/// Copy in chunks while emitting throttled progress; emit a final 100% tick.
async fn copy_with_progress<R, W>(
    src: &mut R,
    dst: &mut W,
    id: SessionId,
    request_id: SftpRequestId,
    name: &str,
    total: u64,
    out: &mpsc::Sender<FromCore>,
) -> Result<(), String>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut buf = vec![0u8; CHUNK];
    let mut transferred = 0u64;
    let mut last_emit = 0u64;
    loop {
        let n = src.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n]).await.map_err(|e| e.to_string())?;
        transferred += n as u64;
        if transferred - last_emit >= PROGRESS_STEP {
            last_emit = transferred;
            let _ = out
                .send(FromCore::SftpProgress {
                    id,
                    request_id,
                    name: name.to_string(),
                    transferred,
                    total,
                })
                .await;
        }
    }
    let _ = out
        .send(FromCore::SftpProgress {
            id,
            request_id,
            name: name.to_string(),
            transferred,
            total,
        })
        .await;
    Ok(())
}

/// 取远端 POSIX 路径的末段作为显示名。
/// Last `/`-separated segment of a remote POSIX path, for display.
fn basename(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn timeout_message(operation: &str, path: &str) -> String {
    format!(
        "{operation} {path} 超时({} 秒)，远端 SFTP 子系统可能无响应",
        QUICK_OP_TIMEOUT.as_secs()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncRead, ReadBuf};

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "kitonyterms-sftp-test-{}-{}-{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ))
    }

    #[test]
    fn timeout_message_includes_operation_path_and_limit() {
        let message = timeout_message("读取目录", "/root");
        assert!(message.contains("读取目录 /root 超时"));
        assert!(message.contains("12 秒"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_temp_file_is_private_and_in_target_directory() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_path("download");
        let (temp, mut file) = create_private_download_temp(&path).await.unwrap();
        file.write_all(b"secret").await.unwrap();
        drop(file);

        let mode = std::fs::metadata(&temp).unwrap().permissions().mode() & 0o777;
        assert_eq!(temp.parent(), path.parent());
        let _ = std::fs::remove_file(&temp);
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn temporary_paths_stay_next_to_their_targets() {
        assert_eq!(
            local_temp_path(Path::new("/tmp/report.txt"), "abc"),
            PathBuf::from("/tmp/.report.txt.kitonyterms-download-abc.tmp")
        );
        assert_eq!(
            remote_temp_path("/home/me/report.txt", "abc"),
            "/home/me/.report.txt.kitonyterms-upload-abc.tmp"
        );
        assert_eq!(
            remote_temp_path("report.txt", "abc"),
            ".report.txt.kitonyterms-upload-abc.tmp"
        );
        assert_eq!(
            remote_backup_path("/home/me/report.txt", "abc"),
            "/home/me/.report.txt.kitonyterms-backup-abc.tmp"
        );
    }

    #[test]
    fn cleanup_failure_keeps_primary_error_visible() {
        let message = append_cleanup_error(
            "上传失败".to_string(),
            "/tmp/upload.tmp",
            "permission denied",
        );
        assert!(message.starts_with("上传失败"));
        assert!(message.contains("/tmp/upload.tmp"));
        assert!(message.contains("permission denied"));
    }

    const COMMIT_TEMP: &str = "/home/me/.report.txt.kitonyterms-upload-1.tmp";
    const COMMIT_TARGET: &str = "/home/me/report.txt";

    /// 模拟远端文件系统。`overwrite_rename` 为 false 时复刻 OpenSSH `SSH_FXP_RENAME`
    /// 的语义：目标已存在则整条 rename 失败。
    struct FakeRemote {
        files: std::sync::Mutex<std::collections::HashMap<String, String>>,
        overwrite_rename: bool,
        /// 强制失败的 rename 调用序号（从 0 起），用于验证提交失败后的回滚。
        fail_nth_rename: Option<usize>,
        renames: std::sync::Mutex<Vec<(String, String)>>,
    }

    impl FakeRemote {
        fn new(files: &[(&str, &str)], overwrite_rename: bool) -> Self {
            Self {
                files: std::sync::Mutex::new(
                    files
                        .iter()
                        .map(|(path, content)| (path.to_string(), content.to_string()))
                        .collect(),
                ),
                overwrite_rename,
                fail_nth_rename: None,
                renames: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn failing_at_rename(mut self, index: usize) -> Self {
            self.fail_nth_rename = Some(index);
            self
        }

        fn content(&self, path: &str) -> Option<String> {
            self.files.lock().unwrap().get(path).cloned()
        }

        fn paths(&self) -> Vec<String> {
            let mut paths: Vec<String> = self.files.lock().unwrap().keys().cloned().collect();
            paths.sort();
            paths
        }

        fn rename_count(&self) -> usize {
            self.renames.lock().unwrap().len()
        }
    }

    impl RemoteCommitOps for FakeRemote {
        async fn move_file(&self, from: &str, to: &str) -> Result<(), String> {
            let mut files = self.files.lock().unwrap();
            let mut renames = self.renames.lock().unwrap();
            let index = renames.len();
            renames.push((from.to_string(), to.to_string()));
            if self.fail_nth_rename == Some(index) {
                return Err("模拟服务器拒绝提交".to_string());
            }
            let Some(content) = files.remove(from) else {
                return Err(format!("{from} 不存在"));
            };
            if !self.overwrite_rename && files.contains_key(to) {
                files.insert(from.to_string(), content);
                return Err("目标已存在".to_string());
            }
            files.insert(to.to_string(), content);
            Ok(())
        }

        async fn delete_file(&self, path: &str) -> Result<(), String> {
            match self.files.lock().unwrap().remove(path) {
                Some(_) => Ok(()),
                None => Err(format!("{path} 不存在")),
            }
        }
    }

    #[tokio::test]
    async fn upload_commit_overwrites_existing_file_when_rename_refuses_clobber() {
        let remote = FakeRemote::new(&[(COMMIT_TEMP, "new"), (COMMIT_TARGET, "original")], false);

        commit_uploaded_temp(&remote, COMMIT_TEMP, COMMIT_TARGET, true)
            .await
            .unwrap();

        assert_eq!(remote.content(COMMIT_TARGET).as_deref(), Some("new"));
        // 临时文件与备份都不残留。
        assert_eq!(remote.paths(), vec![COMMIT_TARGET.to_string()]);
    }

    #[tokio::test]
    async fn upload_commit_uses_single_rename_when_server_overwrites() {
        let remote = FakeRemote::new(&[(COMMIT_TEMP, "new"), (COMMIT_TARGET, "original")], true);

        commit_uploaded_temp(&remote, COMMIT_TEMP, COMMIT_TARGET, true)
            .await
            .unwrap();

        assert_eq!(remote.rename_count(), 1);
        assert_eq!(remote.content(COMMIT_TARGET).as_deref(), Some("new"));
    }

    #[tokio::test]
    async fn upload_commit_restores_original_when_commit_rename_fails() {
        // 调用序号 2 = 备份成功后提交临时文件的那次 rename。
        let remote = FakeRemote::new(&[(COMMIT_TEMP, "new"), (COMMIT_TARGET, "original")], false)
            .failing_at_rename(2);

        let error = commit_uploaded_temp(&remote, COMMIT_TEMP, COMMIT_TARGET, true)
            .await
            .unwrap_err();

        assert!(error.contains("原文件已恢复"), "{error}");
        assert_eq!(remote.content(COMMIT_TARGET).as_deref(), Some("original"));
        // 临时文件保留给调用方统一清理，备份不残留。
        assert_eq!(
            remote.paths(),
            vec![COMMIT_TEMP.to_string(), COMMIT_TARGET.to_string()]
        );
    }

    #[tokio::test]
    async fn upload_commit_skips_backup_dance_for_new_target() {
        let remote = FakeRemote::new(&[(COMMIT_TEMP, "new")], false).failing_at_rename(0);

        let error = commit_uploaded_temp(&remote, COMMIT_TEMP, COMMIT_TARGET, false)
            .await
            .unwrap_err();

        assert!(error.contains("模拟服务器拒绝提交"), "{error}");
        assert_eq!(remote.rename_count(), 1);
        assert_eq!(remote.paths(), vec![COMMIT_TEMP.to_string()]);
    }

    struct FailingReader {
        emitted_partial: bool,
    }

    impl AsyncRead for FailingReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            if !self.emitted_partial {
                buf.put_slice(b"partial");
                self.emitted_partial = true;
                Poll::Ready(Ok(()))
            } else {
                Poll::Ready(Err(std::io::Error::other("simulated read failure")))
            }
        }
    }

    #[tokio::test]
    async fn failed_download_preserves_existing_target_and_removes_temp() {
        let directory = temp_path("failed-download-dir");
        tokio::fs::create_dir(&directory).await.unwrap();
        let target = directory.join("target.txt");
        tokio::fs::write(&target, b"original").await.unwrap();
        let (out_tx, _out_rx) = mpsc::channel(4);
        let mut src = FailingReader {
            emitted_partial: false,
        };

        let error = download_to_local(
            &mut src,
            &target,
            SessionId(1),
            SftpRequestId(2),
            "target.txt",
            100,
            &out_tx,
        )
        .await
        .unwrap_err();

        assert!(error.contains("simulated read failure"));
        assert_eq!(tokio::fs::read(&target).await.unwrap(), b"original");
        let mut entries = tokio::fs::read_dir(&directory).await.unwrap();
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name());
        }
        assert_eq!(names, vec![std::ffi::OsString::from("target.txt")]);
        tokio::fs::remove_dir_all(&directory).await.unwrap();
    }

    #[tokio::test]
    async fn successful_download_commits_target_without_temp_leftover() {
        let directory = temp_path("successful-download-dir");
        tokio::fs::create_dir(&directory).await.unwrap();
        let target = directory.join("target.txt");
        let (out_tx, _out_rx) = mpsc::channel(4);
        let mut src = std::io::Cursor::new(b"complete".to_vec());

        download_to_local(
            &mut src,
            &target,
            SessionId(1),
            SftpRequestId(3),
            "target.txt",
            8,
            &out_tx,
        )
        .await
        .unwrap();

        assert_eq!(tokio::fs::read(&target).await.unwrap(), b"complete");
        let mut entries = tokio::fs::read_dir(&directory).await.unwrap();
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name());
        }
        assert_eq!(names, vec![std::ffi::OsString::from("target.txt")]);
        tokio::fs::remove_dir_all(&directory).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_download_atomically_replaces_existing_target_on_unix() {
        let directory = temp_path("replace-download-dir");
        tokio::fs::create_dir(&directory).await.unwrap();
        let target = directory.join("target.txt");
        tokio::fs::write(&target, b"original").await.unwrap();
        let (out_tx, _out_rx) = mpsc::channel(4);
        let mut src = std::io::Cursor::new(b"replacement".to_vec());

        download_to_local(
            &mut src,
            &target,
            SessionId(1),
            SftpRequestId(4),
            "target.txt",
            11,
            &out_tx,
        )
        .await
        .unwrap();

        assert_eq!(tokio::fs::read(&target).await.unwrap(), b"replacement");
        tokio::fs::remove_dir_all(&directory).await.unwrap();
    }

    #[tokio::test]
    async fn progress_event_keeps_request_id() {
        let mut src = std::io::Cursor::new(b"hello".to_vec());
        let mut dst = tokio::io::sink();
        let (out_tx, mut out_rx) = mpsc::channel(4);

        copy_with_progress(
            &mut src,
            &mut dst,
            SessionId(3),
            SftpRequestId(9),
            "hello.txt",
            5,
            &out_tx,
        )
        .await
        .unwrap();

        assert!(matches!(
            out_rx.recv().await,
            Some(FromCore::SftpProgress {
                id: SessionId(3),
                request_id: SftpRequestId(9),
                transferred: 5,
                total: 5,
                ..
            })
        ));
    }
}
