//! Session orchestration and the UI⇄core message protocol.
//!
//! A [`SessionManager`] owns the tokio runtime and one task per SSH session.
//! Callers (the GUI or the headless example) talk to it exclusively through
//! channels:
//!
//! * [`ToCore`] — commands sent *into* the core (connect, input, resize, …)
//! * [`FromCore`] — events emitted *out* of the core (data ready, auth prompt,
//!   closed, …)
//!
//! This keeps all blocking/async SSH I/O off the UI thread. The UI just pumps
//! messages and repaints from [`GridSnapshot`]s.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use kt_config::ConnectParams;

use crate::monitor::MonitorStats;
use crate::ssh::{AuthProvider, HostKeyVerifier, PtySize, SshShell};
use crate::term::{GridSnapshot, TermEngine, TermEvent};

const SFTP_REUSE_OPEN_TIMEOUT: Duration = Duration::from_secs(8);
const SFTP_STANDALONE_OPEN_TIMEOUT: Duration = Duration::from_secs(20);
const MONITOR_OPEN_TIMEOUT: Duration = Duration::from_secs(8);
const SSH_OPEN_TIMEOUT: Duration = Duration::from_secs(45);

/// Opaque session identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub u64);

/// 远端目录条目(core 内中立类型,不向 UI 暴露 russh-sftp 的类型)。
/// A remote directory entry (a neutral type; russh-sftp types stay in the core).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SftpEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// Unix 修改时间(秒)。Unix mtime in seconds.
    pub modified: Option<u32>,
    /// Unix 权限位。Unix permission bits.
    pub permissions: Option<u32>,
    /// 远端返回的用户名称。Remote owner name.
    pub user: Option<String>,
    /// 远端返回的用户组名称。Remote group name.
    pub group: Option<String>,
    /// 远端返回的用户 ID。Remote owner id.
    pub uid: Option<u32>,
    /// 远端返回的用户组 ID。Remote group id.
    pub gid: Option<u32>,
}

/// SFTP 操作类型,随完成回执返回,便于 UI 决定后续动作(如刷新列表)。
/// The kind of completed SFTP operation, returned with the done event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SftpOp {
    Download,
    Upload,
    Mkdir,
    Remove,
    Rename,
}

/// 一次 SFTP 请求(路径均为远端 POSIX 路径,以 `/` 分隔)。
/// One SFTP request. Remote paths are POSIX (`/`-separated).
#[derive(Debug, Clone)]
pub enum SftpRequest {
    /// 列出远端目录。List a remote directory.
    List { path: String },
    /// 下载远端文件到本地。Download a remote file to a local path.
    Download {
        remote: String,
        local: std::path::PathBuf,
    },
    /// 上传本地文件到远端。Upload a local file to a remote path.
    Upload {
        local: std::path::PathBuf,
        remote: String,
    },
    /// 新建远端目录。Create a remote directory.
    Mkdir { path: String },
    /// 删除远端文件或目录。Remove a remote file or directory.
    Remove { path: String, is_dir: bool },
    /// 重命名/移动远端条目。Rename/move a remote entry.
    Rename { from: String, to: String },
}

/// Commands sent from the UI into the core.
#[derive(Debug)]
pub enum ToCore {
    /// Open a new connection under `id`.
    Connect {
        id: SessionId,
        params: Box<ConnectParams>,
        pty: PtySize,
    },
    /// Keyboard input bytes for a session's PTY.
    Input { id: SessionId, data: Vec<u8> },
    /// New terminal size (columns, rows).
    Resize { id: SessionId, cols: u16, rows: u16 },
    /// Scroll the viewport by `delta` lines (positive = into history).
    Scroll { id: SessionId, delta: i32 },
    /// An SFTP request on this session (opens the subsystem lazily on first use).
    Sftp { id: SessionId, req: SftpRequest },
    /// 启动该会话的资源监控(首次惰性开启,之后持续到断开)。
    /// Start resource monitoring (lazy on first use, runs until disconnect).
    StartMonitor { id: SessionId },
    /// Close / disconnect a session.
    Disconnect { id: SessionId },
}

/// Events emitted from the core out to the UI.
#[derive(Debug)]
pub enum FromCore {
    /// Connection + auth + shell are up.
    Connected { id: SessionId },
    /// New rendered grid available.
    Render {
        id: SessionId,
        snapshot: Box<GridSnapshot>,
    },
    /// Title changed (OSC).
    Title { id: SessionId, title: String },
    /// Terminal bell.
    Bell { id: SessionId },
    /// SFTP 目录列表就绪。SFTP directory listing is ready.
    SftpListing {
        id: SessionId,
        path: String,
        entries: Vec<SftpEntry>,
    },
    /// SFTP 传输进度(`total` 为 0 表示未知)。Transfer progress (`total` 0 = unknown).
    SftpProgress {
        id: SessionId,
        name: String,
        transferred: u64,
        total: u64,
    },
    /// SFTP 操作完成。An SFTP operation finished successfully.
    SftpDone {
        id: SessionId,
        op: SftpOp,
        path: String,
    },
    /// SFTP 操作失败。An SFTP operation failed.
    SftpError { id: SessionId, message: String },
    /// 资源监控采样。A resource-monitor sample.
    Monitor {
        id: SessionId,
        stats: Box<MonitorStats>,
    },
    /// 资源监控正常停止。Resource monitoring stopped without an error.
    MonitorStopped { id: SessionId },
    /// 资源监控启动或采样失败。Resource monitoring failed to start or sample.
    MonitorError { id: SessionId, message: String },
    /// Session ended. `error` is `None` for a clean exit.
    Closed {
        id: SessionId,
        error: Option<String>,
    },
}

/// Factory that produces a fresh [`AuthProvider`] per connection.
///
/// Auth providers are `Send` but generally not `Sync` (they may prompt), so
/// each session gets its own. The factory itself must be shareable.
pub trait AuthProviderFactory: Send + Sync {
    fn create(&self, id: SessionId, params: &ConnectParams) -> Box<dyn AuthProvider>;
}

/// Owns the runtime and live sessions.
pub struct SessionManager {
    to_core_tx: mpsc::UnboundedSender<ToCore>,
    from_core_rx: mpsc::UnboundedReceiver<FromCore>,
    _runtime: tokio::runtime::Runtime,
}

impl SessionManager {
    /// Spawn the core on a dedicated multi-threaded runtime.
    pub fn spawn(
        verifier: Arc<dyn HostKeyVerifier>,
        auth_factory: Arc<dyn AuthProviderFactory>,
    ) -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("kt-core")
            .build()?;

        let (to_core_tx, to_core_rx) = mpsc::unbounded_channel::<ToCore>();
        let (from_core_tx, from_core_rx) = mpsc::unbounded_channel::<FromCore>();

        runtime.spawn(core_loop(to_core_rx, from_core_tx, verifier, auth_factory));

        Ok(Self {
            to_core_tx,
            from_core_rx,
            _runtime: runtime,
        })
    }

    /// Send a command into the core.
    pub fn send(&self, msg: ToCore) {
        // Errors only occur if the core loop has stopped; ignore for now.
        let _ = self.to_core_tx.send(msg);
    }

    /// Clone the raw command sender. Useful for forwarding input from a
    /// separate thread (e.g. a stdin reader) without borrowing the manager.
    pub fn raw_sender(&self) -> mpsc::UnboundedSender<ToCore> {
        self.to_core_tx.clone()
    }

    /// Non-blocking poll for the next event from the core.
    pub fn try_recv(&mut self) -> Option<FromCore> {
        self.from_core_rx.try_recv().ok()
    }

    /// Blocking receive (used by the headless example).
    pub fn blocking_recv(&mut self) -> Option<FromCore> {
        self.from_core_rx.blocking_recv()
    }
}

/// Top-level core dispatch loop: routes [`ToCore`] commands to per-session tasks.
async fn core_loop(
    mut rx: mpsc::UnboundedReceiver<ToCore>,
    tx: mpsc::UnboundedSender<FromCore>,
    verifier: Arc<dyn HostKeyVerifier>,
    auth_factory: Arc<dyn AuthProviderFactory>,
) {
    // Per-session input/control senders.
    let mut sessions: HashMap<SessionId, SessionHandles> = HashMap::new();

    while let Some(cmd) = rx.recv().await {
        match cmd {
            ToCore::Connect { id, params, pty } => {
                let (input_tx, input_rx) = mpsc::unbounded_channel::<SessionCmd>();
                sessions.insert(id, SessionHandles { cmd_tx: input_tx });

                let provider = auth_factory.create(id, &params);
                let task = SessionTask {
                    id,
                    params: *params,
                    pty,
                    verifier: verifier.clone(),
                    provider,
                    out: tx.clone(),
                    cmd_rx: input_rx,
                };
                tokio::spawn(task.run());
            }
            ToCore::Input { id, data } => {
                if let Some(h) = sessions.get(&id) {
                    let _ = h.cmd_tx.send(SessionCmd::Input(data));
                }
            }
            ToCore::Resize { id, cols, rows } => {
                if let Some(h) = sessions.get(&id) {
                    let _ = h.cmd_tx.send(SessionCmd::Resize { cols, rows });
                }
            }
            ToCore::Scroll { id, delta } => {
                if let Some(h) = sessions.get(&id) {
                    let _ = h.cmd_tx.send(SessionCmd::Scroll(delta));
                }
            }
            ToCore::Sftp { id, req } => {
                if let Some(h) = sessions.get(&id) {
                    if h.cmd_tx.send(SessionCmd::Sftp(req)).is_err() {
                        let _ = tx.send(FromCore::SftpError {
                            id,
                            message: "SFTP 请求无法投递，会话任务已结束".to_string(),
                        });
                    }
                } else {
                    let _ = tx.send(FromCore::SftpError {
                        id,
                        message: "SFTP 请求失败：会话不存在或已关闭".to_string(),
                    });
                }
            }
            ToCore::StartMonitor { id } => {
                if let Some(h) = sessions.get(&id) {
                    if h.cmd_tx.send(SessionCmd::StartMonitor).is_err() {
                        let _ = tx.send(FromCore::MonitorError {
                            id,
                            message: "资源监控请求无法投递，会话任务已结束".to_string(),
                        });
                    }
                } else {
                    let _ = tx.send(FromCore::MonitorError {
                        id,
                        message: "资源监控启动失败：会话不存在或已关闭".to_string(),
                    });
                }
            }
            ToCore::Disconnect { id } => {
                if let Some(h) = sessions.remove(&id) {
                    let _ = h.cmd_tx.send(SessionCmd::Disconnect);
                }
            }
        }
    }
}

/// Control messages routed to a single session task.
enum SessionCmd {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Scroll(i32),
    Sftp(SftpRequest),
    StartMonitor,
    Disconnect,
}

enum SessionInternal {
    MonitorExited(crate::monitor::MonitorExit),
}

struct SessionHandles {
    cmd_tx: mpsc::UnboundedSender<SessionCmd>,
}

/// All state for one session's task.
struct SessionTask {
    id: SessionId,
    params: ConnectParams,
    pty: PtySize,
    verifier: Arc<dyn HostKeyVerifier>,
    provider: Box<dyn AuthProvider>,
    out: mpsc::UnboundedSender<FromCore>,
    cmd_rx: mpsc::UnboundedReceiver<SessionCmd>,
}

impl SessionTask {
    async fn run(mut self) {
        let id = self.id;

        let mut shell = match open_ssh_shell_with_timeout(
            SSH_OPEN_TIMEOUT,
            SshShell::open(
                &self.params,
                self.pty,
                self.verifier.clone(),
                self.provider.as_mut(),
            ),
        )
        .await
        {
            Ok(s) => s,
            Err(error) => {
                let _ = self.out.send(FromCore::Closed {
                    id,
                    error: Some(error),
                });
                return;
            }
        };

        let _ = self.out.send(FromCore::Connected { id });

        // Build the terminal engine at the negotiated size.
        let scrollback = 10_000;
        let mut term = TermEngine::new(self.pty.cols as usize, self.pty.rows as usize, scrollback);

        // Emit the initial (blank) frame.
        self.emit_render(&term);

        let mut close_error: Option<String> = None;

        // SFTP 子任务的命令发送端,首次收到 SFTP 请求时惰性建立。
        // Command sender to the SFTP subtask, created lazily on first request.
        let mut sftp_tx: Option<mpsc::UnboundedSender<SftpRequest>> = None;

        // 资源监控子任务是否已启动(惰性,首次请求时开启)。
        // Whether the monitor subtask has been started (lazy on first request).
        let mut monitor_started = false;
        let (internal_tx, mut internal_rx) = mpsc::unbounded_channel::<SessionInternal>();

        loop {
            tokio::select! {
                internal = internal_rx.recv() => {
                    match internal {
                        Some(SessionInternal::MonitorExited(exit)) => {
                            monitor_started = false;
                            if matches!(exit, crate::monitor::MonitorExit::Stopped) {
                                let _ = self.out.send(FromCore::MonitorStopped { id });
                            }
                        }
                        None => {}
                    }
                }

                // Control messages from the UI.
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(SessionCmd::Input(data)) => {
                            if let Err(e) = shell.write(&data).await {
                                close_error = Some(e.to_string());
                                break;
                            }
                        }
                        Some(SessionCmd::Resize { cols, rows }) => {
                            term.resize(cols as usize, rows as usize, scrollback);
                            let _ = shell.resize(cols, rows).await;
                            self.emit_render(&term);
                        }
                        Some(SessionCmd::Scroll(delta)) => {
                            term.scroll(delta);
                            self.emit_render(&term);
                        }
                        Some(SessionCmd::Sftp(req)) => {
                            // 首次使用时在同一会话上开 SFTP 子系统,并 move 进独立子任务。
                            // Open the SFTP subsystem lazily and move it into a subtask.
                            if sftp_tx.is_none() {
                                let primary_error = match tokio::time::timeout(
                                    SFTP_REUSE_OPEN_TIMEOUT,
                                    shell.open_sftp(),
                                )
                                .await
                                {
                                    Ok(Ok(session)) => {
                                        let (tx, rx) = mpsc::unbounded_channel();
                                        tokio::spawn(crate::sftp::sftp_task(
                                            id,
                                            session,
                                            None,
                                            rx,
                                            self.out.clone(),
                                        ));
                                        sftp_tx = Some(tx);
                                        None
                                    }
                                    Ok(Err(e)) => Some(format!("复用当前 SSH 会话失败：{e}")),
                                    Err(_) => Some(format!(
                                        "复用当前 SSH 会话超时({} 秒)",
                                        SFTP_REUSE_OPEN_TIMEOUT.as_secs()
                                    )),
                                };

                                if sftp_tx.is_none() {
                                    match tokio::time::timeout(
                                        SFTP_STANDALONE_OPEN_TIMEOUT,
                                        SshShell::open_standalone_sftp(
                                            &self.params,
                                            self.verifier.clone(),
                                            self.provider.as_mut(),
                                        ),
                                    )
                                    .await
                                    {
                                        Ok(Ok((session, guard))) => {
                                            let (tx, rx) = mpsc::unbounded_channel();
                                            tokio::spawn(crate::sftp::sftp_task(
                                                id,
                                                session,
                                                Some(guard),
                                                rx,
                                                self.out.clone(),
                                            ));
                                            sftp_tx = Some(tx);
                                        }
                                        Ok(Err(e)) => {
                                            let prefix = primary_error
                                                .as_deref()
                                                .unwrap_or("复用当前 SSH 会话失败");
                                            let _ = self.out.send(FromCore::SftpError {
                                                id,
                                                message: format!(
                                                    "打开 SFTP 子系统失败：{prefix}；独立连接也失败：{e}"
                                                ),
                                            });
                                        }
                                        Err(_) => {
                                            let prefix = primary_error
                                                .as_deref()
                                                .unwrap_or("复用当前 SSH 会话失败");
                                            let _ = self.out.send(FromCore::SftpError {
                                                id,
                                                message: format!(
                                                    "打开 SFTP 子系统失败：{prefix}；独立连接超时({} 秒)",
                                                    SFTP_STANDALONE_OPEN_TIMEOUT.as_secs()
                                                ),
                                            });
                                        }
                                    }
                                }
                            }
                            if let Some(tx) = &sftp_tx {
                                if tx.send(req).is_err() {
                                    sftp_tx = None;
                                    let _ = self.out.send(FromCore::SftpError {
                                        id,
                                        message: "SFTP 子任务已退出，请刷新后重试".to_string(),
                                    });
                                }
                            }
                        }
                        Some(SessionCmd::StartMonitor) => {
                            // 首次请求时在同一会话上开监控通道,并 move 进独立子任务。
                            // Open the monitor channel lazily and move it into a subtask.
                            if !monitor_started {
                                match tokio::time::timeout(
                                    MONITOR_OPEN_TIMEOUT,
                                    shell.open_monitor_channel(),
                                )
                                .await
                                {
                                    Ok(Ok(session)) => {
                                        let out = self.out.clone();
                                        let internal_tx = internal_tx.clone();
                                        tokio::spawn(async move {
                                            let exit =
                                                crate::monitor::monitor_task(id, session, out).await;
                                            let _ = internal_tx.send(SessionInternal::MonitorExited(exit));
                                        });
                                        monitor_started = true;
                                    }
                                    Ok(Err(e)) => {
                                        tracing::warn!("failed to start monitor: {e}");
                                        let _ = self.out.send(FromCore::MonitorError {
                                            id,
                                            message: format!("资源监控启动失败：{e}"),
                                        });
                                    }
                                    Err(_) => {
                                        let _ = self.out.send(FromCore::MonitorError {
                                            id,
                                            message: format!(
                                                "资源监控启动超时({} 秒)",
                                                MONITOR_OPEN_TIMEOUT.as_secs()
                                            ),
                                        });
                                    }
                                }
                            }
                        }
                        Some(SessionCmd::Disconnect) | None => {
                            let _ = shell.disconnect().await;
                            break;
                        }
                    }
                }

                // Output / channel events from the remote.
                msg = shell.next_message() => {
                    match msg {
                        Some(russh::ChannelMsg::Data { data }) => {
                            term.advance(&data);
                            self.handle_term_events(&term);
                            self.emit_render(&term);
                        }
                        Some(russh::ChannelMsg::ExtendedData { data, .. }) => {
                            // stderr — feed it to the terminal too.
                            term.advance(&data);
                            self.emit_render(&term);
                        }
                        Some(russh::ChannelMsg::Eof) | Some(russh::ChannelMsg::Close) => {
                            break;
                        }
                        Some(russh::ChannelMsg::ExitStatus { exit_status }) => {
                            if exit_status != 0 {
                                close_error = Some(format!("remote shell exited with status {exit_status}"));
                            }
                            // wait for Close/Eof to actually break.
                        }
                        Some(_) => {}
                        None => break, // channel fully closed
                    }
                }
            }
        }

        let _ = self.out.send(FromCore::Closed {
            id,
            error: close_error,
        });
    }

    /// Forward terminal events (bell/title/pty-write) to the UI / back to PTY.
    fn handle_term_events(&self, term: &TermEngine) {
        for ev in term.take_events() {
            match ev {
                TermEvent::Bell => {
                    let _ = self.out.send(FromCore::Bell { id: self.id });
                }
                TermEvent::Title(title) => {
                    let _ = self.out.send(FromCore::Title { id: self.id, title });
                }
                // PtyWrite would be written back to the shell; deferred until
                // needed (device-status responses etc.).
                TermEvent::PtyWrite(_) | TermEvent::Wakeup => {}
            }
        }
    }

    fn emit_render(&self, term: &TermEngine) {
        let snapshot = Box::new(term.snapshot());
        let _ = self.out.send(FromCore::Render {
            id: self.id,
            snapshot,
        });
    }
}

async fn open_ssh_shell_with_timeout<T, E, F>(
    timeout: Duration,
    open_fut: F,
) -> std::result::Result<T, String>
where
    E: ToString,
    F: Future<Output = std::result::Result<T, E>>,
{
    match tokio::time::timeout(timeout, open_fut).await {
        Ok(Ok(shell)) => Ok(shell),
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => Err(format!(
            "SSH 连接超时({} 秒)，连接流程未在限定时间内完成。请检查网络、ProxyJump、认证方式或远端 shell。",
            timeout.as_secs()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::{AcceptAllVerifier, AuthProvider};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    struct NoopAuth;

    impl AuthProvider for NoopAuth {
        fn password(&mut self, _user: &str, _host: &str, _port: u16) -> Option<String> {
            None
        }

        fn key_passphrase(&mut self, _key_path: &str) -> Option<String> {
            None
        }

        fn keyboard_interactive(
            &mut self,
            _name: &str,
            _instructions: &str,
            _prompts: &[(String, bool)],
        ) -> Option<Vec<String>> {
            None
        }
    }

    struct NoopFactory;

    impl AuthProviderFactory for NoopFactory {
        fn create(&self, _id: SessionId, _params: &ConnectParams) -> Box<dyn AuthProvider> {
            Box::new(NoopAuth)
        }
    }

    #[test]
    fn sftp_request_for_missing_session_returns_error() {
        let mut manager =
            SessionManager::spawn(Arc::new(AcceptAllVerifier), Arc::new(NoopFactory)).unwrap();

        manager.send(ToCore::Sftp {
            id: SessionId(404),
            req: SftpRequest::List {
                path: ".".to_string(),
            },
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        let event = loop {
            if let Some(event) = manager.try_recv() {
                break event;
            }
            assert!(Instant::now() < deadline, "等待 SFTP 错误事件超时");
            std::thread::sleep(Duration::from_millis(10));
        };

        match event {
            FromCore::SftpError { id, message } => {
                assert_eq!(id, SessionId(404));
                assert!(message.contains("会话不存在"));
            }
            other => panic!("期望 SftpError，实际收到 {other:?}"),
        }
    }

    #[test]
    fn monitor_request_for_missing_session_returns_error() {
        let mut manager =
            SessionManager::spawn(Arc::new(AcceptAllVerifier), Arc::new(NoopFactory)).unwrap();

        manager.send(ToCore::StartMonitor { id: SessionId(404) });

        let deadline = Instant::now() + Duration::from_secs(2);
        let event = loop {
            if let Some(event) = manager.try_recv() {
                break event;
            }
            assert!(Instant::now() < deadline, "等待监控错误事件超时");
            std::thread::sleep(Duration::from_millis(10));
        };

        match event {
            FromCore::MonitorError { id, message } => {
                assert_eq!(id, SessionId(404));
                assert!(message.contains("会话不存在"));
            }
            other => panic!("期望 MonitorError，实际收到 {other:?}"),
        }
    }

    #[tokio::test]
    async fn ssh_open_timeout_turns_pending_connect_into_error() {
        let result: std::result::Result<(), String> = open_ssh_shell_with_timeout(
            Duration::from_millis(1),
            std::future::pending::<std::result::Result<(), &'static str>>(),
        )
        .await;

        let err = result.expect_err("pending 连接应当被超时打断");
        assert!(err.contains("SSH 连接超时"));
    }
}
