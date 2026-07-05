//! SFTP 子任务 —— 在独立 tokio 任务中驱动 russh-sftp 的 [`SftpSession`]。
//!
//! SFTP subtask: drives a russh-sftp [`SftpSession`] in its own tokio task so
//! that long transfers never block the interactive shell loop. It consumes
//! [`SftpRequest`]s and emits [`FromCore`] events (listings, progress, done,
//! errors). A single failed operation reports an error but keeps the task alive.

use russh_sftp::client::SftpSession;
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::session::{FromCore, SessionId, SftpEntry, SftpOp, SftpRequest};
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

/// SFTP 子任务主循环。`rx` 关闭(会话结束)时退出,`session` 随之 drop 关闭通道。
/// Main loop. Exits when `rx` closes (session ended); dropping `session` closes
/// the channel.
pub async fn sftp_task(
    id: SessionId,
    session: SftpSession,
    _connection_guard: Option<SshConnectionGuard>,
    mut rx: mpsc::UnboundedReceiver<SftpRequest>,
    out: mpsc::Sender<FromCore>,
) {
    while let Some(req) = rx.recv().await {
        if let Err(message) = handle(&session, id, &req, &out).await {
            let _ = out.send(FromCore::SftpError { id, message }).await;
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
            let mut dst = create_private_download_file(local)
                .await
                .map_err(|e| e.to_string())?;
            copy_with_progress(&mut src, &mut dst, id, &name, total, out).await?;
            dst.flush().await.map_err(|e| e.to_string())?;
            let _ = out
                .send(FromCore::SftpDone {
                    id,
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
            let mut dst = session
                .create(remote.clone())
                .await
                .map_err(|e| e.to_string())?;
            copy_with_progress(&mut src, &mut dst, id, &name, total, out).await?;
            // 关闭远端文件以刷新并提交写入。
            // Shut down the remote file to flush and commit the write.
            dst.shutdown().await.map_err(|e| e.to_string())?;
            let _ = out
                .send(FromCore::SftpDone {
                    id,
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
                    op: SftpOp::Rename,
                    path: to.clone(),
                })
                .await;
            Ok(())
        }
    }
}

async fn create_private_download_file(path: &Path) -> std::io::Result<tokio::fs::File> {
    let path = path.to_path_buf();
    let file = tokio::task::spawn_blocking(move || {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok::<_, std::io::Error>(file)
    })
    .await
    .map_err(std::io::Error::other)??;

    Ok(tokio::fs::File::from_std(file))
}

/// 分块拷贝并周期上报进度。完成时补发一条 100% 进度。
/// Copy in chunks while emitting throttled progress; emit a final 100% tick.
async fn copy_with_progress<R, W>(
    src: &mut R,
    dst: &mut W,
    id: SessionId,
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
    use std::time::{SystemTime, UNIX_EPOCH};

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
    async fn download_target_file_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_path("download");
        let mut file = create_private_download_file(&path).await.unwrap();
        file.write_all(b"secret").await.unwrap();
        drop(file);

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let _ = std::fs::remove_file(&path);
        assert_eq!(mode, 0o600);
    }
}
