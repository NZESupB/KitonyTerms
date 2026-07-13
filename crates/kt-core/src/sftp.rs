//! SFTP 子任务 —— 在独立 tokio 任务中驱动 russh-sftp 的 [`SftpSession`]。
//!
//! SFTP subtask: drives a russh-sftp [`SftpSession`] in its own tokio task so
//! that long transfers never block the interactive shell loop. It consumes
//! [`SftpRequest`]s and emits [`FromCore`] events (listings, progress, done,
//! errors). A single failed operation reports an error but keeps the task alive.

use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
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

            if let Err(error) = session.rename(temp_path.clone(), remote.clone()).await {
                return Err(cleanup_remote_temp(
                    session,
                    &temp_path,
                    format!(
                        "安全提交上传 {remote} 失败，服务器可能不支持原子覆盖 rename；原文件保持不变：{error}"
                    ),
                )
                .await);
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
    let (directory, name) = match target.rsplit_once('/') {
        Some((directory, name)) => (Some(directory), name),
        None => (None, target),
    };
    let temp_name = format!(".{name}.kitonyterms-upload-{suffix}.tmp");
    match directory {
        Some("") => format!("/{temp_name}"),
        Some(directory) => format!("{directory}/{temp_name}"),
        None => temp_name,
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
