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

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::sync::{mpsc as std_mpsc, Arc};
use std::time::Duration;

use tokio::sync::mpsc;

use kt_config::ConnectParams;

use crate::monitor::MonitorStats;
use crate::ssh::{AuthProvider, HostKeyVerifier, PtySize, SshError, SshShell};
use crate::term::{GridSnapshot, TermEngine, TermEvent};

const AUTH_RESPONSE_TIMEOUT: Duration = Duration::from_secs(45);
const SFTP_REUSE_OPEN_TIMEOUT: Duration = Duration::from_secs(8);
const SFTP_STANDALONE_OPEN_TIMEOUT: Duration = Duration::from_secs(20);
const MONITOR_OPEN_TIMEOUT: Duration = Duration::from_secs(8);
const SSH_OPEN_TIMEOUT: Duration = Duration::from_secs(45);
const TO_CORE_CAPACITY: usize = 2_048;
const FROM_CORE_CAPACITY: usize = 2_048;
const OSC7_MAX_SEQUENCE_LEN: usize = 4 * 1024;

/// Opaque session identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub u64);

/// 一次 SFTP 请求的稳定标识，由调用方生成并随请求级事件原样返回。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SftpRequestId(pub u64);

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

/// 认证交互提示。`echo=false` 表示应以密码输入框展示。
/// An authentication prompt. `echo=false` means the answer should be hidden.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthPrompt {
    pub text: String,
    pub echo: bool,
}

/// core 发给 UI 的认证挑战。
/// Authentication challenge emitted by the core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthChallenge {
    Password {
        user: String,
        host: String,
        port: u16,
    },
    KeyPassphrase {
        key_path: String,
    },
    KeyboardInteractive {
        name: String,
        instructions: String,
        prompts: Vec<AuthPrompt>,
    },
}

/// UI 回给 core 的认证答案。
/// Authentication response returned by the UI.
#[derive(Clone, PartialEq, Eq)]
pub enum AuthResponse {
    Answers(Vec<String>),
    Cancel,
}

impl fmt::Debug for AuthResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthResponse::Answers(answers) => {
                write!(f, "Answers(<{} value(s) redacted>)", answers.len())
            }
            AuthResponse::Cancel => f.write_str("Cancel"),
        }
    }
}

/// Commands sent from the UI into the core.
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
    Sftp {
        id: SessionId,
        request_id: SftpRequestId,
        req: SftpRequest,
    },
    /// 启动该会话的资源监控(首次惰性开启,之后持续到断开)。
    /// Start resource monitoring (lazy on first use, runs until disconnect).
    StartMonitor { id: SessionId },
    /// Answer or cancel an authentication challenge.
    AuthResponse {
        id: SessionId,
        response: AuthResponse,
    },
    /// Close / disconnect a session.
    Disconnect { id: SessionId },
}

impl fmt::Debug for ToCore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToCore::Connect { id, params, pty } => f
                .debug_struct("Connect")
                .field("id", id)
                .field("host", &params.host)
                .field("port", &params.port)
                .field("user", &params.user)
                .field("auth_methods", &params.auth.len())
                .field("proxy_jump", &params.proxy_jump.is_some())
                .field("forward_agent", &params.forward_agent)
                .field("pty", pty)
                .finish(),
            ToCore::Input { id, data } => f
                .debug_struct("Input")
                .field("id", id)
                .field("bytes", &data.len())
                .finish(),
            ToCore::Resize { id, cols, rows } => f
                .debug_struct("Resize")
                .field("id", id)
                .field("cols", cols)
                .field("rows", rows)
                .finish(),
            ToCore::Scroll { id, delta } => f
                .debug_struct("Scroll")
                .field("id", id)
                .field("delta", delta)
                .finish(),
            ToCore::Sftp {
                id,
                request_id,
                req,
            } => f
                .debug_struct("Sftp")
                .field("id", id)
                .field("request_id", request_id)
                .field("req", req)
                .finish(),
            ToCore::StartMonitor { id } => f.debug_struct("StartMonitor").field("id", id).finish(),
            ToCore::AuthResponse { id, response } => f
                .debug_struct("AuthResponse")
                .field("id", id)
                .field("response", response)
                .finish(),
            ToCore::Disconnect { id } => f.debug_struct("Disconnect").field("id", id).finish(),
        }
    }
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
    /// 远端 shell 通过 OSC 7 上报的当前工作目录（用于文件管理跟随终端目录）。
    /// The remote shell's current working directory, reported via OSC 7.
    Cwd { id: SessionId, path: String },
    /// Terminal bell.
    Bell { id: SessionId },
    /// SFTP 目录列表就绪。SFTP directory listing is ready.
    SftpListing {
        id: SessionId,
        request_id: SftpRequestId,
        path: String,
        entries: Vec<SftpEntry>,
    },
    /// SFTP 传输进度(`total` 为 0 表示未知)。Transfer progress (`total` 0 = unknown).
    SftpProgress {
        id: SessionId,
        request_id: SftpRequestId,
        name: String,
        transferred: u64,
        total: u64,
    },
    /// SFTP 操作完成。An SFTP operation finished successfully.
    SftpDone {
        id: SessionId,
        request_id: SftpRequestId,
        op: SftpOp,
        path: String,
    },
    /// SFTP 操作失败。An SFTP operation failed.
    SftpError {
        id: SessionId,
        request_id: SftpRequestId,
        message: String,
    },
    /// SFTP 子任务正常停止。SFTP subtask stopped without a per-operation error.
    SftpStopped { id: SessionId },
    /// 资源监控采样。A resource-monitor sample.
    Monitor {
        id: SessionId,
        stats: Box<MonitorStats>,
    },
    /// 资源监控正常停止。Resource monitoring stopped without an error.
    MonitorStopped { id: SessionId },
    /// 资源监控启动或采样失败。Resource monitoring failed to start or sample.
    MonitorError { id: SessionId, message: String },
    /// 需要 UI 提供认证输入。Authentication input is required from the UI.
    AuthChallenge {
        id: SessionId,
        challenge: AuthChallenge,
    },
    /// 主机密钥需要用户确认；本次握手会随后的 Closed 事件结束。
    /// Host key confirmation is pending; the current handshake will close.
    HostKeyPending { id: SessionId },
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
    to_core_tx: mpsc::Sender<ToCore>,
    from_core_rx: mpsc::Receiver<FromCore>,
    event_buffer: VecDeque<FromCore>,
    pending_renders: HashMap<SessionId, Box<GridSnapshot>>,
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

        let (to_core_tx, to_core_rx) = mpsc::channel::<ToCore>(TO_CORE_CAPACITY);
        let (from_core_tx, from_core_rx) = mpsc::channel::<FromCore>(FROM_CORE_CAPACITY);

        runtime.spawn(core_loop(to_core_rx, from_core_tx, verifier, auth_factory));

        Ok(Self {
            to_core_tx,
            from_core_rx,
            event_buffer: VecDeque::new(),
            pending_renders: HashMap::new(),
            _runtime: runtime,
        })
    }

    /// Send a command into the core.
    pub fn send(&self, msg: ToCore) -> bool {
        match self.to_core_tx.try_send(msg) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(msg)) => {
                tracing::warn!("core 命令队列已满，丢弃命令: {msg:?}");
                false
            }
            Err(mpsc::error::TrySendError::Closed(msg)) => {
                tracing::warn!("core 命令队列已关闭，丢弃命令: {msg:?}");
                false
            }
        }
    }

    /// Clone the raw command sender. Useful for forwarding input from a
    /// separate thread (e.g. a stdin reader) without borrowing the manager.
    pub fn raw_sender(&self) -> mpsc::Sender<ToCore> {
        self.to_core_tx.clone()
    }

    /// Non-blocking poll for the next event from the core.
    pub fn try_recv(&mut self) -> Option<FromCore> {
        if let Some(event) = self.event_buffer.pop_front() {
            return Some(event);
        }

        self.drain_available_events();

        self.event_buffer
            .pop_front()
            .or_else(|| self.pop_pending_render())
    }

    /// Blocking receive (used by the headless example).
    pub fn blocking_recv(&mut self) -> Option<FromCore> {
        if let Some(event) = self.event_buffer.pop_front() {
            return Some(event);
        }
        if let Some(event) = self.pop_pending_render() {
            return Some(event);
        }
        self.from_core_rx.blocking_recv()
    }

    fn drain_available_events(&mut self) {
        while let Ok(event) = self.from_core_rx.try_recv() {
            match event {
                FromCore::Render { id, snapshot } => {
                    self.pending_renders.insert(id, snapshot);
                }
                other => self.event_buffer.push_back(other),
            }
        }
    }

    fn pop_pending_render(&mut self) -> Option<FromCore> {
        let id = self.pending_renders.keys().copied().min()?;
        let snapshot = self.pending_renders.remove(&id)?;
        Some(FromCore::Render { id, snapshot })
    }
}

/// Top-level core dispatch loop: routes [`ToCore`] commands to per-session tasks.
async fn core_loop(
    mut rx: mpsc::Receiver<ToCore>,
    tx: mpsc::Sender<FromCore>,
    verifier: Arc<dyn HostKeyVerifier>,
    auth_factory: Arc<dyn AuthProviderFactory>,
) {
    // Per-session input/control senders.
    let mut sessions: HashMap<SessionId, SessionHandles> = HashMap::new();

    while let Some(cmd) = rx.recv().await {
        match cmd {
            ToCore::Connect { id, params, pty } => {
                let (input_tx, input_rx) = mpsc::unbounded_channel::<SessionCmd>();
                let (auth_response_tx, auth_response_rx) = std_mpsc::channel::<AuthResponse>();
                sessions.insert(
                    id,
                    SessionHandles {
                        cmd_tx: input_tx,
                        auth_response_tx,
                    },
                );

                let provider = auth_factory.create(id, &params);
                let provider = Box::new(InteractiveAuthProvider {
                    id,
                    inner: provider,
                    out: tx.clone(),
                    responses: auth_response_rx,
                });
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
            ToCore::Sftp {
                id,
                request_id,
                req,
            } => {
                if let Some(h) = sessions.get(&id) {
                    if h.cmd_tx.send(SessionCmd::Sftp { request_id, req }).is_err() {
                        let _ = tx
                            .send(FromCore::SftpError {
                                id,
                                request_id,
                                message: "SFTP 请求无法投递，会话任务已结束".to_string(),
                            })
                            .await;
                    }
                } else {
                    let _ = tx
                        .send(FromCore::SftpError {
                            id,
                            request_id,
                            message: "SFTP 请求失败：会话不存在或已关闭".to_string(),
                        })
                        .await;
                }
            }
            ToCore::StartMonitor { id } => {
                if let Some(h) = sessions.get(&id) {
                    if h.cmd_tx.send(SessionCmd::StartMonitor).is_err() {
                        let _ = tx
                            .send(FromCore::MonitorError {
                                id,
                                message: "资源监控请求无法投递，会话任务已结束".to_string(),
                            })
                            .await;
                    }
                } else {
                    let _ = tx
                        .send(FromCore::MonitorError {
                            id,
                            message: "资源监控启动失败：会话不存在或已关闭".to_string(),
                        })
                        .await;
                }
            }
            ToCore::AuthResponse { id, response } => {
                if let Some(h) = sessions.get(&id) {
                    let _ = h.auth_response_tx.send(response);
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
    Resize {
        cols: u16,
        rows: u16,
    },
    Scroll(i32),
    Sftp {
        request_id: SftpRequestId,
        req: SftpRequest,
    },
    StartMonitor,
    Disconnect,
}

enum SessionInternal {
    MonitorExited(crate::monitor::MonitorExit),
}

struct SessionHandles {
    cmd_tx: mpsc::UnboundedSender<SessionCmd>,
    auth_response_tx: std_mpsc::Sender<AuthResponse>,
}

struct InteractiveAuthProvider {
    id: SessionId,
    inner: Box<dyn AuthProvider>,
    out: mpsc::Sender<FromCore>,
    responses: std_mpsc::Receiver<AuthResponse>,
}

impl InteractiveAuthProvider {
    fn request_answers(&mut self, challenge: AuthChallenge) -> Option<Vec<String>> {
        if self
            .out
            .try_send(FromCore::AuthChallenge {
                id: self.id,
                challenge,
            })
            .is_err()
        {
            tracing::warn!("认证挑战无法投递，取消认证: {:?}", self.id);
            return None;
        }

        let response = match tokio::runtime::Handle::try_current() {
            Ok(handle)
                if matches!(
                    handle.runtime_flavor(),
                    tokio::runtime::RuntimeFlavor::MultiThread
                ) =>
            {
                tokio::task::block_in_place(|| self.responses.recv_timeout(AUTH_RESPONSE_TIMEOUT))
            }
            _ => self.responses.recv_timeout(AUTH_RESPONSE_TIMEOUT),
        };

        match response {
            Ok(AuthResponse::Answers(answers)) => Some(answers),
            Ok(AuthResponse::Cancel) => None,
            Err(err) => {
                tracing::warn!("等待认证响应超时或中断: {:?} {}", self.id, err);
                None
            }
        }
    }

    fn request_single_answer(&mut self, challenge: AuthChallenge) -> Option<String> {
        self.request_answers(challenge)
            .and_then(|answers| answers.into_iter().next())
    }
}

impl AuthProvider for InteractiveAuthProvider {
    fn password(&mut self, user: &str, host: &str, port: u16) -> Option<String> {
        if let Some(password) = self.inner.password(user, host, port) {
            return Some(password);
        }
        self.request_single_answer(AuthChallenge::Password {
            user: user.to_string(),
            host: host.to_string(),
            port,
        })
    }

    fn key_passphrase(&mut self, key_path: &str) -> Option<String> {
        if let Some(passphrase) = self.inner.key_passphrase(key_path) {
            return Some(passphrase);
        }
        self.request_single_answer(AuthChallenge::KeyPassphrase {
            key_path: key_path.to_string(),
        })
    }

    fn keyboard_interactive(
        &mut self,
        name: &str,
        instructions: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<String>> {
        if let Some(answers) = self.inner.keyboard_interactive(name, instructions, prompts) {
            return Some(answers);
        }

        let prompt_count = prompts.len();
        let answers = self.request_answers(AuthChallenge::KeyboardInteractive {
            name: name.to_string(),
            instructions: instructions.to_string(),
            prompts: prompts
                .iter()
                .map(|(text, echo)| AuthPrompt {
                    text: text.clone(),
                    echo: *echo,
                })
                .collect(),
        })?;

        if answers.len() == prompt_count {
            Some(answers)
        } else {
            tracing::warn!(
                "keyboard-interactive 响应数量不匹配: expected={}, actual={}",
                prompt_count,
                answers.len()
            );
            None
        }
    }
}

/// All state for one session's task.
struct SessionTask {
    id: SessionId,
    params: ConnectParams,
    pty: PtySize,
    verifier: Arc<dyn HostKeyVerifier>,
    provider: Box<dyn AuthProvider>,
    out: mpsc::Sender<FromCore>,
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
            Err(OpenSshShellError::HostKeyPending(error)) => {
                let _ = self.out.send(FromCore::HostKeyPending { id }).await;
                let _ = self
                    .out
                    .send(FromCore::Closed {
                        id,
                        error: Some(error),
                    })
                    .await;
                return;
            }
            Err(OpenSshShellError::Failed(error)) => {
                let _ = self
                    .out
                    .send(FromCore::Closed {
                        id,
                        error: Some(error),
                    })
                    .await;
                return;
            }
        };

        let _ = self.out.send(FromCore::Connected { id }).await;

        // Build the terminal engine at the negotiated size.
        let scrollback = 10_000;
        let mut term = TermEngine::new(self.pty.cols as usize, self.pty.rows as usize, scrollback);

        // Emit the initial (blank) frame.
        self.emit_render(&term);

        let mut close_error: Option<String> = None;

        // 最近一次通过 OSC 7 上报的工作目录，用于去重，避免重复投递 Cwd。
        // Last CWD reported via OSC 7, used to dedupe repeated Cwd events.
        let mut last_cwd: Option<String> = None;
        let mut osc7_scanner = Osc7Scanner::default();

        // SFTP 子任务的命令发送端,首次收到 SFTP 请求时惰性建立。
        // Command sender to the SFTP subtask, created lazily on first request.
        let mut sftp_tx: Option<mpsc::UnboundedSender<(SftpRequestId, SftpRequest)>> = None;

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
                                let _ = self.out.send(FromCore::MonitorStopped { id }).await;
                            }
                        }
                        None => {}
                    }
                }

                // Control messages from the UI.
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(SessionCmd::Input(data)) => {
                            // 用户在查看历史输出时开始输入，应立即回到当前命令行；
                            // 空输入不改变视口，也避免实时视图下每次按键产生额外渲染。
                            if prepare_terminal_for_input(&mut term, &data) {
                                self.emit_render(&term);
                            }
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
                        Some(SessionCmd::Sftp { request_id, req }) => {
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
                                                request_id,
                                                message: format!(
                                                    "打开 SFTP 子系统失败：{prefix}；独立连接也失败：{e}"
                                                ),
                                            }).await;
                                        }
                                        Err(_) => {
                                            let prefix = primary_error
                                                .as_deref()
                                                .unwrap_or("复用当前 SSH 会话失败");
                                            let _ = self.out.send(FromCore::SftpError {
                                                id,
                                                request_id,
                                                message: format!(
                                                    "打开 SFTP 子系统失败：{prefix}；独立连接超时({} 秒)",
                                                    SFTP_STANDALONE_OPEN_TIMEOUT.as_secs()
                                                ),
                                            }).await;
                                        }
                                    }
                                }
                            }
                            if let Some(tx) = &sftp_tx {
                                if tx.send((request_id, req)).is_err() {
                                    sftp_tx = None;
                                    let _ = self.out.send(FromCore::SftpError {
                                        id,
                                        request_id,
                                        message: "SFTP 子任务已退出，请刷新后重试".to_string(),
                                    }).await;
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
                                        let latency_target =
                                            crate::monitor::LatencyProbeTarget::new(
                                                self.params.host.clone(),
                                                self.params.port,
                                            );
                                        tokio::spawn(async move {
                                            let exit = crate::monitor::monitor_task(
                                                id,
                                                session,
                                                latency_target,
                                                out,
                                            )
                                            .await;
                                            let _ = internal_tx.send(SessionInternal::MonitorExited(exit));
                                        });
                                        monitor_started = true;
                                    }
                                    Ok(Err(e)) => {
                                        tracing::warn!("failed to start monitor: {e}");
                                        let _ = self.out.send(FromCore::MonitorError {
                                            id,
                                            message: format!("资源监控启动失败：{e}"),
                                        }).await;
                                    }
                                    Err(_) => {
                                        let _ = self.out.send(FromCore::MonitorError {
                                            id,
                                            message: format!(
                                                "资源监控启动超时({} 秒)",
                                                MONITOR_OPEN_TIMEOUT.as_secs()
                                            ),
                                        }).await;
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
                            for path in osc7_scanner.feed(&data) {
                                if last_cwd.as_deref() != Some(path.as_str()) {
                                    last_cwd = Some(path.clone());
                                    let _ = self.out.send(FromCore::Cwd { id, path }).await;
                                }
                            }
                            term.advance(&data);
                            let writes = handle_term_events(
                                self.id,
                                self.out.clone(),
                                term.take_events(),
                            ).await;
                            if let Err(error) = write_pty_responses(&shell, writes).await {
                                close_error = Some(error.to_string());
                                break;
                            }
                            self.emit_render(&term);
                        }
                        Some(russh::ChannelMsg::ExtendedData { data, .. }) => {
                            // stderr — feed it to the terminal too.
                            term.advance(&data);
                            let writes = handle_term_events(
                                self.id,
                                self.out.clone(),
                                term.take_events(),
                            ).await;
                            if let Err(error) = write_pty_responses(&shell, writes).await {
                                close_error = Some(error.to_string());
                                break;
                            }
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

        let _ = self
            .out
            .send(FromCore::Closed {
                id,
                error: close_error,
            })
            .await;
    }

    fn emit_render(&self, term: &TermEngine) {
        let snapshot = Box::new(term.snapshot());
        if let Err(err) = self.out.try_send(FromCore::Render {
            id: self.id,
            snapshot,
        }) {
            match err {
                mpsc::error::TrySendError::Full(_) => {
                    tracing::debug!("core 输出队列已满，丢弃一帧终端渲染");
                }
                mpsc::error::TrySendError::Closed(_) => {}
            }
        }
    }
}

fn prepare_terminal_for_input(term: &mut TermEngine, data: &[u8]) -> bool {
    !data.is_empty() && term.scroll_to_bottom()
}

/// 跨 PTY 数据块扫描 OSC 7；只保留当前候选序列，并限制异常输入占用的内存。
#[derive(Default)]
struct Osc7Scanner {
    candidate: Vec<u8>,
    pending_st: bool,
}

impl Osc7Scanner {
    const PREFIX: &'static [u8] = b"\x1b]7;";

    fn feed(&mut self, data: &[u8]) -> Vec<String> {
        let mut paths = Vec::new();
        for &byte in data {
            self.feed_byte(byte, &mut paths);
        }
        paths
    }

    fn feed_byte(&mut self, byte: u8, paths: &mut Vec<String>) {
        if self.pending_st {
            self.pending_st = false;
            if byte == b'\\' {
                self.finish(paths);
                return;
            }

            // OSC 载荷内出现非 ST 的 ESC，放弃旧候选，并把它当作新序列起点。
            self.candidate.clear();
            self.candidate.push(0x1b);
            self.feed_prefix_byte(byte);
            return;
        }

        if self.candidate.len() < Self::PREFIX.len() {
            self.feed_prefix_byte(byte);
            return;
        }

        match byte {
            0x07 => self.finish(paths),
            0x1b => self.pending_st = true,
            _ => {
                self.candidate.push(byte);
                if self.candidate.len() > OSC7_MAX_SEQUENCE_LEN {
                    self.reset();
                }
            }
        }
    }

    fn feed_prefix_byte(&mut self, byte: u8) {
        let expected = Self::PREFIX.get(self.candidate.len()).copied();
        if expected == Some(byte) {
            self.candidate.push(byte);
        } else if byte == Self::PREFIX[0] {
            self.candidate.clear();
            self.candidate.push(byte);
        } else {
            self.reset();
        }
    }

    fn finish(&mut self, paths: &mut Vec<String>) {
        if self.candidate.starts_with(Self::PREFIX) {
            let payload = &self.candidate[Self::PREFIX.len()..];
            if let Ok(payload) = std::str::from_utf8(payload) {
                if let Some(path) = osc7_payload_to_path(payload) {
                    paths.push(path);
                }
            }
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.candidate.clear();
        self.pending_st = false;
    }
}

#[cfg(test)]
fn parse_osc7_cwd(data: &[u8]) -> Option<String> {
    Osc7Scanner::default().feed(data).into_iter().next()
}

/// 把 OSC 7 的 `file://host/path` 载荷解析为本地路径，并做百分号解码。
fn osc7_payload_to_path(payload: &str) -> Option<String> {
    let rest = payload.strip_prefix("file://")?;
    // 去掉 host 部分（第一个 `/` 之前）。
    let path = match rest.find('/') {
        Some(pos) => &rest[pos..],
        None => return None,
    };
    let decoded = percent_decode(path);
    (!decoded.is_empty()).then_some(decoded)
}

/// 最小百分号解码（%XX → 字节），非法转义原样保留。
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 向 UI 转发终端事件，并返回必须写回远端 PTY 的响应字节。
async fn handle_term_events(
    id: SessionId,
    out: mpsc::Sender<FromCore>,
    events: Vec<TermEvent>,
) -> Vec<Vec<u8>> {
    let mut writes = Vec::new();
    for ev in events {
        match ev {
            TermEvent::Bell => {
                let _ = out.send(FromCore::Bell { id }).await;
            }
            TermEvent::Title(title) => {
                let _ = out.send(FromCore::Title { id, title }).await;
            }
            TermEvent::PtyWrite(data) => writes.push(data),
            TermEvent::Wakeup => {}
        }
    }
    writes
}

async fn write_pty_responses(shell: &SshShell, responses: Vec<Vec<u8>>) -> Result<(), SshError> {
    for data in responses {
        shell.write(&data).await?;
    }
    Ok(())
}

#[derive(Debug)]
enum OpenSshShellError {
    HostKeyPending(String),
    Failed(String),
}

async fn open_ssh_shell_with_timeout<T, F>(
    timeout: Duration,
    open_fut: F,
) -> std::result::Result<T, OpenSshShellError>
where
    F: Future<Output = std::result::Result<T, SshError>>,
{
    match tokio::time::timeout(timeout, open_fut).await {
        Ok(Ok(shell)) => Ok(shell),
        Ok(Err(SshError::HostKeyRejected)) => Err(OpenSshShellError::HostKeyPending(
            SshError::HostKeyRejected.to_string(),
        )),
        Ok(Err(err)) => Err(OpenSshShellError::Failed(err.to_string())),
        Err(_) => Err(OpenSshShellError::Failed(format!(
            "SSH 连接超时({} 秒)，连接流程未在限定时间内完成。请检查网络、ProxyJump、认证方式或远端 shell。",
            timeout.as_secs()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::{AcceptAllVerifier, AuthProvider};
    use crate::term::{Cursor, CursorShape, SnapshotCell};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn osc7_cwd_parsed_from_bel_terminated_sequence() {
        let seq = b"prefix\x1b]7;file://myhost/home/me/project\x07suffix";
        assert_eq!(parse_osc7_cwd(seq), Some("/home/me/project".to_string()));
    }

    #[test]
    fn osc7_cwd_parsed_from_st_terminated_and_percent_decoded() {
        let seq = b"\x1b]7;file://h/tmp/a%20b\x1b\\";
        assert_eq!(parse_osc7_cwd(seq), Some("/tmp/a b".to_string()));
    }

    #[test]
    fn osc7_cwd_absent_returns_none() {
        assert_eq!(parse_osc7_cwd(b"just terminal output\n"), None);
        assert_eq!(parse_osc7_cwd(b"\x1b]0;window title\x07"), None);
    }

    #[test]
    fn terminal_input_returns_scrolled_viewport_to_live_bottom() {
        let mut term = TermEngine::new(20, 3, 20);
        term.advance(b"line1\r\nline2\r\nline3\r\nline4\r\nline5");
        term.scroll(2);
        assert!(term.snapshot().display_offset > 0);

        assert!(prepare_terminal_for_input(&mut term, b"n"));
        assert_eq!(term.snapshot().display_offset, 0);
    }

    #[test]
    fn empty_terminal_input_keeps_scrollback_position() {
        let mut term = TermEngine::new(20, 3, 20);
        term.advance(b"line1\r\nline2\r\nline3\r\nline4\r\nline5");
        term.scroll(2);
        let before = term.snapshot();

        assert!(!prepare_terminal_for_input(&mut term, b""));
        let after = term.snapshot();
        assert_eq!(after.display_offset, before.display_offset);
        assert_eq!(after.revision, before.revision);
    }

    #[test]
    fn osc7_scanner_waits_for_complete_cross_chunk_sequence() {
        let mut scanner = Osc7Scanner::default();
        assert!(scanner.feed(b"prefix\x1b]7;file://host/home/").is_empty());
        assert!(scanner.feed(b"demo\x1b").is_empty());
        assert_eq!(scanner.feed(b"\\suffix"), vec!["/home/demo".to_string()]);
    }

    #[test]
    fn osc7_scanner_discards_oversized_sequence_and_recovers() {
        let mut scanner = Osc7Scanner::default();
        let mut oversized = b"\x1b]7;file://host/".to_vec();
        oversized.extend(std::iter::repeat_n(b'a', OSC7_MAX_SEQUENCE_LEN));
        oversized.push(0x07);
        assert!(scanner.feed(&oversized).is_empty());
        assert_eq!(
            scanner.feed(b"\x1b]7;file://host/recovered\x07"),
            vec!["/recovered".to_string()]
        );
    }

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

    struct PasswordAuth(&'static str);

    impl AuthProvider for PasswordAuth {
        fn password(&mut self, _user: &str, _host: &str, _port: u16) -> Option<String> {
            Some(self.0.to_string())
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

    fn test_snapshot(revision: u64) -> Box<GridSnapshot> {
        Box::new(GridSnapshot {
            rows: 1,
            cols: 1,
            cells: vec![SnapshotCell::default()],
            cursor: Cursor {
                line: 0,
                column: 0,
                shape: CursorShape::Block,
            },
            revision,
            display_offset: 0,
            wrapped: vec![false],
        })
    }

    #[test]
    fn interactive_auth_provider_uses_inner_password_without_prompt() {
        let (out_tx, mut out_rx) = mpsc::channel(4);
        let (_response_tx, response_rx) = std_mpsc::channel();
        let mut provider = InteractiveAuthProvider {
            id: SessionId(7),
            inner: Box::new(PasswordAuth("secret")),
            out: out_tx,
            responses: response_rx,
        };

        assert_eq!(
            provider.password("root", "example.com", 22),
            Some("secret".to_string())
        );
        assert!(out_rx.try_recv().is_err());
    }

    #[test]
    fn interactive_auth_provider_sends_keyboard_interactive_challenge() {
        let (out_tx, mut out_rx) = mpsc::channel(4);
        let (response_tx, response_rx) = std_mpsc::channel();
        let mut provider = InteractiveAuthProvider {
            id: SessionId(7),
            inner: Box::new(NoopAuth),
            out: out_tx,
            responses: response_rx,
        };

        let join = std::thread::spawn(move || {
            provider.keyboard_interactive(
                "otp",
                "Enter one-time code",
                &[("Code: ".to_string(), false)],
            )
        });

        match out_rx.blocking_recv() {
            Some(FromCore::AuthChallenge { id, challenge }) => {
                assert_eq!(id, SessionId(7));
                assert_eq!(
                    challenge,
                    AuthChallenge::KeyboardInteractive {
                        name: "otp".to_string(),
                        instructions: "Enter one-time code".to_string(),
                        prompts: vec![AuthPrompt {
                            text: "Code: ".to_string(),
                            echo: false,
                        }],
                    }
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }

        response_tx
            .send(AuthResponse::Answers(vec!["123456".to_string()]))
            .unwrap();
        assert_eq!(join.join().unwrap(), Some(vec!["123456".to_string()]));
    }

    #[test]
    fn concurrent_auth_challenges_leave_runtime_capacity_for_response_routing() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let (out_tx, mut out_rx) = mpsc::channel(4);
        let (route_tx, mut route_rx) = mpsc::channel::<(SessionId, AuthResponse)>(4);
        let (response_1_tx, response_1_rx) = std_mpsc::channel();
        let (response_2_tx, response_2_rx) = std_mpsc::channel();
        let response_1_cleanup = response_1_tx.clone();
        let response_2_cleanup = response_2_tx.clone();
        let (done_tx, done_rx) = std_mpsc::channel();

        runtime.spawn(async move {
            while let Some((id, response)) = route_rx.recv().await {
                let sender = if id == SessionId(1) {
                    &response_1_tx
                } else {
                    &response_2_tx
                };
                let _ = sender.send(response);
            }
        });

        for (id, responses) in [(SessionId(1), response_1_rx), (SessionId(2), response_2_rx)] {
            let out = out_tx.clone();
            let done = done_tx.clone();
            runtime.spawn(async move {
                let mut provider = InteractiveAuthProvider {
                    id,
                    inner: Box::new(NoopAuth),
                    out,
                    responses,
                };
                let answer = provider.password("root", "example.com", 22);
                let _ = done.send((id, answer));
            });
        }
        drop(out_tx);
        drop(done_tx);

        let mut challenged = Vec::new();
        for _ in 0..2 {
            match out_rx.blocking_recv() {
                Some(FromCore::AuthChallenge { id, .. }) => challenged.push(id),
                other => panic!("期望认证挑战，实际收到 {other:?}"),
            }
        }
        challenged.sort();
        assert_eq!(challenged, vec![SessionId(1), SessionId(2)]);

        for id in challenged {
            route_tx
                .blocking_send((id, AuthResponse::Answers(vec![format!("answer-{}", id.0)])))
                .unwrap();
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut answers = Vec::new();
        while answers.len() < 2 && Instant::now() < deadline {
            if let Ok(result) = done_rx.recv_timeout(Duration::from_millis(50)) {
                answers.push(result);
            }
        }
        if answers.len() != 2 {
            let _ = response_1_cleanup.send(AuthResponse::Cancel);
            let _ = response_2_cleanup.send(AuthResponse::Cancel);
            panic!("并发认证答案未能在 runtime 内完成路由");
        }
        answers.sort_by_key(|(id, _)| *id);
        assert_eq!(answers[0].1.as_deref(), Some("answer-1"));
        assert_eq!(answers[1].1.as_deref(), Some("answer-2"));
    }

    #[tokio::test]
    async fn terminal_pty_write_events_are_returned_for_shell_writeback() {
        let (out_tx, mut out_rx) = mpsc::channel(4);
        let writes = handle_term_events(
            SessionId(9),
            out_tx,
            vec![TermEvent::Bell, TermEvent::PtyWrite(b"\x1b[1;1R".to_vec())],
        )
        .await;

        assert_eq!(writes, vec![b"\x1b[1;1R".to_vec()]);
        assert!(matches!(
            out_rx.recv().await,
            Some(FromCore::Bell { id }) if id == SessionId(9)
        ));
    }

    #[test]
    fn to_core_debug_redacts_sensitive_payloads() {
        let input_debug = format!(
            "{:?}",
            ToCore::Input {
                id: SessionId(1),
                data: b"secret-input".to_vec(),
            }
        );
        assert!(input_debug.contains("bytes"));
        assert!(!input_debug.contains("secret-input"));

        let auth_debug = format!(
            "{:?}",
            ToCore::AuthResponse {
                id: SessionId(1),
                response: AuthResponse::Answers(vec!["secret-password".to_string()]),
            }
        );
        assert!(auth_debug.contains("redacted"));
        assert!(!auth_debug.contains("secret-password"));
    }

    #[test]
    fn try_recv_coalesces_render_events_per_session() {
        let (to_core_tx, _to_core_rx) = mpsc::channel(16);
        let (from_core_tx, from_core_rx) = mpsc::channel(16);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut manager = SessionManager {
            to_core_tx,
            from_core_rx,
            event_buffer: VecDeque::new(),
            pending_renders: HashMap::new(),
            _runtime: runtime,
        };

        from_core_tx
            .try_send(FromCore::Connected { id: SessionId(1) })
            .unwrap();
        from_core_tx
            .try_send(FromCore::Render {
                id: SessionId(1),
                snapshot: test_snapshot(1),
            })
            .unwrap();
        from_core_tx
            .try_send(FromCore::Render {
                id: SessionId(1),
                snapshot: test_snapshot(2),
            })
            .unwrap();
        from_core_tx
            .try_send(FromCore::Title {
                id: SessionId(1),
                title: "demo".to_string(),
            })
            .unwrap();
        from_core_tx
            .try_send(FromCore::Render {
                id: SessionId(1),
                snapshot: test_snapshot(3),
            })
            .unwrap();

        assert!(matches!(
            manager.try_recv(),
            Some(FromCore::Connected { id }) if id == SessionId(1)
        ));
        assert!(matches!(
            manager.try_recv(),
            Some(FromCore::Title { id, title }) if id == SessionId(1) && title == "demo"
        ));
        match manager.try_recv() {
            Some(FromCore::Render { id, snapshot }) => {
                assert_eq!(id, SessionId(1));
                assert_eq!(snapshot.revision, 3);
            }
            other => panic!("期望合并后的 Render，实际收到 {other:?}"),
        }
        assert!(manager.try_recv().is_none());
    }

    #[test]
    fn try_recv_coalesces_large_render_burst() {
        let (to_core_tx, _to_core_rx) = mpsc::channel(16);
        let (from_core_tx, from_core_rx) = mpsc::channel(1_100);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut manager = SessionManager {
            to_core_tx,
            from_core_rx,
            event_buffer: VecDeque::new(),
            pending_renders: HashMap::new(),
            _runtime: runtime,
        };

        for revision in 1..=1_000 {
            from_core_tx
                .try_send(FromCore::Render {
                    id: SessionId(1),
                    snapshot: test_snapshot(revision),
                })
                .unwrap();
        }
        from_core_tx
            .try_send(FromCore::Bell { id: SessionId(1) })
            .unwrap();

        assert!(matches!(
            manager.try_recv(),
            Some(FromCore::Bell { id }) if id == SessionId(1)
        ));
        match manager.try_recv() {
            Some(FromCore::Render { id, snapshot }) => {
                assert_eq!(id, SessionId(1));
                assert_eq!(snapshot.revision, 1_000);
            }
            other => panic!("期望合并后的高频 Render，实际收到 {other:?}"),
        }
        assert!(manager.try_recv().is_none());
    }

    #[test]
    fn send_reports_full_core_command_queue() {
        let (to_core_tx, _to_core_rx) = mpsc::channel(1);
        let (_from_core_tx, from_core_rx) = mpsc::channel(1);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let manager = SessionManager {
            to_core_tx,
            from_core_rx,
            event_buffer: VecDeque::new(),
            pending_renders: HashMap::new(),
            _runtime: runtime,
        };

        assert!(manager.send(ToCore::Disconnect { id: SessionId(1) }));
        assert!(!manager.send(ToCore::Disconnect { id: SessionId(2) }));
    }

    #[test]
    fn sftp_request_for_missing_session_returns_error() {
        let mut manager =
            SessionManager::spawn(Arc::new(AcceptAllVerifier), Arc::new(NoopFactory)).unwrap();

        manager.send(ToCore::Sftp {
            id: SessionId(404),
            request_id: SftpRequestId(77),
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
            FromCore::SftpError {
                id,
                request_id,
                message,
            } => {
                assert_eq!(id, SessionId(404));
                assert_eq!(request_id, SftpRequestId(77));
                assert!(message.contains("会话不存在"));
            }
            other => panic!("期望 SftpError，实际收到 {other:?}"),
        }
    }

    #[test]
    fn try_recv_keeps_late_sftp_listing_request_identity() {
        let (to_core_tx, _to_core_rx) = mpsc::channel(4);
        let (from_core_tx, from_core_rx) = mpsc::channel(4);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut manager = SessionManager {
            to_core_tx,
            from_core_rx,
            event_buffer: VecDeque::new(),
            pending_renders: HashMap::new(),
            _runtime: runtime,
        };

        from_core_tx
            .try_send(FromCore::SftpListing {
                id: SessionId(1),
                request_id: SftpRequestId(2),
                path: "/new".to_string(),
                entries: Vec::new(),
            })
            .unwrap();
        from_core_tx
            .try_send(FromCore::SftpListing {
                id: SessionId(1),
                request_id: SftpRequestId(1),
                path: "/old".to_string(),
                entries: Vec::new(),
            })
            .unwrap();

        assert!(matches!(
            manager.try_recv(),
            Some(FromCore::SftpListing {
                request_id: SftpRequestId(2),
                path,
                ..
            }) if path == "/new"
        ));
        assert!(matches!(
            manager.try_recv(),
            Some(FromCore::SftpListing {
                request_id: SftpRequestId(1),
                path,
                ..
            }) if path == "/old"
        ));
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
        let result: std::result::Result<(), OpenSshShellError> = open_ssh_shell_with_timeout(
            Duration::from_millis(1),
            std::future::pending::<std::result::Result<(), SshError>>(),
        )
        .await;

        let err = result.expect_err("pending 连接应当被超时打断");
        match err {
            OpenSshShellError::Failed(message) => assert!(message.contains("SSH 连接超时")),
            OpenSshShellError::HostKeyPending(message) => {
                panic!("不应进入主机密钥待确认分支: {message}")
            }
        }
    }
}
