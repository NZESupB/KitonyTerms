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
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex};
use tokio::task::JoinHandle;

use kt_config::ConnectParams;

use crate::monitor::MonitorStats;
use crate::remote_ops::{
    self, OperationId, OperationsDomain, OperationsError, OperationsErrorKind, OperationsRequest,
    OperationsResult,
};
use crate::shell_integration;
use crate::ssh::{AuthProvider, ChannelCloseGuard, HostKeyVerifier, PtySize, SshError, SshShell};
use crate::term::{GridSnapshot, TermEngine, TermEvent};

const AUTH_RESPONSE_TIMEOUT: Duration = Duration::from_secs(45);
const SFTP_REUSE_OPEN_TIMEOUT: Duration = Duration::from_secs(8);
const SFTP_STANDALONE_OPEN_TIMEOUT: Duration = Duration::from_secs(20);
const MONITOR_OPEN_TIMEOUT: Duration = Duration::from_secs(8);
const SSH_OPEN_TIMEOUT: Duration = Duration::from_secs(45);
const CONTAINER_IO_TIMEOUT: Duration = Duration::from_secs(5);
const EXEC_EVENT_TIMEOUT: Duration = Duration::from_secs(1);
const TO_CORE_CAPACITY: usize = 2_048;
const FROM_CORE_CAPACITY: usize = 2_048;
const OSC7_MAX_SEQUENCE_LEN: usize = 4 * 1024;

/// Opaque session identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub u64);

/// 一次 SFTP 请求的稳定标识，由调用方生成并随请求级事件原样返回。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SftpRequestId(pub u64);

/// 独立容器 PTY 的稳定标识。与宿主会话和运维查询 ID 相互隔离。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExecId(pub u64);

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
    /// 通过独立无 PTY exec 通道执行类型化的只读运维查询。
    Operations {
        id: SessionId,
        operation_id: OperationId,
        request: OperationsRequest,
    },
    /// 打开独立的 Docker 容器 PTY。
    OpenContainerTerminal {
        id: SessionId,
        exec_id: ExecId,
        container_id: String,
        pty: PtySize,
    },
    /// 向独立容器 PTY 写入输入。
    ContainerInput {
        id: SessionId,
        exec_id: ExecId,
        data: Vec<u8>,
    },
    /// 调整独立容器 PTY 尺寸。
    ContainerResize {
        id: SessionId,
        exec_id: ExecId,
        cols: u16,
        rows: u16,
    },
    /// 滚动独立容器终端视口。
    ContainerScroll {
        id: SessionId,
        exec_id: ExecId,
        delta: i32,
    },
    /// 关闭独立容器 PTY。
    CloseContainerTerminal { id: SessionId, exec_id: ExecId },
    /// 向远端交互 shell 注入一次工作目录上报 hook(每个连接只生效一次)。
    /// Inject the CWD-reporting hook into the remote interactive shell (once per connection).
    SetupShellIntegration { id: SessionId },
    /// Answer or cancel an authentication challenge.
    AuthResponse {
        id: SessionId,
        /// 认证挑战所属的连接代次；旧弹窗的答案不得进入新连接。
        generation: u64,
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
            ToCore::Operations {
                id,
                operation_id,
                request,
            } => f
                .debug_struct("Operations")
                .field("id", id)
                .field("operation_id", operation_id)
                .field("request", request)
                .finish(),
            ToCore::OpenContainerTerminal {
                id,
                exec_id,
                container_id,
                pty,
            } => f
                .debug_struct("OpenContainerTerminal")
                .field("id", id)
                .field("exec_id", exec_id)
                .field("container_id", container_id)
                .field("pty", pty)
                .finish(),
            ToCore::ContainerInput { id, exec_id, data } => f
                .debug_struct("ContainerInput")
                .field("id", id)
                .field("exec_id", exec_id)
                .field("bytes", &data.len())
                .finish(),
            ToCore::ContainerResize {
                id,
                exec_id,
                cols,
                rows,
            } => f
                .debug_struct("ContainerResize")
                .field("id", id)
                .field("exec_id", exec_id)
                .field("cols", cols)
                .field("rows", rows)
                .finish(),
            ToCore::ContainerScroll { id, exec_id, delta } => f
                .debug_struct("ContainerScroll")
                .field("id", id)
                .field("exec_id", exec_id)
                .field("delta", delta)
                .finish(),
            ToCore::CloseContainerTerminal { id, exec_id } => f
                .debug_struct("CloseContainerTerminal")
                .field("id", id)
                .field("exec_id", exec_id)
                .finish(),
            ToCore::SetupShellIntegration { id } => f
                .debug_struct("SetupShellIntegration")
                .field("id", id)
                .finish(),
            ToCore::AuthResponse {
                id,
                generation,
                response,
            } => f
                .debug_struct("AuthResponse")
                .field("id", id)
                .field("generation", generation)
                .field("response", response)
                .finish(),
            ToCore::Disconnect { id } => f.debug_struct("Disconnect").field("id", id).finish(),
        }
    }
}

/// Events emitted from the core out to the UI.
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
    /// 类型化运维查询成功。原始 stdout 不会作为事件载荷传播。
    OperationResult {
        id: SessionId,
        operation_id: OperationId,
        domain: OperationsDomain,
        result: OperationsResult,
    },
    /// 类型化运维查询失败。错误正文不包含远端日志、inspect 或 argv。
    OperationFailed {
        id: SessionId,
        operation_id: OperationId,
        domain: OperationsDomain,
        error: OperationsError,
    },
    /// 独立容器 PTY 已成功启动。
    ExecStarted {
        id: SessionId,
        exec_id: ExecId,
        container_id: String,
    },
    /// 独立容器 PTY 的渲染快照。
    ExecRender {
        id: SessionId,
        exec_id: ExecId,
        snapshot: Box<GridSnapshot>,
    },
    /// 独立容器 PTY 已关闭；错误正文不包含远端输出。
    ExecClosed {
        id: SessionId,
        exec_id: ExecId,
        error: Option<String>,
    },
    /// 需要 UI 提供认证输入。Authentication input is required from the UI.
    AuthChallenge {
        id: SessionId,
        /// 产生挑战的连接代次。
        generation: u64,
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

/// 事件调试输出只保留类别、标识和尺寸，禁止递归格式化终端/运维/SFTP payload。
impl fmt::Debug for FromCore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FromCore::Connected { id } => f.debug_struct("Connected").field("id", id).finish(),
            FromCore::Render { id, snapshot } => f
                .debug_struct("Render")
                .field("id", id)
                .field("revision", &snapshot.revision)
                .field("rows", &snapshot.rows)
                .field("cols", &snapshot.cols)
                .finish(),
            FromCore::Title { id, .. } => f.debug_struct("Title").field("id", id).finish(),
            FromCore::Cwd { id, .. } => f.debug_struct("Cwd").field("id", id).finish(),
            FromCore::Bell { id } => f.debug_struct("Bell").field("id", id).finish(),
            FromCore::SftpListing {
                id,
                request_id,
                entries,
                ..
            } => f
                .debug_struct("SftpListing")
                .field("id", id)
                .field("request_id", request_id)
                .field("count", &entries.len())
                .finish(),
            FromCore::SftpProgress {
                id,
                request_id,
                transferred,
                total,
                ..
            } => f
                .debug_struct("SftpProgress")
                .field("id", id)
                .field("request_id", request_id)
                .field("transferred", transferred)
                .field("total", total)
                .finish(),
            FromCore::SftpDone {
                id, request_id, op, ..
            } => f
                .debug_struct("SftpDone")
                .field("id", id)
                .field("request_id", request_id)
                .field("op", op)
                .finish(),
            FromCore::SftpError { id, request_id, .. } => f
                .debug_struct("SftpError")
                .field("id", id)
                .field("request_id", request_id)
                .finish(),
            FromCore::SftpStopped { id } => f.debug_struct("SftpStopped").field("id", id).finish(),
            FromCore::Monitor { id, stats } => f
                .debug_struct("Monitor")
                .field("id", id)
                .field("uptime_secs", &stats.uptime_secs)
                .finish(),
            FromCore::MonitorStopped { id } => {
                f.debug_struct("MonitorStopped").field("id", id).finish()
            }
            FromCore::MonitorError { id, .. } => {
                f.debug_struct("MonitorError").field("id", id).finish()
            }
            FromCore::OperationResult {
                id,
                operation_id,
                domain,
                ..
            } => f
                .debug_struct("OperationResult")
                .field("id", id)
                .field("operation_id", operation_id)
                .field("domain", domain)
                .finish(),
            FromCore::OperationFailed {
                id,
                operation_id,
                domain,
                error,
            } => f
                .debug_struct("OperationFailed")
                .field("id", id)
                .field("operation_id", operation_id)
                .field("domain", domain)
                .field("kind", &error.kind)
                .finish(),
            FromCore::ExecStarted {
                id,
                exec_id,
                container_id,
            } => f
                .debug_struct("ExecStarted")
                .field("id", id)
                .field("exec_id", exec_id)
                .field("container_id", container_id)
                .finish(),
            FromCore::ExecRender {
                id,
                exec_id,
                snapshot,
            } => f
                .debug_struct("ExecRender")
                .field("id", id)
                .field("exec_id", exec_id)
                .field("revision", &snapshot.revision)
                .field("rows", &snapshot.rows)
                .field("cols", &snapshot.cols)
                .finish(),
            FromCore::ExecClosed { id, exec_id, error } => f
                .debug_struct("ExecClosed")
                .field("id", id)
                .field("exec_id", exec_id)
                .field("has_error", &error.is_some())
                .finish(),
            FromCore::AuthChallenge {
                id,
                generation,
                challenge,
            } => {
                let (kind, prompt_count) = match challenge {
                    AuthChallenge::Password { .. } => ("password", None),
                    AuthChallenge::KeyPassphrase { .. } => ("key_passphrase", None),
                    AuthChallenge::KeyboardInteractive { prompts, .. } => {
                        ("keyboard_interactive", Some(prompts.len()))
                    }
                };
                let mut debug = f.debug_struct("AuthChallenge");
                debug
                    .field("id", id)
                    .field("generation", generation)
                    .field("kind", &kind);
                if let Some(prompt_count) = prompt_count {
                    debug.field("prompt_count", &prompt_count);
                }
                debug.finish()
            }
            FromCore::HostKeyPending { id } => {
                f.debug_struct("HostKeyPending").field("id", id).finish()
            }
            FromCore::Closed { id, error } => f
                .debug_struct("Closed")
                .field("id", id)
                .field("has_error", &error.is_some())
                .finish(),
        }
    }
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

/// Core 内部的会话事件信封。公共 `FromCore` 保持稳定，但事件在跨任务边界时
/// 必须携带连接代次，避免旧连接在同一 `SessionId` 重连后污染新状态。
#[derive(Debug)]
pub(crate) struct SessionEvent {
    pub(crate) id: SessionId,
    pub(crate) generation: u64,
    pub(crate) event: FromCore,
}

#[derive(Clone)]
pub(crate) struct SessionEventSender {
    id: SessionId,
    generation: u64,
    tx: mpsc::Sender<SessionEvent>,
}

impl SessionEventSender {
    pub(crate) fn new(id: SessionId, generation: u64, tx: mpsc::Sender<SessionEvent>) -> Self {
        Self { id, generation, tx }
    }

    pub(crate) async fn send(
        &self,
        event: FromCore,
    ) -> Result<(), mpsc::error::SendError<SessionEvent>> {
        self.tx
            .send(SessionEvent {
                id: self.id,
                generation: self.generation,
                event,
            })
            .await
    }

    pub(crate) fn try_send(
        &self,
        event: FromCore,
    ) -> Result<(), mpsc::error::TrySendError<SessionEvent>> {
        self.tx.try_send(SessionEvent {
            id: self.id,
            generation: self.generation,
            event,
        })
    }
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
        let (session_event_tx, session_event_rx) =
            mpsc::channel::<SessionEvent>(FROM_CORE_CAPACITY);

        runtime.spawn(core_loop(
            to_core_rx,
            from_core_tx,
            session_event_tx,
            session_event_rx,
            verifier,
            auth_factory,
        ));

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
        let kind = to_core_kind(&msg);
        match self.to_core_tx.try_send(msg) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_msg)) => {
                tracing::warn!(command = kind, "core 命令队列已满，丢弃命令");
                false
            }
            Err(mpsc::error::TrySendError::Closed(_msg)) => {
                tracing::warn!(command = kind, "core 命令队列已关闭，丢弃命令");
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

fn to_core_kind(command: &ToCore) -> &'static str {
    match command {
        ToCore::Connect { .. } => "connect",
        ToCore::Input { .. } => "input",
        ToCore::Resize { .. } => "resize",
        ToCore::Scroll { .. } => "scroll",
        ToCore::Sftp { .. } => "sftp",
        ToCore::StartMonitor { .. } => "start_monitor",
        ToCore::Operations { .. } => "operations",
        ToCore::OpenContainerTerminal { .. } => "open_container_terminal",
        ToCore::ContainerInput { .. } => "container_input",
        ToCore::ContainerResize { .. } => "container_resize",
        ToCore::ContainerScroll { .. } => "container_scroll",
        ToCore::CloseContainerTerminal { .. } => "close_container_terminal",
        ToCore::SetupShellIntegration { .. } => "setup_shell_integration",
        ToCore::AuthResponse { .. } => "auth_response",
        ToCore::Disconnect { .. } => "disconnect",
    }
}

/// Top-level core dispatch loop: routes [`ToCore`] commands to per-session tasks.
async fn core_loop(
    mut rx: mpsc::Receiver<ToCore>,
    tx: mpsc::Sender<FromCore>,
    session_event_tx: mpsc::Sender<SessionEvent>,
    mut session_event_rx: mpsc::Receiver<SessionEvent>,
    verifier: Arc<dyn HostKeyVerifier>,
    auth_factory: Arc<dyn AuthProviderFactory>,
) {
    // Per-session input/control senders.
    let mut sessions: HashMap<SessionId, SessionHandles> = HashMap::new();
    // 同一会话 ID 重连时，旧任务发出的事件不能污染新任务句柄。
    let mut next_generation: HashMap<SessionId, u64> = HashMap::new();

    loop {
        let cmd = tokio::select! {
            Some(event) = session_event_rx.recv() => {
                if session_generation_is_current(&sessions, event.id, event.generation) {
                    let closed = matches!(&event.event, FromCore::Closed { .. });
                    let _ = tx.send(event.event).await;
                    if closed {
                        sessions.remove(&event.id);
                    }
                }
                continue;
            }
            cmd = rx.recv() => cmd,
        };
        let Some(cmd) = cmd else {
            break;
        };
        match cmd {
            ToCore::Connect { id, params, pty } => {
                let (input_tx, input_rx) = mpsc::unbounded_channel::<SessionCmd>();
                let (auth_response_tx, auth_response_rx) = std_mpsc::channel::<AuthResponse>();
                let generation = next_generation.entry(id).or_insert(0);
                *generation = generation.saturating_add(1);
                let generation = *generation;
                let previous = sessions.insert(
                    id,
                    SessionHandles {
                        cmd_tx: input_tx,
                        auth_response_tx,
                        generation,
                    },
                );
                if let Some(previous) = previous {
                    let _ = previous.cmd_tx.send(SessionCmd::Disconnect);
                }

                let provider = auth_factory.create(id, &params);
                let provider = Box::new(InteractiveAuthProvider {
                    id,
                    inner: provider,
                    out: SessionEventSender::new(id, generation, session_event_tx.clone()),
                    responses: auth_response_rx,
                });
                let task = SessionTask {
                    id,
                    params: *params,
                    pty,
                    verifier: verifier.clone(),
                    provider,
                    out: SessionEventSender::new(id, generation, session_event_tx.clone()),
                    cmd_rx: input_rx,
                };
                tokio::spawn(async move {
                    task.run().await;
                });
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
                    let generation = h.generation;
                    if h.cmd_tx.send(SessionCmd::Sftp { request_id, req }).is_err() {
                        send_session_event(
                            &session_event_tx,
                            id,
                            generation,
                            FromCore::SftpError {
                                id,
                                request_id,
                                message: "SFTP 请求无法投递，会话任务已结束".to_string(),
                            },
                        )
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
                    let generation = h.generation;
                    if h.cmd_tx.send(SessionCmd::StartMonitor).is_err() {
                        send_session_event(
                            &session_event_tx,
                            id,
                            generation,
                            FromCore::MonitorError {
                                id,
                                message: "资源监控请求无法投递，会话任务已结束".to_string(),
                            },
                        )
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
            ToCore::Operations {
                id,
                operation_id,
                request,
            } => {
                let OperationsRequest::Refresh(domain) = request;
                if let Some(h) = sessions.get(&id) {
                    let generation = h.generation;
                    if h.cmd_tx
                        .send(SessionCmd::Operations {
                            operation_id,
                            request,
                        })
                        .is_err()
                    {
                        send_session_event(
                            &session_event_tx,
                            id,
                            generation,
                            FromCore::OperationFailed {
                                id,
                                operation_id,
                                domain,
                                error: OperationsError::new(
                                    OperationsErrorKind::Disconnected,
                                    "运维请求无法投递，会话任务已结束",
                                ),
                            },
                        )
                        .await;
                    }
                } else {
                    let _ = tx
                        .send(FromCore::OperationFailed {
                            id,
                            operation_id,
                            domain,
                            error: OperationsError::new(
                                OperationsErrorKind::Disconnected,
                                "运维请求失败：会话不存在或已关闭",
                            ),
                        })
                        .await;
                }
            }
            ToCore::OpenContainerTerminal {
                id,
                exec_id,
                container_id,
                pty,
            } => {
                if let Some(h) = sessions.get(&id) {
                    let generation = h.generation;
                    if h.cmd_tx
                        .send(SessionCmd::OpenContainerTerminal {
                            exec_id,
                            container_id,
                            pty,
                        })
                        .is_err()
                    {
                        send_session_event(
                            &session_event_tx,
                            id,
                            generation,
                            FromCore::ExecClosed {
                                id,
                                exec_id,
                                error: Some("容器终端请求无法投递，会话任务已结束".to_string()),
                            },
                        )
                        .await;
                    }
                } else {
                    let _ = tx
                        .send(FromCore::ExecClosed {
                            id,
                            exec_id,
                            error: Some("容器终端请求失败：会话不存在或已关闭".to_string()),
                        })
                        .await;
                }
            }
            ToCore::ContainerInput { id, exec_id, data } => {
                if let Some(h) = sessions.get(&id) {
                    let _ = h.cmd_tx.send(SessionCmd::ContainerInput { exec_id, data });
                }
            }
            ToCore::ContainerResize {
                id,
                exec_id,
                cols,
                rows,
            } => {
                if let Some(h) = sessions.get(&id) {
                    let _ = h.cmd_tx.send(SessionCmd::ContainerResize {
                        exec_id,
                        cols,
                        rows,
                    });
                }
            }
            ToCore::ContainerScroll { id, exec_id, delta } => {
                if let Some(h) = sessions.get(&id) {
                    let _ = h
                        .cmd_tx
                        .send(SessionCmd::ContainerScroll { exec_id, delta });
                }
            }
            ToCore::CloseContainerTerminal { id, exec_id } => {
                if let Some(h) = sessions.get(&id) {
                    let _ = h
                        .cmd_tx
                        .send(SessionCmd::CloseContainerTerminal { exec_id });
                }
            }
            // Shell 集成是尽力而为的增强：UI 侧本就有输入推断兜底，注入失败既不
            // 阻塞终端也不产生等待态，因此只记日志，不引入 FromCore 错误事件。
            ToCore::SetupShellIntegration { id } => {
                if let Some(h) = sessions.get(&id) {
                    if h.cmd_tx.send(SessionCmd::SetupShellIntegration).is_err() {
                        tracing::warn!("shell 集成注入无法投递，会话任务已结束: {:?}", id);
                    }
                } else {
                    tracing::debug!("忽略已关闭会话的 shell 集成注入请求: {:?}", id);
                }
            }
            ToCore::AuthResponse {
                id,
                generation,
                response,
            } => {
                if !route_auth_response(&sessions, id, generation, response) {
                    tracing::debug!(
                        "忽略旧连接的认证响应，会话 {:?}，generation={}",
                        id,
                        generation
                    );
                }
            }
            ToCore::Disconnect { id } => {
                // 保留句柄直到任务发出 `Closed`；此处提前移除会与终态事件竞态，
                // 导致正常断开对 UI 不可见。
                if let Some(h) = sessions.get(&id) {
                    let _ = h.cmd_tx.send(SessionCmd::Disconnect);
                }
            }
        }
    }
}

fn session_generation_is_current(
    sessions: &HashMap<SessionId, SessionHandles>,
    id: SessionId,
    generation: u64,
) -> bool {
    sessions
        .get(&id)
        .is_some_and(|session| session.generation == generation)
}

fn route_auth_response(
    sessions: &HashMap<SessionId, SessionHandles>,
    id: SessionId,
    generation: u64,
    response: AuthResponse,
) -> bool {
    sessions
        .get(&id)
        .filter(|session| session.generation == generation)
        .is_some_and(|session| session.auth_response_tx.send(response).is_ok())
}

async fn send_session_event(
    tx: &mpsc::Sender<SessionEvent>,
    id: SessionId,
    generation: u64,
    event: FromCore,
) {
    let _ = tx
        .send(SessionEvent {
            id,
            generation,
            event,
        })
        .await;
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
    Operations {
        operation_id: OperationId,
        request: OperationsRequest,
    },
    OpenContainerTerminal {
        exec_id: ExecId,
        container_id: String,
        pty: PtySize,
    },
    ContainerInput {
        exec_id: ExecId,
        data: Vec<u8>,
    },
    ContainerResize {
        exec_id: ExecId,
        cols: u16,
        rows: u16,
    },
    ContainerScroll {
        exec_id: ExecId,
        delta: i32,
    },
    CloseContainerTerminal {
        exec_id: ExecId,
    },
    SetupShellIntegration,
    Disconnect,
}

enum SessionInternal {
    MonitorExited(crate::monitor::MonitorExit),
    ContainerExecExited {
        exec_id: ExecId,
        error: Option<String>,
    },
}

struct SessionHandles {
    cmd_tx: mpsc::UnboundedSender<SessionCmd>,
    auth_response_tx: std_mpsc::Sender<AuthResponse>,
    generation: u64,
}

struct InteractiveAuthProvider {
    id: SessionId,
    inner: Box<dyn AuthProvider>,
    out: SessionEventSender,
    responses: std_mpsc::Receiver<AuthResponse>,
}

impl InteractiveAuthProvider {
    fn request_answers(&mut self, challenge: AuthChallenge) -> Option<Vec<String>> {
        if self
            .out
            .try_send(FromCore::AuthChallenge {
                id: self.id,
                generation: self.out.generation,
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
    out: SessionEventSender,
    cmd_rx: mpsc::UnboundedReceiver<SessionCmd>,
}

struct ContainerExecHandle {
    exec_id: ExecId,
    cmd_tx: mpsc::UnboundedSender<ContainerExecCmd>,
    cancel_tx: Option<oneshot::Sender<Option<String>>>,
    task: JoinHandle<()>,
    closed: Arc<AsyncMutex<bool>>,
}

struct SftpTaskHandle {
    tx: mpsc::UnboundedSender<(SftpRequestId, SftpRequest)>,
    task: JoinHandle<()>,
}

struct MonitorTaskHandle {
    cancel_tx: Option<oneshot::Sender<()>>,
    task: JoinHandle<crate::monitor::MonitorExit>,
}

struct OperationTaskHandle {
    cancel_tx: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

enum ContainerExecCmd {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Scroll(i32),
    Close(Option<String>),
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
        let mut sftp_handle: Option<SftpTaskHandle> = None;

        // 资源监控子任务是否已启动(惰性,首次请求时开启)。
        // Whether the monitor subtask has been started (lazy on first request).
        let mut monitor_started = false;
        let mut monitor_handle: Option<MonitorTaskHandle> = None;

        // 运维查询任务必须挂在宿主会话上，断开时统一取消，避免迟到结果继续占用
        // SSH channel 或在重连后污染新会话。
        let mut operation_tasks: Vec<OperationTaskHandle> = Vec::new();

        // 同一宿主会话最多显示一个容器 PTY；它拥有独立 channel、TermEngine 和命令队列。
        let mut container_exec: Option<ContainerExecHandle> = None;

        // Shell 集成 hook 每个连接只注入一次；过滤器只隐藏本次命令回显，不能吞掉
        // 同时到达的 MOTD、Last login 等正常登录输出。
        let mut shell_integration_sent = false;
        let mut bootstrap_filter = shell_integration::BootstrapOutputFilter::default();

        let (internal_tx, mut internal_rx) = mpsc::unbounded_channel::<SessionInternal>();

        loop {
            tokio::select! {
                internal = internal_rx.recv() => {
                    match internal {
                        Some(SessionInternal::MonitorExited(exit)) => {
                            monitor_started = false;
                            monitor_handle = None;
                            if matches!(
                                exit,
                                crate::monitor::MonitorExit::Stopped
                                    | crate::monitor::MonitorExit::ReceiverDropped
                            ) {
                                let _ = send_exec_event(
                                    &self.out,
                                    FromCore::MonitorStopped { id },
                                )
                                .await;
                            }
                        }
                        Some(SessionInternal::ContainerExecExited { exec_id, error })
                            if container_exec
                                .as_ref()
                                .is_some_and(|handle| handle.exec_id == exec_id) =>
                        {
                            // 任务退出时再给终态事件一次发送机会，覆盖首次发送遇到
                            // 短暂事件队列背压的情况；`closed` 保证成功后不会重复发送。
                            if let Some(closed) =
                                container_exec.as_ref().map(|handle| handle.closed.clone())
                            {
                                send_exec_closed_once(
                                    id,
                                    exec_id,
                                    error,
                                    &self.out,
                                    &closed,
                                )
                                .await;
                            }
                            container_exec = None;
                        }
                        Some(SessionInternal::ContainerExecExited { .. }) => {}
                        None => {}
                    }
                }

                // Control messages from the UI.
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(SessionCmd::Input(data)) => {
                            // 用户开始输入时必须马上恢复回显，并先冲刷过滤器里尚未
                            // 确认属于 bootstrap 的数据，避免用户输入前的输出丢失。
                            if !data.is_empty() && bootstrap_filter.is_active() {
                                let pending = bootstrap_filter.finish();
                                if !pending.is_empty() {
                                    term.advance(&pending);
                                    let writes = handle_term_events(
                                        self.id,
                                        self.out.clone(),
                                        term.take_events(),
                                    )
                                    .await;
                                    if let Err(error) = write_pty_responses(&shell, writes).await {
                                        close_error = Some(error.to_string());
                                        break;
                                    }
                                    self.emit_render(&term);
                                }
                            }
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
                            if sftp_handle.is_none() {
                                let primary_error = match tokio::time::timeout(
                                    SFTP_REUSE_OPEN_TIMEOUT,
                                    shell.open_sftp(),
                                )
                                .await
                                {
                                    Ok(Ok(session)) => {
                                        let (tx, rx) = mpsc::unbounded_channel();
                                        let task = tokio::spawn(crate::sftp::sftp_task(
                                            id,
                                            session,
                                            None,
                                            rx,
                                            self.out.clone(),
                                        ));
                                        sftp_handle = Some(SftpTaskHandle { tx, task });
                                        None
                                    }
                                    Ok(Err(e)) => Some(format!("复用当前 SSH 会话失败：{e}")),
                                    Err(_) => Some(format!(
                                        "复用当前 SSH 会话超时({} 秒)",
                                        SFTP_REUSE_OPEN_TIMEOUT.as_secs()
                                    )),
                                };

                                if sftp_handle.is_none() {
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
                                            let task = tokio::spawn(crate::sftp::sftp_task(
                                                id,
                                                session,
                                                Some(guard),
                                                rx,
                                                self.out.clone(),
                                            ));
                                            sftp_handle = Some(SftpTaskHandle { tx, task });
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
                            let send_failed = sftp_handle
                                .as_ref()
                                .is_some_and(|handle| handle.tx.send((request_id, req)).is_err());
                            if send_failed {
                                if let Some(handle) = sftp_handle.take() {
                                    shutdown_sftp_task(handle).await;
                                }
                                let _ = self.out.send(FromCore::SftpError {
                                    id,
                                    request_id,
                                    message: "SFTP 子任务已退出，请刷新后重试".to_string(),
                                }).await;
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
                                        let (cancel_tx, cancel_rx) = oneshot::channel();
                                        let latency_target =
                                            crate::monitor::LatencyProbeTarget::new(
                                                self.params.host.clone(),
                                                self.params.port,
                                            );
                                        let task = tokio::spawn(async move {
                                            let exit = crate::monitor::monitor_task(
                                                id,
                                                session,
                                                latency_target,
                                                out,
                                                cancel_rx,
                                            )
                                            .await;
                                            let _ = internal_tx.send(SessionInternal::MonitorExited(exit));
                                            exit
                                        });
                                        monitor_handle = Some(MonitorTaskHandle {
                                            cancel_tx: Some(cancel_tx),
                                            task,
                                        });
                                        monitor_started = true;
                                    }
                                    Ok(Err(e)) => {
                                        let _ = e;
                                        tracing::warn!("failed to start monitor");
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
                        Some(SessionCmd::Operations { operation_id, request }) => {
                            let OperationsRequest::Refresh(domain) = request;
                            let command = remote_ops::read_command_for_request(&request);
                            operation_tasks.retain(|task| !task.task.is_finished());
                            match tokio::time::timeout(
                                MONITOR_OPEN_TIMEOUT,
                                shell.open_exec_channel(command),
                            ).await {
                                Ok(Ok(channel)) => {
                                    let out = self.out.clone();
                                    let (cancel_tx, cancel_rx) = oneshot::channel();
                                    let task = tokio::spawn(async move {
                                        remote_ops::execute_readonly_with_cancel(
                                            id,
                                            operation_id,
                                            domain,
                                            channel,
                                            out,
                                            Some(cancel_rx),
                                        )
                                        .await;
                                    });
                                    operation_tasks.push(OperationTaskHandle {
                                        cancel_tx: Some(cancel_tx),
                                        task,
                                    });
                                }
                                Ok(Err(error)) => {
                                    let _ = self.out.send(FromCore::OperationFailed {
                                        id,
                                        operation_id,
                                        domain,
                                        error: OperationsError::new(
                                            OperationsErrorKind::Disconnected,
                                            format!("打开运维查询通道失败：{error}"),
                                        ),
                                    }).await;
                                }
                                Err(_) => {
                                    let _ = self.out.send(FromCore::OperationFailed {
                                        id,
                                        operation_id,
                                        domain,
                                        error: OperationsError::new(
                                            OperationsErrorKind::Timeout,
                                            "打开运维查询通道超时（8 秒）",
                                        ),
                                    }).await;
                                }
                            }
                        }
                        Some(SessionCmd::OpenContainerTerminal {
                            exec_id,
                            container_id,
                            pty,
                        }) => {
                            if let Some(previous) = container_exec.take() {
                                close_container_exec(
                                    id,
                                    previous,
                                    Some("容器终端已被新的终端替换".to_string()),
                                    &self.out,
                                )
                                .await;
                            }
                            match tokio::time::timeout(
                                MONITOR_OPEN_TIMEOUT,
                                shell.open_pty_exec_channel(&container_id, pty),
                            )
                            .await
                            {
                                Ok(Ok(channel)) => {
                                    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
                                    let (cancel_tx, cancel_rx) = oneshot::channel();
                                    let out = self.out.clone();
                                    let internal_tx = internal_tx.clone();
                                    let closed = Arc::new(AsyncMutex::new(false));
                                    let task_closed = closed.clone();
                                    let task = tokio::spawn(async move {
                                        let error = run_container_exec(
                                            id,
                                            exec_id,
                                            container_id,
                                            pty,
                                            channel,
                                            cmd_rx,
                                            cancel_rx,
                                            out,
                                            task_closed,
                                        )
                                        .await;
                                        let _ = internal_tx.send(
                                            SessionInternal::ContainerExecExited { exec_id, error },
                                        );
                                    });
                                    container_exec = Some(ContainerExecHandle {
                                        exec_id,
                                        cmd_tx,
                                        cancel_tx: Some(cancel_tx),
                                        task,
                                        closed,
                                    });
                                }
                                Ok(Err(error)) => {
                                    let _ = self.out.send(FromCore::ExecClosed {
                                        id,
                                        exec_id,
                                        error: Some(container_error_message(error)),
                                    }).await;
                                }
                                Err(_) => {
                                    let _ = self.out.send(FromCore::ExecClosed {
                                        id,
                                        exec_id,
                                        error: Some(format!(
                                            "打开容器终端通道超时（{} 秒）",
                                            MONITOR_OPEN_TIMEOUT.as_secs()
                                        )),
                                    }).await;
                                }
                            }
                        }
                        Some(SessionCmd::ContainerInput { exec_id, data }) => {
                            if let Some(handle) = container_exec.as_ref() {
                                if handle.exec_id == exec_id {
                                    let _ = handle.cmd_tx.send(ContainerExecCmd::Input(data));
                                }
                            }
                        }
                        Some(SessionCmd::ContainerResize { exec_id, cols, rows }) => {
                            if let Some(handle) = container_exec.as_ref() {
                                if handle.exec_id == exec_id {
                                    let _ = handle.cmd_tx.send(ContainerExecCmd::Resize { cols, rows });
                                }
                            }
                        }
                        Some(SessionCmd::ContainerScroll { exec_id, delta }) => {
                            if let Some(handle) = container_exec.as_ref() {
                                if handle.exec_id == exec_id {
                                    let _ = handle.cmd_tx.send(ContainerExecCmd::Scroll(delta));
                                }
                            }
                        }
                        Some(SessionCmd::CloseContainerTerminal { exec_id }) => {
                            if let Some(handle) = container_exec.as_ref() {
                                if handle.exec_id == exec_id {
                                    let _ = handle.cmd_tx.send(ContainerExecCmd::Close(None));
                                }
                            }
                        }
                        Some(SessionCmd::SetupShellIntegration) => {
                            if shell_integration_sent {
                                tracing::debug!("shell 集成已注入，跳过重复请求: {:?}", id);
                            } else {
                                shell_integration_sent = true;
                                bootstrap_filter.start(Instant::now());
                                if let Err(e) = shell
                                    .write(shell_integration::BOOTSTRAP_COMMAND.as_bytes())
                                    .await
                                {
                                    close_error = Some(e.to_string());
                                    break;
                                }
                            }
                        }
                        Some(SessionCmd::Disconnect) | None => {
                            if let Some(previous) = container_exec.take() {
                                close_container_exec(
                                    id,
                                    previous,
                                    None,
                                    &self.out,
                                )
                                .await;
                            }
                            cancel_operation_tasks(&mut operation_tasks).await;
                            let _ = shell.disconnect().await;
                            break;
                        }
                    }
                }

                // Output / channel events from the remote.
                msg = shell.next_message() => {
                    match msg {
                        Some(russh::ChannelMsg::Data { data }) => {
                            // OSC 7 始终解析：注入的 hook 上报的第一个目录也在静默期内。
                            for path in osc7_scanner.feed(&data) {
                                if last_cwd.as_deref() != Some(path.as_str()) {
                                    last_cwd = Some(path.clone());
                                    let _ = self.out.send(FromCore::Cwd { id, path }).await;
                                }
                            }
                            let data = bootstrap_filter.filter(&data, Instant::now());
                            if data.is_empty() {
                                continue;
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
                            // stderr 不参与 bootstrap 过滤，登录脚本的诊断信息也必须可见。
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

        if let Some(previous) = container_exec.take() {
            close_container_exec(id, previous, close_error.clone(), &self.out).await;
        }
        cancel_operation_tasks(&mut operation_tasks).await;
        if let Some(handle) = monitor_handle.take() {
            shutdown_monitor_task(handle).await;
        }
        if let Some(handle) = sftp_handle.take() {
            shutdown_sftp_task(handle).await;
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
    let path = &rest[rest.find('/')?..];
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
    out: SessionEventSender,
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

fn container_error_message(error: SshError) -> String {
    match error {
        SshError::Channel(_) => "容器终端通道不可用".to_string(),
        SshError::Russh(_) => "容器终端连接失败".to_string(),
        _ => "容器终端无法启动".to_string(),
    }
}

async fn cancel_operation_tasks(tasks: &mut Vec<OperationTaskHandle>) {
    for mut handle in tasks.drain(..) {
        if let Some(cancel_tx) = handle.cancel_tx.take() {
            let _ = cancel_tx.send(());
        }
        if tokio::time::timeout(Duration::from_secs(1), &mut handle.task)
            .await
            .is_err()
        {
            handle.task.abort();
            let _ = handle.task.await;
        }
    }
}

async fn shutdown_join_handle<T>(mut task: JoinHandle<T>) {
    if tokio::time::timeout(Duration::from_secs(1), &mut task)
        .await
        .is_err()
    {
        task.abort();
        let _ = task.await;
    }
}

async fn shutdown_sftp_task(handle: SftpTaskHandle) {
    drop(handle.tx);
    shutdown_join_handle(handle.task).await;
}

async fn shutdown_monitor_task(mut handle: MonitorTaskHandle) {
    if let Some(cancel_tx) = handle.cancel_tx.take() {
        let _ = cancel_tx.send(());
    }
    shutdown_join_handle(handle.task).await;
}

async fn close_container_exec(
    id: SessionId,
    mut handle: ContainerExecHandle,
    error: Option<String>,
    out: &SessionEventSender,
) {
    if let Some(cancel_tx) = handle.cancel_tx.take() {
        let _ = cancel_tx.send(error.clone());
    }
    let _ = handle.cmd_tx.send(ContainerExecCmd::Close(error.clone()));
    if tokio::time::timeout(Duration::from_secs(1), &mut handle.task)
        .await
        .is_err()
    {
        handle.task.abort();
        let _ = handle.task.await;
    }
    send_exec_closed_once(id, handle.exec_id, error, out, &handle.closed).await;
}

enum ContainerStartup {
    Started,
    Failed(Option<String>),
    Cancelled(Option<String>),
}

enum ContainerIoResult {
    Sent,
    Cancelled(Option<String>),
    Failed(String),
}

async fn send_exec_event(out: &SessionEventSender, event: FromCore) -> bool {
    match tokio::time::timeout(EXEC_EVENT_TIMEOUT, out.send(event)).await {
        Ok(Ok(())) => true,
        Ok(Err(_)) | Err(_) => false,
    }
}

async fn send_exec_closed_once(
    id: SessionId,
    exec_id: ExecId,
    error: Option<String>,
    out: &SessionEventSender,
    closed: &AsyncMutex<bool>,
) {
    let mut sent = closed.lock().await;
    if *sent {
        return;
    }
    if !send_exec_event(out, FromCore::ExecClosed { id, exec_id, error }).await {
        // Keep the state retryable. The session cleanup helper may get another
        // chance after a transiently full event queue or an aborted task.
        return;
    }
    *sent = true;
}

async fn container_data_with_cancel(
    channel: &ChannelCloseGuard,
    cancel_rx: &mut oneshot::Receiver<Option<String>>,
    data: Vec<u8>,
) -> ContainerIoResult {
    tokio::select! {
        cancellation = cancel_rx => ContainerIoResult::Cancelled(cancellation.unwrap_or(None)),
        result = tokio::time::timeout(CONTAINER_IO_TIMEOUT, channel.data_bytes(data)) => {
            match result {
                Ok(Ok(())) => ContainerIoResult::Sent,
                Ok(Err(error)) => ContainerIoResult::Failed(container_error_message(SshError::from(error))),
                Err(_) => ContainerIoResult::Failed("容器终端写入超时".to_string()),
            }
        }
    }
}

async fn container_window_change_with_cancel(
    channel: &ChannelCloseGuard,
    cancel_rx: &mut oneshot::Receiver<Option<String>>,
    cols: u16,
    rows: u16,
) -> ContainerIoResult {
    tokio::select! {
        cancellation = cancel_rx => ContainerIoResult::Cancelled(cancellation.unwrap_or(None)),
        result = tokio::time::timeout(
            CONTAINER_IO_TIMEOUT,
            channel.window_change(cols as u32, rows as u32, 0, 0),
        ) => {
            match result {
                Ok(Ok(())) => ContainerIoResult::Sent,
                Ok(Err(error)) => ContainerIoResult::Failed(container_error_message(SshError::from(error))),
                Err(_) => ContainerIoResult::Failed("容器终端调整大小超时".to_string()),
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_container_exec(
    id: SessionId,
    exec_id: ExecId,
    _container_id: String,
    pty: PtySize,
    channel: russh::Channel<russh::client::Msg>,
    mut cmd_rx: mpsc::UnboundedReceiver<ContainerExecCmd>,
    mut cancel_rx: oneshot::Receiver<Option<String>>,
    out: SessionEventSender,
    closed: Arc<AsyncMutex<bool>>,
) -> Option<String> {
    const SCROLLBACK: usize = 10_000;
    let mut term = TermEngine::new(pty.cols as usize, pty.rows as usize, SCROLLBACK);
    let mut close_error: Option<String> = None;
    let mut channel = ChannelCloseGuard::new(channel);
    let mut pending_commands = VecDeque::new();

    // request_pty 与 exec 都要求远端确认；同一通道上的前两个 Success 分别对应
    // PTY 和 exec。只有收到 exec 的确认后才对 UI 宣布 started，避免把被拒绝的
    // 容器请求误报为已启动。
    let mut request_successes = 0_u8;
    let startup = tokio::time::timeout(MONITOR_OPEN_TIMEOUT, async {
        loop {
            let message = tokio::select! {
                cancellation = &mut cancel_rx => return ContainerStartup::Cancelled(cancellation.unwrap_or(None)),
                command = cmd_rx.recv() => match command {
                    Some(ContainerExecCmd::Close(error)) => {
                        return ContainerStartup::Cancelled(error)
                    }
                    None => return ContainerStartup::Cancelled(None),
                    Some(command) => {
                        pending_commands.push_back(command);
                        continue;
                    }
                },
                message = channel.wait() => message,
            };
            match message {
                Some(russh::ChannelMsg::Success) => {
                    request_successes = request_successes.saturating_add(1);
                    if request_successes >= 2 {
                        let started = send_exec_event(
                            &out,
                            FromCore::ExecStarted {
                                id,
                                exec_id,
                                container_id: _container_id.clone(),
                            },
                        )
                        .await;
                        return if started {
                            ContainerStartup::Started
                        } else {
                            ContainerStartup::Failed(None)
                        };
                    }
                }
                Some(russh::ChannelMsg::Failure) => {
                    return ContainerStartup::Failed(Some("容器终端请求被远端拒绝".to_string()))
                }
                Some(russh::ChannelMsg::Data { data })
                | Some(russh::ChannelMsg::ExtendedData { data, .. }) => {
                    term.advance(&data);
                    let writes = match tokio::time::timeout(
                        EXEC_EVENT_TIMEOUT,
                        handle_term_events(id, out.clone(), term.take_events()),
                    )
                    .await
                    {
                        Ok(writes) => writes,
                        Err(_) => return ContainerStartup::Failed(None),
                    };
                    for data in writes {
                        match container_data_with_cancel(&channel, &mut cancel_rx, data).await {
                            ContainerIoResult::Sent => {}
                            ContainerIoResult::Cancelled(error) => {
                                return ContainerStartup::Cancelled(error)
                            }
                            ContainerIoResult::Failed(error) => {
                                return ContainerStartup::Failed(Some(error))
                            }
                        }
                    }
                }
                Some(russh::ChannelMsg::Eof) | Some(russh::ChannelMsg::Close) => {
                    return ContainerStartup::Failed(Some("容器终端已被远端关闭".to_string()))
                }
                None => return ContainerStartup::Failed(Some("容器终端通道已断开".to_string())),
                Some(_) => {}
            }
        }
    })
    .await;
    match startup {
        Ok(ContainerStartup::Started) => {}
        Ok(ContainerStartup::Failed(error)) | Ok(ContainerStartup::Cancelled(error)) => {
            channel.close().await;
            send_exec_closed_once(id, exec_id, error.clone(), &out, &closed).await;
            return error;
        }
        Err(_) => {
            let error = Some(format!(
                "容器终端启动超时（{} 秒）",
                MONITOR_OPEN_TIMEOUT.as_secs()
            ));
            channel.close().await;
            send_exec_closed_once(id, exec_id, error.clone(), &out, &closed).await;
            return error;
        }
    }
    if !send_exec_render(id, exec_id, &term, &out).await {
        let error = Some("容器终端初始渲染失败".to_string());
        channel.close().await;
        send_exec_closed_once(id, exec_id, error.clone(), &out, &closed).await;
        return error;
    }

    'run: loop {
        tokio::select! {
            cancellation = &mut cancel_rx => {
                close_error = cancellation.unwrap_or(None);
                break 'run;
            }
            command = async {
                if let Some(command) = pending_commands.pop_front() {
                    Some(command)
                } else {
                    cmd_rx.recv().await
                }
            } => {
                match command {
                    Some(ContainerExecCmd::Input(data)) => {
                        if data.is_empty() {
                            continue;
                        }
                        if term.scroll_to_bottom() && !send_exec_render(id, exec_id, &term, &out).await {
                            break 'run;
                        }
                        match container_data_with_cancel(&channel, &mut cancel_rx, data).await {
                            ContainerIoResult::Sent => {}
                            ContainerIoResult::Cancelled(error) => {
                                close_error = error;
                                break 'run;
                            }
                            ContainerIoResult::Failed(error) => {
                                close_error = Some(error);
                                break 'run;
                            }
                        }
                    }
                    Some(ContainerExecCmd::Resize { cols, rows }) => {
                        term.resize(cols as usize, rows as usize, SCROLLBACK);
                        match container_window_change_with_cancel(&channel, &mut cancel_rx, cols, rows).await {
                            ContainerIoResult::Sent => {}
                            ContainerIoResult::Cancelled(error) => {
                                close_error = error;
                                break 'run;
                            }
                            ContainerIoResult::Failed(error) => {
                                close_error = Some(error);
                                break 'run;
                            }
                        }
                        if !send_exec_render(id, exec_id, &term, &out).await {
                            break 'run;
                        }
                    }
                    Some(ContainerExecCmd::Scroll(delta)) => {
                        term.scroll(delta);
                        if !send_exec_render(id, exec_id, &term, &out).await {
                            break 'run;
                        }
                    }
                    Some(ContainerExecCmd::Close(error)) => {
                        close_error = error;
                        break 'run;
                    }
                    None => {
                        break 'run;
                    }
                }
            }
            message = channel.wait() => {
                match message {
                    Some(russh::ChannelMsg::Data { data })
                    | Some(russh::ChannelMsg::ExtendedData { data, .. }) => {
                        term.advance(&data);
                        let writes = match tokio::time::timeout(
                            EXEC_EVENT_TIMEOUT,
                            handle_term_events(id, out.clone(), term.take_events()),
                        )
                        .await
                        {
                            Ok(writes) => writes,
                            Err(_) => break 'run,
                        };
                        for data in writes {
                            match container_data_with_cancel(&channel, &mut cancel_rx, data).await {
                                ContainerIoResult::Sent => {}
                                ContainerIoResult::Cancelled(error) => {
                                    close_error = error;
                                    break 'run;
                                }
                                ContainerIoResult::Failed(error) => {
                                    close_error = Some(error);
                                    break 'run;
                                }
                            }
                        }
                        if close_error.is_some() || !send_exec_render(id, exec_id, &term, &out).await {
                            break 'run;
                        }
                    }
                    Some(russh::ChannelMsg::ExitStatus { exit_status }) => {
                        if exit_status != 0 {
                            close_error = Some("容器终端进程已退出".to_string());
                        }
                    }
                    // 与只读 exec 一样，exit-status 可能在 EOF 之后到达；继续
                    // drain 到 Close，确保非零退出不会被静默吞掉。
                    Some(russh::ChannelMsg::Eof) => continue,
                    Some(russh::ChannelMsg::Failure) => {
                        close_error = Some("容器终端请求被远端拒绝".to_string());
                        break 'run;
                    }
                    Some(russh::ChannelMsg::Close) | None => break 'run,
                    Some(_) => {}
                }
            }
        }
    }

    // 普通 Channel 被 drop 时不会自动发送 CHANNEL_CLOSE；运行阶段任何退出原因
    // （包括 UI 输出通道关闭、PTY 写入失败和远端主动关闭）都在这里统一收敛。
    channel.close().await;
    send_exec_closed_once(id, exec_id, close_error.clone(), &out, &closed).await;
    close_error
}

async fn send_exec_render(
    id: SessionId,
    exec_id: ExecId,
    term: &TermEngine,
    out: &SessionEventSender,
) -> bool {
    send_exec_event(
        out,
        FromCore::ExecRender {
            id,
            exec_id,
            snapshot: Box::new(term.snapshot()),
        },
    )
    .await
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

    /// `BOOTSTRAP_COMMAND` 注入的 hook 发出的正是省略 host 的 `file:///path`
    /// 形式。这条路径一旦解析不了，shell 集成整个方向就静默失效。
    #[test]
    fn osc7_cwd_parsed_from_the_empty_host_form_emitted_by_our_bootstrap() {
        assert_eq!(
            parse_osc7_cwd(b"\x1b]7;file:///tmp\x07"),
            Some("/tmp".to_string())
        );
        assert_eq!(
            parse_osc7_cwd(b"\x1b]7;file:///usr/local\x07"),
            Some("/usr/local".to_string())
        );
        assert_eq!(
            parse_osc7_cwd(b"\x1b]7;file:///\x07"),
            Some("/".to_string())
        );
        // 路径含空格时 shell 不做编码，载荷直到 BEL 才结束。
        assert_eq!(
            parse_osc7_cwd(b"\x1b]7;file:///srv/my logs\x07"),
            Some("/srv/my logs".to_string())
        );
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
            history_size: 0,
            wrapped: vec![false],
            alt_screen: false,
        })
    }

    #[test]
    fn interactive_auth_provider_uses_inner_password_without_prompt() {
        let (out_tx, mut out_rx) = mpsc::channel(4);
        let (_response_tx, response_rx) = std_mpsc::channel();
        let mut provider = InteractiveAuthProvider {
            id: SessionId(7),
            inner: Box::new(PasswordAuth("secret")),
            out: SessionEventSender::new(SessionId(7), 0, out_tx),
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
            out: SessionEventSender::new(SessionId(7), 0, out_tx),
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
            Some(SessionEvent {
                id,
                event: FromCore::AuthChallenge { challenge, .. },
                ..
            }) => {
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
        let (out_tx, mut out_rx) = mpsc::channel::<SessionEvent>(4);
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
                    out: SessionEventSender::new(id, 0, out),
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
                Some(SessionEvent {
                    id,
                    event: FromCore::AuthChallenge { .. },
                    ..
                }) => challenged.push(id),
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
            SessionEventSender::new(SessionId(9), 0, out_tx),
            vec![TermEvent::Bell, TermEvent::PtyWrite(b"\x1b[1;1R".to_vec())],
        )
        .await;

        assert_eq!(writes, vec![b"\x1b[1;1R".to_vec()]);
        assert!(matches!(
            out_rx.recv().await,
            Some(SessionEvent { id, event: FromCore::Bell { .. }, .. }) if id == SessionId(9)
        ));
    }

    #[tokio::test]
    async fn exec_closed_event_is_emitted_once() {
        let (out_tx, mut out_rx) = mpsc::channel(4);
        let out = SessionEventSender::new(SessionId(9), 0, out_tx);
        let closed = Arc::new(AsyncMutex::new(false));

        send_exec_closed_once(
            SessionId(9),
            ExecId(3),
            Some("closed".to_string()),
            &out,
            &closed,
        )
        .await;
        send_exec_closed_once(
            SessionId(9),
            ExecId(3),
            Some("duplicate".to_string()),
            &out,
            &closed,
        )
        .await;

        assert!(matches!(
            out_rx.recv().await,
            Some(SessionEvent {
                event: FromCore::ExecClosed {
                    id: SessionId(9),
                    exec_id: ExecId(3),
                    error: Some(error),
                },
                ..
            }) if error == "closed"
        ));
        assert!(out_rx.try_recv().is_err());
        assert!(*closed.lock().await);
    }

    #[tokio::test]
    async fn exec_closed_event_remains_retryable_when_receiver_is_closed() {
        let (out_tx, out_rx) = mpsc::channel(1);
        drop(out_rx);
        let out = SessionEventSender::new(SessionId(9), 0, out_tx);
        let closed = Arc::new(AsyncMutex::new(false));

        send_exec_closed_once(SessionId(9), ExecId(3), None, &out, &closed).await;

        assert!(!*closed.lock().await);
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
                generation: 1,
                response: AuthResponse::Answers(vec!["secret-password".to_string()]),
            }
        );
        assert!(auth_debug.contains("redacted"));
        assert!(!auth_debug.contains("secret-password"));
    }

    #[test]
    fn protocol_debug_does_not_include_remote_payloads() {
        let command_debug = format!(
            "{:?}",
            ToCore::Operations {
                id: SessionId(1),
                operation_id: OperationId(2),
                request: OperationsRequest::Refresh(OperationsDomain::Processes),
            }
        );
        assert!(!command_debug.contains("secret-command"));

        let snapshot = test_snapshot(7);
        let events = [
            FromCore::Render {
                id: SessionId(1),
                snapshot: snapshot.clone(),
            },
            FromCore::SftpListing {
                id: SessionId(1),
                request_id: SftpRequestId(3),
                path: "/private/remote/path".to_string(),
                entries: Vec::new(),
            },
            FromCore::SftpError {
                id: SessionId(1),
                request_id: SftpRequestId(3),
                message: "private sftp failure".to_string(),
            },
            FromCore::OperationResult {
                id: SessionId(1),
                operation_id: OperationId(2),
                domain: OperationsDomain::Processes,
                result: OperationsResult::Processes(
                    vec![crate::remote_ops::ProcessSummary {
                        pid: 7,
                        ppid: 1,
                        uid: 1000,
                        state: "S".to_string(),
                        cpu_percent: 1.0,
                        memory_percent: 2.0,
                        rss_kib: 3,
                        vsz_kib: 4,
                        elapsed: "00:01".to_string(),
                        command: "private process command".to_string(),
                    }]
                    .into(),
                ),
            },
            FromCore::ExecRender {
                id: SessionId(1),
                exec_id: ExecId(5),
                snapshot,
            },
            FromCore::ExecClosed {
                id: SessionId(1),
                exec_id: ExecId(5),
                error: Some("private close error".to_string()),
            },
            FromCore::AuthChallenge {
                id: SessionId(1),
                generation: 1,
                challenge: AuthChallenge::KeyboardInteractive {
                    name: "private auth name".to_string(),
                    instructions: "private auth instructions".to_string(),
                    prompts: vec![AuthPrompt {
                        text: "private prompt".to_string(),
                        echo: false,
                    }],
                },
            },
        ];
        for event in events {
            let debug = format!("{event:?}");
            assert!(!debug.contains("private"), "debug leaked payload: {debug}");
        }
    }

    #[test]
    fn connection_generation_only_matches_current_session() {
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let (auth_response_tx, auth_response_rx) = std_mpsc::channel();
        let mut sessions = HashMap::new();
        sessions.insert(
            SessionId(7),
            SessionHandles {
                cmd_tx,
                auth_response_tx,
                generation: 2,
            },
        );

        assert!(!session_generation_is_current(&sessions, SessionId(7), 1));
        assert!(session_generation_is_current(&sessions, SessionId(7), 2));
        assert!(!session_generation_is_current(&sessions, SessionId(8), 2));

        assert!(!route_auth_response(
            &sessions,
            SessionId(7),
            1,
            AuthResponse::Cancel,
        ));
        assert!(auth_response_rx.try_recv().is_err());
        assert!(route_auth_response(
            &sessions,
            SessionId(7),
            2,
            AuthResponse::Answers(vec!["current".to_string()]),
        ));
        assert!(matches!(
            auth_response_rx.try_recv(),
            Ok(AuthResponse::Answers(answers)) if answers == vec!["current"]
        ));
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
