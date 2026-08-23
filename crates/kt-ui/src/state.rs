//! 全局应用状态

use std::collections::{HashMap, VecDeque};

use kt_config::ConnectParams;
use kt_core::monitor::MonitorStats;
use kt_core::shell_integration;
use kt_core::term::GridSnapshot;
use kt_core::PtySize;
use kt_core::{
    AuthChallenge, FromCore, SessionId, SessionManager, SftpEntry, SftpOp, SftpRequest,
    SftpRequestId, ToCore,
};

const MAX_SFTP_OUTCOMES: usize = 256;
const MAX_TERMINAL_INPUT_OBSERVATION_BYTES: usize = 4096;
/// 终端目录栈的上限，防止异常输入让 `pushd` 推断无限增长。
const MAX_TERMINAL_DIR_STACK: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SftpCompletion {
    pub request_id: SftpRequestId,
    pub op: SftpOp,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SftpFailure {
    pub request_id: SftpRequestId,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SftpProgressState {
    pub request_id: SftpRequestId,
    pub name: String,
    pub transferred: u64,
    pub total: u64,
}

/// 单个会话的 UI 状态
#[derive(Clone)]
pub struct SessionState {
    pub id: SessionId,
    pub title: String,
    pub connect_params: ConnectParams,
    pub pty: PtySize,
    pub snapshot: Option<GridSnapshot>,
    pub connected: bool,
    /// 最近一次连接错误。None 表示仍在连接或已连接。
    pub connection_error: Option<String>,
    /// 主机密钥正在等待用户确认，不能作为普通连接失败展示。
    pub host_key_pending: bool,
    /// 当前等待用户输入的认证挑战。
    pub auth_challenge: Option<AuthChallenge>,

    // SFTP 状态
    pub sftp_path: String,
    pub sftp_entries: Vec<SftpEntry>,
    pub sftp_loading: bool,
    pub sftp_error: Option<String>,
    /// 当前目录列表请求；迟到的旧列表不得覆盖它。
    pub sftp_list_request_id: Option<SftpRequestId>,
    /// 有界保存近期请求结果，供外部编辑等异步状态机按 request ID 精确消费。
    pub sftp_completions: VecDeque<SftpCompletion>,
    pub sftp_failures: VecDeque<SftpFailure>,
    pub sftp_progress: Option<SftpProgressState>,
    /// 远端 shell 通过 OSC 7 上报或根据简单 `cd` 保守推断的当前工作目录。
    pub terminal_cwd: Option<String>,
    /// 最近一次由用户提交的简单 `cd` 目标。目标得到相同 OSC 7 确认前，
    /// 忽略输入前已经排队的旧目录事件，避免旧路径覆盖乐观推断。
    pub terminal_cwd_inference_target: Option<String>,
    /// 用户在终端实际提交的输入缓冲，仅用于识别保守的简单 `cd` 命令。
    pub terminal_input_buffer: Vec<u8>,
    /// 当前输入行包含编辑控制序列时，不对其进行目录推断。
    pub terminal_input_invalid: bool,
    /// 文件管理与终端目录是否保持双向自动同步。
    pub sftp_auto_sync: bool,
    /// 本次连接是否已向远端 shell 请求过目录上报 hook 注入。
    pub shell_integration_requested: bool,
    /// 远端 home 目录，由 SFTP 对 `.` 的 canonicalize 结果得到。
    /// 用于把 `cd`、`cd ~`、`cd ~/x` 解析成绝对路径。
    pub remote_home: Option<String>,
    /// 终端上一个工作目录，对应 shell 的 `OLDPWD`，用于推断 `cd -`。
    pub terminal_prev_cwd: Option<String>,
    /// 终端目录栈，用于推断 `pushd` / `popd`。栈顶不含当前目录。
    pub terminal_dir_stack: Vec<String>,
    /// 最近一次已用于文件管理跟随的终端目录，避免 canonicalize 后重复请求。
    pub sftp_followed_terminal_cwd: Option<String>,
    /// 当前文件管理目录请求成功后是否需要把终端切到 canonical 路径。
    pub sftp_terminal_sync_request: Option<(SftpRequestId, Option<String>)>,

    /// 最近一次资源监控采样。
    pub monitor: Option<MonitorStats>,
    /// 资源监控是否正在等待首次采样。
    pub monitor_loading: bool,
    /// 最近一次资源监控错误。
    pub monitor_error: Option<String>,
}

/// 全局应用状态（跨组件共享）
pub struct AppState {
    pub manager: SessionManager,
    pub sessions: HashMap<SessionId, SessionState>,
    pub next_id: u64,
    next_sftp_request_id: u64,
}

impl AppState {
    pub fn new(manager: SessionManager) -> Self {
        Self {
            manager,
            sessions: HashMap::new(),
            next_id: 1,
            next_sftp_request_id: 1,
        }
    }

    /// 创建新会话 ID
    pub fn next_session_id(&mut self) -> SessionId {
        let id = SessionId(self.next_id);
        self.next_id += 1;
        id
    }

    /// 分配请求 ID 并投递 SFTP 请求。投递失败时调用方不得进入 loading 状态。
    pub fn send_sftp_request(
        &mut self,
        session_id: SessionId,
        req: SftpRequest,
    ) -> Result<SftpRequestId, String> {
        if !self
            .sessions
            .get(&session_id)
            .is_some_and(|session| session.connected)
        {
            return Err("会话已断开，无法执行 SFTP 请求".to_string());
        }
        let request_id = SftpRequestId(self.next_sftp_request_id);
        self.next_sftp_request_id = self.next_sftp_request_id.saturating_add(1);
        if self.manager.send(ToCore::Sftp {
            id: session_id,
            request_id,
            req,
        }) {
            Ok(request_id)
        } else {
            Err("SFTP 请求无法投递，核心命令队列不可用".to_string())
        }
    }

    /// 重置易失会话状态并开始（或重新开始）连接，保留标签页与连接配置。
    pub fn connect_session(&mut self, id: SessionId) -> Result<(), String> {
        let (params, pty) = {
            let Some(session) = self.sessions.get_mut(&id) else {
                return Err("会话不存在，无法重新连接".to_string());
            };
            reset_connection_state(session);
            (session.connect_params.clone(), session.pty)
        };

        if self.manager.send(ToCore::Connect {
            id,
            params: Box::new(params),
            pty,
        }) {
            Ok(())
        } else {
            let message = "连接请求无法投递，核心命令队列不可用".to_string();
            if let Some(session) = self.sessions.get_mut(&id) {
                session.connection_error = Some(message.clone());
            }
            Err(message)
        }
    }

    pub fn set_sftp_auto_sync(&mut self, id: SessionId, enabled: bool) -> Result<(), String> {
        let follow_path = {
            let Some(session) = self.sessions.get_mut(&id) else {
                return Err("会话不存在，自动同步设置未生效".to_string());
            };
            session.sftp_auto_sync = enabled;
            session.sftp_terminal_sync_request = None;
            if enabled && session.connected && !session.sftp_loading {
                session.terminal_cwd.clone().filter(|path| {
                    path != &session.sftp_path
                        && session.sftp_followed_terminal_cwd.as_deref() != Some(path.as_str())
                })
            } else {
                None
            }
        };
        // 先让远端 shell 开始上报目录，再跟随已知目录，避免开关刚打开时空等一轮。
        self.request_shell_integration(id);
        if let Some(path) = follow_path {
            self.start_terminal_cwd_listing(id, path);
        }
        Ok(())
    }

    /// 请求向远端交互 shell 注入目录上报 hook。
    ///
    /// 只有已连接且自动同步开启的会话才注入，且每次连接最多一次。注入是尽力而为的
    /// 增强（UI 侧有输入推断兜底），投递失败只记日志，不写入会话错误状态。
    fn request_shell_integration(&mut self, id: SessionId) {
        let should_request = self.sessions.get(&id).is_some_and(|session| {
            session.connected && session.sftp_auto_sync && !session.shell_integration_requested
        });
        if !should_request {
            return;
        }
        if self.manager.send(ToCore::SetupShellIntegration { id }) {
            if let Some(session) = self.sessions.get_mut(&id) {
                session.shell_integration_requested = true;
            }
        } else {
            tracing::warn!("shell 集成注入请求无法投递，会话 {:?}", id);
        }
    }

    /// 投递用户实际输入，并在识别到简单 `cd` 后更新自动同步状态。
    ///
    /// 这里只观察已经要发送给远端的输入，不会改写输入或追加探测命令。
    pub fn send_terminal_input(&mut self, id: SessionId, data: Vec<u8>) -> bool {
        let observed_data = data.clone();
        let sent = self.manager.send(ToCore::Input { id, data });
        if sent {
            let inferred_path = self
                .sessions
                .get_mut(&id)
                .and_then(|session| session.observe_terminal_input(&observed_data));
            if let Some(path) = inferred_path {
                self.handle_inferred_terminal_cwd(id, path);
            }
        }
        sent
    }

    /// 处理来自 core 的事件
    pub fn pump_events(&mut self) {
        let mut event_count = 0;
        while let Some(ev) = self.manager.try_recv() {
            event_count += 1;
            tracing::debug!("收到事件: {:?}", ev);
            self.handle_event(ev);
        }
        if event_count > 0 {
            tracing::debug!("本轮处理了 {} 个事件", event_count);
        }
    }

    fn handle_event(&mut self, ev: FromCore) {
        match ev {
            FromCore::Connected { id } => {
                tracing::info!("会话 {:?} 已连接", id);
                let (sftp_path, should_start_monitor) =
                    if let Some(sess) = self.sessions.get_mut(&id) {
                        sess.connected = true;
                        sess.connection_error = None;
                        sess.host_key_pending = false;
                        sess.auth_challenge = None;
                        sess.monitor_loading = true;
                        sess.monitor_error = None;
                        if should_auto_load_sftp(sess) {
                            sess.sftp_loading = true;
                            sess.sftp_error = None;
                            (Some(sess.sftp_path.clone()), true)
                        } else {
                            (None, true)
                        }
                    } else {
                        (None, false)
                    };
                if should_start_monitor {
                    self.manager.send(ToCore::StartMonitor { id });
                }
                // 自动同步已开启的会话在连接就绪后立刻注入目录上报 hook。
                self.request_shell_integration(id);
                if let Some(path) = sftp_path {
                    match self.send_sftp_request(id, SftpRequest::List { path }) {
                        Ok(request_id) => {
                            if let Some(sess) = self.sessions.get_mut(&id) {
                                sess.sftp_list_request_id = Some(request_id);
                            }
                        }
                        Err(message) => {
                            if let Some(sess) = self.sessions.get_mut(&id) {
                                sess.sftp_loading = false;
                                sess.sftp_error = Some(message);
                            }
                        }
                    }
                }
            }
            FromCore::Render { id, snapshot } => {
                tracing::debug!(
                    "收到终端渲染数据，会话 {:?}，revision {}",
                    id,
                    snapshot.revision
                );
                if let Some(sess) = self.sessions.get_mut(&id) {
                    sess.snapshot = Some(*snapshot);
                }
            }
            FromCore::Title { id, title } => {
                tracing::debug!("忽略远端终端标题更新，会话 {:?}: {:?}", id, title);
            }
            FromCore::Cwd { id, path } => {
                let should_follow = if let Some(sess) = self.sessions.get_mut(&id) {
                    if let Some(expected_path) = sess.terminal_cwd_inference_target.as_deref() {
                        if expected_path != path {
                            tracing::debug!(
                                "忽略简单 cd 后迟到的旧终端目录事件，会话 {:?}: {}（等待 {}）",
                                id,
                                path,
                                expected_path
                            );
                            return;
                        }
                        sess.terminal_cwd_inference_target = None;
                    }
                    let waiting_for_file_manager_target =
                        sess.sftp_terminal_sync_request.is_some() && sess.sftp_auto_sync;
                    let is_stale_previous_cwd =
                        waiting_for_file_manager_target && sess.sftp_path != path;
                    if is_stale_previous_cwd {
                        tracing::debug!(
                            "忽略文件管理目录切换后迟到的终端目录事件，会话 {:?}: {}",
                            id,
                            path
                        );
                        false
                    } else {
                        sess.set_terminal_cwd(path.clone());
                        if waiting_for_file_manager_target {
                            sess.sftp_terminal_sync_request = None;
                            if sess.sftp_path == path {
                                sess.sftp_followed_terminal_cwd = Some(path.clone());
                            }
                        }
                        sess.sftp_auto_sync
                            && sess.connected
                            && !sess.sftp_loading
                            && sess.sftp_followed_terminal_cwd.as_deref() != Some(path.as_str())
                    }
                } else {
                    false
                };
                if should_follow {
                    self.start_terminal_cwd_listing(id, path);
                }
            }
            FromCore::Bell { .. } => {}
            FromCore::Closed { id, error } => {
                if let Some(sess) = self.sessions.get_mut(&id) {
                    sess.connected = false;
                    if sess.host_key_pending {
                        sess.connection_error = None;
                    } else {
                        sess.connection_error = Some(
                            error
                                .clone()
                                .unwrap_or_else(|| "SSH 会话已断开".to_string()),
                        );
                    }
                    sess.auth_challenge = None;
                    sess.snapshot = None;
                    sess.terminal_cwd = None;
                    sess.terminal_cwd_inference_target = None;
                    sess.terminal_prev_cwd = None;
                    sess.terminal_dir_stack.clear();
                    sess.shell_integration_requested = false;
                    sess.remote_home = None;
                    sess.sftp_followed_terminal_cwd = None;
                    sess.sftp_terminal_sync_request = None;
                    sess.monitor_loading = false;
                    sess.monitor_error = None;
                    sess.monitor = None;
                    sess.sftp_loading = false;
                    sess.sftp_list_request_id = None;
                    sess.sftp_progress = None;
                    sess.sftp_entries.clear();
                    if let Some(err) = error {
                        tracing::warn!("Session {} closed with error: {}", id.0, err);
                    }
                }
            }
            FromCore::SftpListing {
                id,
                request_id,
                path,
                entries,
            } => {
                tracing::info!(
                    "收到 SFTP 列表，会话 {:?}，路径 {}，{} 项",
                    id,
                    path,
                    entries.len()
                );
                let (sync_terminal_path, should_follow_latest) = if let Some(sess) =
                    self.sessions.get_mut(&id)
                {
                    if sess.sftp_list_request_id != Some(request_id) {
                        tracing::debug!(
                            "忽略迟到的 SFTP 列表，会话 {:?}，request={:?}",
                            id,
                            request_id
                        );
                        return;
                    }
                    let terminal_sync = sess
                        .sftp_terminal_sync_request
                        .take()
                        .filter(|(request, _)| *request == request_id);
                    // 首个列表请求发的是 `.`，core 侧 canonicalize 后的结果就是远端
                    // home，用来把 `cd`、`cd ~`、`cd ~/x` 解析成绝对路径。
                    if sess.remote_home.is_none() && sess.sftp_path == "." && path.starts_with('/')
                    {
                        sess.remote_home = Some(path.clone());
                    }
                    sess.sftp_path = path.clone();
                    sess.sftp_entries = entries;
                    sess.sftp_loading = false;
                    sess.sftp_error = None;
                    sess.sftp_list_request_id = None;
                    let sync_terminal_path = terminal_sync.and_then(|(_, terminal_before)| {
                        if sess.sftp_auto_sync
                            && sess.terminal_cwd == terminal_before
                            && sess.terminal_cwd.as_deref() != Some(path.as_str())
                        {
                            sess.sftp_terminal_sync_request = Some((request_id, terminal_before));
                            Some(path.clone())
                        } else {
                            if sess.terminal_cwd.as_deref() == Some(path.as_str()) {
                                sess.sftp_followed_terminal_cwd = Some(path.clone());
                            }
                            None
                        }
                    });
                    let should_follow_latest =
                        sync_terminal_path.is_none() && should_follow_terminal_cwd(sess);
                    (sync_terminal_path, should_follow_latest)
                } else {
                    (None, false)
                };
                if let Some(path) = sync_terminal_path {
                    self.sync_terminal_to_path(id, path);
                }
                if should_follow_latest {
                    self.start_latest_terminal_cwd_listing(id);
                }
            }
            FromCore::SftpError {
                id,
                request_id,
                message,
            } => {
                tracing::error!("SFTP 错误，会话 {:?}: {}", id, message);
                let should_follow_latest = if let Some(sess) = self.sessions.get_mut(&id) {
                    push_bounded(
                        &mut sess.sftp_failures,
                        SftpFailure {
                            request_id,
                            message: message.clone(),
                        },
                    );
                    let is_current_list = sess.sftp_list_request_id == Some(request_id);
                    let is_current_progress = sess
                        .sftp_progress
                        .as_ref()
                        .is_some_and(|progress| progress.request_id == request_id);
                    if is_current_list || sess.sftp_list_request_id.is_none() {
                        sess.sftp_loading = false;
                        sess.sftp_error = Some(message);
                    }
                    if is_current_list {
                        sess.sftp_list_request_id = None;
                        sess.sftp_terminal_sync_request = None;
                    }
                    if is_current_progress {
                        sess.sftp_progress = None;
                    }
                    is_current_list && should_follow_terminal_cwd(sess)
                } else {
                    false
                };
                if should_follow_latest {
                    self.start_latest_terminal_cwd_listing(id);
                }
            }
            FromCore::SftpStopped { id } => {
                if let Some(sess) = self.sessions.get_mut(&id) {
                    if !sess.connected {
                        sess.sftp_loading = false;
                        sess.sftp_list_request_id = None;
                        sess.sftp_progress = None;
                        sess.sftp_terminal_sync_request = None;
                    } else {
                        tracing::debug!("忽略已重连会话的迟到 SFTP 停止事件: {:?}", id);
                    }
                }
            }
            FromCore::SftpProgress {
                id,
                request_id,
                name,
                transferred,
                total,
            } => {
                if let Some(sess) = self.sessions.get_mut(&id) {
                    sess.sftp_progress = Some(SftpProgressState {
                        request_id,
                        name,
                        transferred,
                        total,
                    });
                }
            }
            FromCore::SftpDone {
                id,
                request_id,
                op,
                path,
            } => {
                tracing::info!("SFTP 操作完成，会话 {:?}: {:?} {}", id, op, path);
                if let Some(sess) = self.sessions.get_mut(&id) {
                    push_bounded(
                        &mut sess.sftp_completions,
                        SftpCompletion {
                            request_id,
                            op,
                            path: path.clone(),
                        },
                    );
                    if sess
                        .sftp_progress
                        .as_ref()
                        .is_some_and(|progress| progress.request_id == request_id)
                    {
                        sess.sftp_progress = None;
                    }
                }
                if should_refresh_after_sftp_op(op) {
                    let sftp_path = self.sessions.get(&id).map(|sess| sess.sftp_path.clone());
                    if let Some(path) = sftp_path {
                        match self.send_sftp_request(id, SftpRequest::List { path }) {
                            Ok(list_request_id) => {
                                if let Some(sess) = self.sessions.get_mut(&id) {
                                    sess.sftp_loading = true;
                                    sess.sftp_error = None;
                                    sess.sftp_list_request_id = Some(list_request_id);
                                }
                            }
                            Err(message) => {
                                if let Some(sess) = self.sessions.get_mut(&id) {
                                    sess.sftp_loading = false;
                                    sess.sftp_error = Some(message);
                                }
                            }
                        }
                    }
                }
            }
            FromCore::Monitor { id, stats } => {
                if let Some(sess) = self.sessions.get_mut(&id) {
                    sess.monitor = Some(*stats);
                    sess.monitor_loading = false;
                    sess.monitor_error = None;
                }
            }
            FromCore::MonitorStopped { id } => {
                if let Some(sess) = self.sessions.get_mut(&id) {
                    sess.monitor_loading = false;
                }
            }
            FromCore::MonitorError { id, message } => {
                tracing::error!("资源监控错误，会话 {:?}: {}", id, message);
                if let Some(sess) = self.sessions.get_mut(&id) {
                    sess.monitor_loading = false;
                    sess.monitor_error = Some(message);
                }
            }
            FromCore::AuthChallenge { id, challenge } => {
                if let Some(sess) = self.sessions.get_mut(&id) {
                    sess.auth_challenge = Some(challenge);
                    sess.connection_error = None;
                    sess.host_key_pending = false;
                }
            }
            FromCore::HostKeyPending { id } => {
                if let Some(sess) = self.sessions.get_mut(&id) {
                    sess.connected = false;
                    sess.connection_error = None;
                    sess.auth_challenge = None;
                    sess.host_key_pending = true;
                    sess.monitor_loading = false;
                    sess.monitor_error = None;
                    sess.sftp_loading = false;
                }
            }
        }
    }

    fn start_latest_terminal_cwd_listing(&mut self, id: SessionId) {
        let Some(path) = self
            .sessions
            .get(&id)
            .and_then(|session| session.terminal_cwd.clone())
        else {
            return;
        };
        self.start_terminal_cwd_listing(id, path);
    }

    fn start_terminal_cwd_listing(&mut self, id: SessionId, path: String) {
        self.start_sftp_listing(id, path);
        if let Some(session) = self.sessions.get_mut(&id) {
            if session.sftp_loading {
                session.sftp_followed_terminal_cwd = session.terminal_cwd.clone();
            }
        }
    }

    fn start_sftp_listing(&mut self, id: SessionId, path: String) {
        match self.send_sftp_request(id, SftpRequest::List { path: path.clone() }) {
            Ok(request_id) => {
                if let Some(session) = self.sessions.get_mut(&id) {
                    session.sftp_path = path;
                    session.sftp_loading = true;
                    session.sftp_error = None;
                    session.sftp_entries.clear();
                    session.sftp_list_request_id = Some(request_id);
                    session.sftp_terminal_sync_request = None;
                }
            }
            Err(message) => {
                if let Some(session) = self.sessions.get_mut(&id) {
                    session.sftp_loading = false;
                    session.sftp_error = Some(message);
                }
            }
        }
    }

    /// 把终端切换到指定目录。
    ///
    /// SSH 协议无法直接修改一个已在运行的交互 shell 的工作目录，只能向 PTY 写入，
    /// 所以这里是自动同步与手动同步按钮**共用的唯一写入点**：连接检查、备用屏保护、
    /// 路径转义与待确认目标的清理都收敛在这一处。
    pub fn send_terminal_cd(&mut self, id: SessionId, path: &str) -> Result<(), TerminalCdBlocked> {
        let Some(session) = self.sessions.get(&id) else {
            return Err(TerminalCdBlocked::Unavailable);
        };
        if !session.connected {
            return Err(TerminalCdBlocked::Unavailable);
        }
        // 终端在跑 vim/top/less 时，写进去的命令会被那个程序当按键消费：既改不了
        // 目录，还会破坏用户正在编辑的内容。
        if session
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.alt_screen)
        {
            return Err(TerminalCdBlocked::AltScreen);
        }
        let Some(command) = shell_integration::change_directory_command(path) else {
            return Err(TerminalCdBlocked::Unavailable);
        };

        // 这条 cd 是新的权威目标，不应继续等待上一次用户输入的推断路径确认，
        // 否则合法的 OSC 7 也会被误判成迟到事件。
        if let Some(session) = self.sessions.get_mut(&id) {
            session.terminal_cwd_inference_target = None;
        }
        if self.manager.send(ToCore::Input {
            id,
            data: command.into_bytes(),
        }) {
            Ok(())
        } else {
            Err(TerminalCdBlocked::SendFailed)
        }
    }

    fn sync_terminal_to_path(&mut self, id: SessionId, path: String) {
        match self.send_terminal_cd(id, &path) {
            Ok(()) => {}
            // 自动同步是后台行为：用户在全屏程序里浏览文件管理时静默跳过，
            // 不打断他，也不把这件事写成错误提示。
            Err(TerminalCdBlocked::AltScreen) => {
                tracing::debug!(
                    "终端正在运行全屏程序，跳过自动目录同步，会话 {:?}: {}",
                    id,
                    path
                );
                if let Some(session) = self.sessions.get_mut(&id) {
                    session.sftp_terminal_sync_request = None;
                }
            }
            Err(error) => {
                tracing::warn!("终端目录同步失败，会话 {:?}: {:?}", id, error);
                if let Some(session) = self.sessions.get_mut(&id) {
                    session.sftp_terminal_sync_request = None;
                }
            }
        }
    }

    fn handle_inferred_terminal_cwd(&mut self, id: SessionId, path: String) {
        let should_follow = if let Some(session) = self.sessions.get_mut(&id) {
            // `terminal_cwd` 与 OLDPWD 已由 `observe_terminal_input` 写入。
            session.terminal_cwd_inference_target = Some(path.clone());
            // 用户明确提交了新的 cd，优先于文件管理器此前等待的目标。
            session.sftp_terminal_sync_request = None;
            if session.sftp_path == path {
                session.sftp_followed_terminal_cwd = Some(path.clone());
                false
            } else {
                session.sftp_auto_sync
                    && session.connected
                    && !session.sftp_loading
                    && session.sftp_followed_terminal_cwd.as_deref() != Some(path.as_str())
            }
        } else {
            false
        };
        if should_follow {
            self.start_terminal_cwd_listing(id, path);
        }
    }

    pub(crate) fn finish_sftp_directory_request(
        &mut self,
        id: SessionId,
        request_id: SftpRequestId,
    ) {
        if let Some(session) = self.sessions.get_mut(&id) {
            if session
                .sftp_terminal_sync_request
                .as_ref()
                .is_some_and(|(request, _)| *request == request_id)
            {
                session.sftp_terminal_sync_request = None;
            }
        }
        if self
            .sessions
            .get(&id)
            .is_some_and(should_follow_terminal_cwd)
        {
            self.start_latest_terminal_cwd_listing(id);
        }
    }

    pub fn clear_host_key_pending_for(&mut self, host: &str, port: u16) -> usize {
        let mut cleared = 0;
        for sess in self.sessions.values_mut() {
            if sess.host_key_pending && session_matches_host_key_target(sess, host, port) {
                sess.host_key_pending = false;
                cleared += 1;
            }
        }
        cleared
    }

    pub fn reconnect_host_key_pending_for(&mut self, host: &str, port: u16) -> usize {
        let reconnects = self
            .sessions
            .values()
            .filter(|session| {
                session.host_key_pending && session_matches_host_key_target(session, host, port)
            })
            .map(|session| session.id)
            .collect::<Vec<_>>();

        reconnects
            .into_iter()
            .filter(|id| self.connect_session(*id).is_ok())
            .count()
    }
}

/// 向终端写入目录切换命令被拒绝的原因，由 UI 层按语言渲染。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalCdBlocked {
    /// 会话不存在、已断开，或目标路径为空。
    Unavailable,
    /// 终端正在运行 vim/top/less 等全屏程序（备用屏）。
    AltScreen,
    /// 核心命令队列不可用。
    SendFailed,
}

impl SessionState {
    /// 更新终端已知工作目录，并按 shell 的 `OLDPWD` 语义记录上一个目录。
    fn set_terminal_cwd(&mut self, path: String) {
        if self.terminal_cwd.as_deref() == Some(path.as_str()) {
            return;
        }
        if let Some(previous) = self.terminal_cwd.replace(path) {
            self.terminal_prev_cwd = Some(previous);
        }
    }

    fn observe_terminal_input(&mut self, data: &[u8]) -> Option<String> {
        let mut inferred_path = None;
        // 一次粘贴可能包含多条目录命令，因此当前目录与 OLDPWD 都要在批内滚动推进。
        let mut known_cwd = self.terminal_cwd.clone();
        let mut known_prev = self.terminal_prev_cwd.clone();

        // 任何新的用户输入都代表命令行时序已经向前推进；旧的推断目标不应
        // 阻塞后续命令（例如 cd 未触发 OSC 7 后再执行 pushd）。
        if !data.is_empty() {
            self.terminal_cwd_inference_target = None;
        }

        for &byte in data {
            match byte {
                b'\r' | b'\n' => {
                    let line = std::mem::take(&mut self.terminal_input_buffer);
                    let intent = (!self.terminal_input_invalid)
                        .then(|| parse_directory_intent(&line))
                        .flatten();
                    self.terminal_input_invalid = false;
                    let path = intent.and_then(|intent| {
                        self.apply_directory_intent(
                            intent,
                            known_cwd.as_deref(),
                            known_prev.as_deref(),
                        )
                    });
                    if let Some(path) = path {
                        known_prev = known_cwd.replace(path.clone());
                        inferred_path = Some(path);
                    }
                }
                0x7f if !self.terminal_input_invalid => {
                    remove_last_input_byte(&mut self.terminal_input_buffer)
                }
                0x03 | 0x15 => {
                    self.terminal_input_buffer.clear();
                    self.terminal_input_invalid = false;
                }
                0x1b | b'\t' => {
                    self.terminal_input_buffer.clear();
                    self.terminal_input_invalid = true;
                }
                byte if byte >= 0x20 && !self.terminal_input_invalid => {
                    if self.terminal_input_buffer.len() < MAX_TERMINAL_INPUT_OBSERVATION_BYTES {
                        self.terminal_input_buffer.push(byte);
                    } else {
                        self.terminal_input_buffer.clear();
                        self.terminal_input_invalid = true;
                    }
                }
                byte if byte >= 0x20 => {}
                _ => {
                    self.terminal_input_buffer.clear();
                    self.terminal_input_invalid = true;
                }
            }
        }

        // 批内滚动的结果要写回会话，`cd -` 与相对路径才能在多次输入之间连续推进。
        // 这里是输入推断这条路径上 `terminal_cwd` / `terminal_prev_cwd` 的唯一写点。
        if inferred_path.is_some() {
            self.terminal_cwd = known_cwd;
            self.terminal_prev_cwd = known_prev;
        }

        inferred_path
    }

    /// 把解析出的目录意图落成绝对路径，无法可靠推断时返回 `None`。
    ///
    /// `pushd` 只在目标真的能解析出来时才压栈，否则本地目录栈会与远端 shell 漂移。
    fn apply_directory_intent(
        &mut self,
        intent: DirectoryIntent,
        known_cwd: Option<&str>,
        known_prev: Option<&str>,
    ) -> Option<String> {
        let home = self.remote_home.clone();
        match intent {
            DirectoryIntent::Change(target) => {
                resolve_remote_cd_path(&target, known_cwd, home.as_deref())
            }
            DirectoryIntent::Home => home,
            DirectoryIntent::Previous => known_prev.map(str::to_string),
            DirectoryIntent::Push(target) => {
                let next = resolve_remote_cd_path(&target, known_cwd, home.as_deref())?;
                if let Some(current) = known_cwd {
                    if self.terminal_dir_stack.len() >= MAX_TERMINAL_DIR_STACK {
                        self.terminal_dir_stack.remove(0);
                    }
                    self.terminal_dir_stack.push(current.to_string());
                }
                Some(next)
            }
            DirectoryIntent::Pop => self.terminal_dir_stack.pop(),
        }
    }
}

/// 从用户输入行识别出的目录变更意图。只覆盖能够可靠推断的形式：别名、函数、
/// 脚本、子 shell、`~user`、失败的命令一律不猜。
#[derive(Clone, Debug, PartialEq, Eq)]
enum DirectoryIntent {
    /// `cd <target>` 或 `cd -- <target>`
    Change(String),
    /// `cd`（无参数），切到 home
    Home,
    /// `cd -`，切回 `OLDPWD`
    Previous,
    /// `pushd <target>`，压栈并切换
    Push(String),
    /// `popd`，弹栈并切回
    Pop,
}

fn parse_directory_intent(line: &[u8]) -> Option<DirectoryIntent> {
    let line = std::str::from_utf8(line).ok()?.trim();
    let mut words = parse_shell_words(line)?;
    if words.is_empty() {
        return None;
    }
    let command = words.remove(0);
    if words.first().map(String::as_str) == Some("--") {
        words.remove(0);
    }
    match (command.as_str(), words.len()) {
        ("cd", 0) => Some(DirectoryIntent::Home),
        ("cd", 1) if words[0] == "-" => Some(DirectoryIntent::Previous),
        ("cd", 1) => Some(DirectoryIntent::Change(words.remove(0))),
        // `pushd` 无参是交换栈顶，`popd` 带参是按索引弹出，都不做推断。
        ("pushd", 1) if words[0] != "-" => Some(DirectoryIntent::Push(words.remove(0))),
        ("popd", 0) => Some(DirectoryIntent::Pop),
        _ => None,
    }
}

fn remove_last_input_byte(buffer: &mut Vec<u8>) {
    let Some(mut byte) = buffer.pop() else {
        return;
    };
    while byte & 0xc0 == 0x80 {
        let Some(previous) = buffer.pop() else {
            break;
        };
        byte = previous;
    }
}

fn parse_shell_words(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut has_content = false;

    for ch in line.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            has_content = true;
            continue;
        }

        match quote {
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                } else {
                    current.push(ch);
                }
                has_content = true;
            }
            Some('"') => match ch {
                '"' => quote = None,
                '\\' => return None,
                '$' | '`' | '(' | ')' => return None,
                _ => current.push(ch),
            },
            Some(_) => return None,
            None => match ch {
                '\'' | '"' => {
                    quote = Some(ch);
                    has_content = true;
                }
                '\\' => {
                    escaped = true;
                    has_content = true;
                }
                ch if ch.is_whitespace() => {
                    if has_content {
                        words.push(std::mem::take(&mut current));
                        has_content = false;
                    }
                }
                ';' | '&' | '|' | '`' | '$' | '>' | '<' | '(' | ')' | '*' | '?' | '[' | ']'
                | '{' | '}' | '!' => return None,
                _ => {
                    current.push(ch);
                    has_content = true;
                }
            },
        }
    }

    if quote.is_some() || escaped {
        return None;
    }
    if has_content {
        words.push(current);
    }
    Some(words)
}

/// 把 `cd` 目标解析成远端绝对路径。
///
/// `~` 与 `~/x` 依赖由 SFTP canonicalize 得到的 `home`；`~user` 无法本地解析。
/// 相对路径依赖已知的当前目录，两者都必须是绝对路径，否则拒绝推断。
fn resolve_remote_cd_path(
    target: &str,
    current_cwd: Option<&str>,
    home: Option<&str>,
) -> Option<String> {
    if target.is_empty() || target == "-" {
        return None;
    }

    let absolute = if let Some(rest) = target.strip_prefix('~') {
        let home = home?;
        if rest.is_empty() {
            home.to_string()
        } else {
            // `~user` 形式无从得知目标 home，只接受 `~/...`。
            let rest = rest.strip_prefix('/')?;
            format!("{}/{rest}", home.trim_end_matches('/'))
        }
    } else if target.starts_with('/') {
        target.to_string()
    } else {
        format!("{}/{target}", current_cwd?.trim_end_matches('/'))
    };

    if !absolute.starts_with('/') {
        return None;
    }
    Some(normalize_remote_absolute_path(&absolute))
}

fn normalize_remote_absolute_path(path: &str) -> String {
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", parts.join("/"))
    }
}

fn reset_connection_state(session: &mut SessionState) {
    session.snapshot = None;
    session.connected = false;
    session.connection_error = None;
    session.host_key_pending = false;
    session.auth_challenge = None;
    session.sftp_path = ".".to_string();
    session.sftp_entries.clear();
    session.sftp_loading = false;
    session.sftp_error = None;
    session.sftp_list_request_id = None;
    session.sftp_completions.clear();
    session.sftp_failures.clear();
    session.sftp_progress = None;
    session.terminal_cwd = None;
    session.terminal_cwd_inference_target = None;
    session.terminal_prev_cwd = None;
    session.terminal_dir_stack.clear();
    session.terminal_input_buffer.clear();
    session.terminal_input_invalid = false;
    session.shell_integration_requested = false;
    session.remote_home = None;
    session.sftp_followed_terminal_cwd = None;
    session.sftp_terminal_sync_request = None;
    session.monitor = None;
    session.monitor_loading = false;
    session.monitor_error = None;
}

/// 全局状态包装器（用于 Dioxus Signal）
pub type GlobalState = std::sync::Arc<std::sync::Mutex<AppState>>;

fn push_bounded<T>(queue: &mut VecDeque<T>, value: T) {
    if queue.len() >= MAX_SFTP_OUTCOMES {
        queue.pop_front();
    }
    queue.push_back(value);
}

fn should_auto_load_sftp(sess: &SessionState) -> bool {
    sess.connected
        && !sess.sftp_loading
        && sess.sftp_entries.is_empty()
        && sess.sftp_error.is_none()
}

fn should_follow_terminal_cwd(sess: &SessionState) -> bool {
    sess.sftp_auto_sync
        && sess.connected
        && !sess.sftp_loading
        && sess
            .terminal_cwd
            .as_deref()
            .is_some_and(|path| path != sess.sftp_path)
        && sess
            .terminal_cwd
            .as_deref()
            .is_some_and(|path| sess.sftp_followed_terminal_cwd.as_deref() != Some(path))
}

fn should_refresh_after_sftp_op(op: SftpOp) -> bool {
    matches!(
        op,
        SftpOp::Upload | SftpOp::Mkdir | SftpOp::Remove | SftpOp::Rename
    )
}

fn session_matches_host_key_target(sess: &SessionState, host: &str, port: u16) -> bool {
    same_host_port(
        &sess.connect_params.host,
        sess.connect_params.port,
        host,
        port,
    ) || sess
        .connect_params
        .proxy_jump
        .as_deref()
        .and_then(parse_proxy_jump_host_port)
        .is_some_and(|(jump_host, jump_port)| same_host_port(&jump_host, jump_port, host, port))
}

fn same_host_port(left_host: &str, left_port: u16, right_host: &str, right_port: u16) -> bool {
    normalize_host(left_host).eq_ignore_ascii_case(normalize_host(right_host))
        && left_port == right_port
}

fn normalize_host(host: &str) -> &str {
    host.trim()
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or_else(|| host.trim())
}

fn parse_proxy_jump_host_port(value: &str) -> Option<(String, u16)> {
    let target = value
        .trim()
        .rsplit_once('@')
        .map_or(value.trim(), |(_, target)| target);
    if let Some(rest) = target.strip_prefix('[') {
        let (host, suffix) = rest.split_once(']')?;
        let port = suffix
            .strip_prefix(':')
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(22);
        return Some((host.to_string(), port));
    }

    if target.matches(':').count() == 1 {
        let (host, port) = target.rsplit_once(':')?;
        if let Ok(port) = port.parse::<u16>() {
            return Some((host.to_string(), port));
        }
    }
    (!target.is_empty()).then(|| (target.to_string(), 22))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kt_core::ssh::{AcceptAllVerifier, AuthProvider};
    use std::sync::Arc;

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

    impl kt_core::session::AuthProviderFactory for NoopFactory {
        fn create(
            &self,
            _id: SessionId,
            _params: &kt_config::ConnectParams,
        ) -> Box<dyn AuthProvider> {
            Box::new(NoopAuth)
        }
    }

    fn app_state() -> AppState {
        let manager =
            SessionManager::spawn(Arc::new(AcceptAllVerifier), Arc::new(NoopFactory)).unwrap();
        AppState::new(manager)
    }

    fn session_state(connected: bool) -> SessionState {
        SessionState {
            id: SessionId(1),
            title: "demo".to_string(),
            connect_params: ConnectParams::new("example.com", "root"),
            pty: PtySize {
                cols: 100,
                rows: 30,
            },
            snapshot: None,
            connected,
            connection_error: None,
            host_key_pending: false,
            auth_challenge: None,
            sftp_path: ".".to_string(),
            sftp_entries: Vec::new(),
            sftp_loading: false,
            sftp_error: None,
            sftp_list_request_id: None,
            sftp_completions: VecDeque::new(),
            sftp_failures: VecDeque::new(),
            sftp_progress: None,
            terminal_cwd: None,
            terminal_cwd_inference_target: None,
            terminal_prev_cwd: None,
            terminal_dir_stack: Vec::new(),
            terminal_input_buffer: Vec::new(),
            terminal_input_invalid: false,
            sftp_auto_sync: false,
            shell_integration_requested: false,
            remote_home: None,
            sftp_followed_terminal_cwd: None,
            sftp_terminal_sync_request: None,
            monitor: None,
            monitor_loading: false,
            monitor_error: None,
        }
    }

    fn pending_session(id: u64, host: &str, port: u16) -> SessionState {
        let mut session = session_state(false);
        session.id = SessionId(id);
        session.connect_params.host = host.to_string();
        session.connect_params.port = port;
        session.host_key_pending = true;
        session.connection_error = Some("旧错误".to_string());
        session.monitor_loading = true;
        session
    }

    #[test]
    fn auto_sftp_load_only_when_connected_and_idle() {
        let mut sess = session_state(false);
        assert!(!should_auto_load_sftp(&sess));

        sess.connected = true;
        assert!(should_auto_load_sftp(&sess));

        sess.sftp_loading = true;
        assert!(!should_auto_load_sftp(&sess));
    }

    #[test]
    fn shell_integration_is_requested_once_per_connection() {
        let mut app_state = app_state();
        let mut sess = session_state(false);
        sess.sftp_auto_sync = true;
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        // 未连接时不注入。
        app_state.request_shell_integration(id);
        assert!(!app_state.sessions[&id].shell_integration_requested);

        app_state.handle_event(FromCore::Connected { id });
        assert!(
            app_state.sessions[&id].shell_integration_requested,
            "连接就绪且自动同步已开时应立刻注入"
        );

        // 同一次连接内重复触发（例如用户来回切开关）不得重复注入。
        app_state.sessions.get_mut(&id).unwrap().sftp_auto_sync = false;
        app_state.set_sftp_auto_sync(id, true).unwrap();
        assert!(app_state.sessions[&id].shell_integration_requested);

        // 重连会得到一个全新的远端 shell，必须允许再次注入。
        let _ = app_state.connect_session(id);
        assert!(!app_state.sessions[&id].shell_integration_requested);
        assert!(app_state.sessions[&id].remote_home.is_none());
        assert!(app_state.sessions[&id].terminal_dir_stack.is_empty());
    }

    #[test]
    fn shell_integration_is_not_requested_while_auto_sync_is_off() {
        let mut app_state = app_state();
        let sess = session_state(false);
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.handle_event(FromCore::Connected { id });
        assert!(!app_state.sessions[&id].shell_integration_requested);

        // 用户打开开关时补注入。
        app_state.set_sftp_auto_sync(id, true).unwrap();
        assert!(app_state.sessions[&id].shell_integration_requested);
    }

    #[test]
    fn remote_home_is_captured_from_the_first_canonicalized_listing() {
        let mut app_state = app_state();
        let mut sess = session_state(true);
        sess.sftp_list_request_id = Some(SftpRequestId(1));
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        // 首个列表请求发的是 `.`，core canonicalize 后的结果就是远端 home。
        app_state.handle_event(FromCore::SftpListing {
            id,
            request_id: SftpRequestId(1),
            path: "/home/demo".to_string(),
            entries: Vec::new(),
        });
        assert_eq!(
            app_state.sessions[&id].remote_home.as_deref(),
            Some("/home/demo")
        );

        // 之后进入其他目录不得覆盖 home。
        app_state
            .sessions
            .get_mut(&id)
            .unwrap()
            .sftp_list_request_id = Some(SftpRequestId(2));
        app_state.handle_event(FromCore::SftpListing {
            id,
            request_id: SftpRequestId(2),
            path: "/var/log".to_string(),
            entries: Vec::new(),
        });
        assert_eq!(
            app_state.sessions[&id].remote_home.as_deref(),
            Some("/home/demo")
        );
    }

    fn alt_screen_snapshot() -> GridSnapshot {
        let mut engine = kt_core::TermEngine::new(20, 4, 20);
        engine.advance(b"\x1b[?1049h");
        let snapshot = engine.snapshot();
        assert!(snapshot.alt_screen, "构造的快照应处于备用屏");
        snapshot
    }

    #[test]
    fn terminal_cd_is_refused_while_a_full_screen_program_runs() {
        let mut app_state = app_state();
        let mut sess = session_state(true);
        sess.snapshot = Some(alt_screen_snapshot());
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        assert_eq!(
            app_state.send_terminal_cd(id, "/var/log"),
            Err(TerminalCdBlocked::AltScreen),
            "vim/top 运行时写入的命令会被那个程序吃掉，必须拒绝"
        );
    }

    #[test]
    fn terminal_cd_requires_a_connected_session_and_non_empty_path() {
        let mut app_state = app_state();
        let sess = session_state(false);
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        assert_eq!(
            app_state.send_terminal_cd(id, "/var/log"),
            Err(TerminalCdBlocked::Unavailable)
        );
        assert_eq!(
            app_state.send_terminal_cd(SessionId(99), "/var/log"),
            Err(TerminalCdBlocked::Unavailable)
        );

        app_state.sessions.get_mut(&id).unwrap().connected = true;
        assert_eq!(
            app_state.send_terminal_cd(id, "   "),
            Err(TerminalCdBlocked::Unavailable)
        );
        assert!(app_state.send_terminal_cd(id, "/var/log").is_ok());
    }

    #[test]
    fn auto_sync_skips_terminal_cd_silently_on_alternate_screen() {
        let mut app_state = app_state();
        let mut sess = session_state(true);
        sess.sftp_auto_sync = true;
        sess.snapshot = Some(alt_screen_snapshot());
        sess.sftp_terminal_sync_request = Some((SftpRequestId(3), None));
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.sync_terminal_to_path(id, "/var/log".to_string());

        let sess = &app_state.sessions[&id];
        // 后台跟随不该打断正在用 vim 的用户，所以不写错误提示，只清理等待态。
        assert!(sess.sftp_error.is_none());
        assert!(sess.sftp_terminal_sync_request.is_none());
    }

    #[test]
    fn terminal_cd_clears_the_pending_inference_target() {
        let mut app_state = app_state();
        let mut sess = session_state(true);
        sess.terminal_cwd_inference_target = Some("/home".to_string());
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.send_terminal_cd(id, "/var/log").unwrap();

        // 这条 cd 是新的权威目标，否则它自己触发的 OSC 7 会被当成迟到事件丢掉。
        assert!(app_state.sessions[&id]
            .terminal_cwd_inference_target
            .is_none());
    }

    #[test]
    fn mutating_sftp_ops_refresh_listing() {
        assert!(should_refresh_after_sftp_op(SftpOp::Mkdir));
        assert!(should_refresh_after_sftp_op(SftpOp::Remove));
        assert!(should_refresh_after_sftp_op(SftpOp::Rename));
        assert!(should_refresh_after_sftp_op(SftpOp::Upload));
        assert!(!should_refresh_after_sftp_op(SftpOp::Download));
    }

    #[test]
    fn monitor_error_clears_loading_and_records_message() {
        let mut app_state = app_state();
        let mut sess = session_state(true);
        sess.monitor_loading = true;
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.handle_event(FromCore::MonitorError {
            id,
            message: "启动失败".to_string(),
        });

        let sess = app_state.sessions.get(&id).unwrap();
        assert!(!sess.monitor_loading);
        assert_eq!(sess.monitor_error.as_deref(), Some("启动失败"));
    }

    #[test]
    fn monitor_sample_clears_error_and_loading() {
        let mut app_state = app_state();
        let mut sess = session_state(true);
        sess.monitor_loading = true;
        sess.monitor_error = Some("旧错误".to_string());
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.handle_event(FromCore::Monitor {
            id,
            stats: Box::new(MonitorStats {
                cpu_percent: 12.5,
                ..MonitorStats::default()
            }),
        });

        let sess = app_state.sessions.get(&id).unwrap();
        assert!(!sess.monitor_loading);
        assert!(sess.monitor_error.is_none());
        assert_eq!(sess.monitor.as_ref().unwrap().cpu_percent, 12.5);
    }

    #[test]
    fn clean_session_close_clears_runtime_state_and_is_disconnected() {
        let mut app_state = app_state();
        let mut sess = session_state(true);
        sess.monitor_loading = true;
        sess.monitor_error = Some("资源监控通道已关闭".to_string());
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.handle_event(FromCore::Closed { id, error: None });

        let sess = app_state.sessions.get(&id).unwrap();
        assert!(!sess.connected);
        assert_eq!(sess.connection_error.as_deref(), Some("SSH 会话已断开"));
        assert!(!sess.monitor_loading);
        assert!(sess.monitor_error.is_none());
        assert!(sess.monitor.is_none());
        assert!(sess.snapshot.is_none());
        assert!(sess.terminal_cwd.is_none());
    }

    #[test]
    fn session_close_records_connection_error() {
        let mut app_state = app_state();
        let sess = session_state(false);
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.handle_event(FromCore::Closed {
            id,
            error: Some("authentication failed".to_string()),
        });

        let sess = app_state.sessions.get(&id).unwrap();
        assert!(!sess.connected);
        assert_eq!(
            sess.connection_error.as_deref(),
            Some("authentication failed")
        );
    }

    #[test]
    fn host_key_pending_suppresses_connection_error_until_user_decides() {
        let mut app_state = app_state();
        let sess = session_state(false);
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.handle_event(FromCore::HostKeyPending { id });
        app_state.handle_event(FromCore::Closed {
            id,
            error: Some("host key rejected by user".to_string()),
        });

        let sess = app_state.sessions.get(&id).unwrap();
        assert!(!sess.connected);
        assert!(sess.host_key_pending);
        assert!(sess.connection_error.is_none());

        app_state.clear_host_key_pending_for("example.com", 22);
        assert!(!app_state.sessions[&id].host_key_pending);
    }

    #[test]
    fn reconnect_host_key_pending_for_restarts_matching_connection() {
        let mut app_state = app_state();
        let sess = pending_session(1, "example.com", 22);
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        assert_eq!(
            app_state.reconnect_host_key_pending_for("example.com", 22),
            1
        );

        let sess = app_state.sessions.get(&id).unwrap();
        assert!(!sess.host_key_pending);
        assert!(sess.connection_error.is_none());
        assert!(!sess.monitor_loading);
    }

    #[test]
    fn reconnect_resets_stale_terminal_and_sftp_state() {
        let mut app_state = app_state();
        let mut sess = session_state(false);
        sess.connection_error = Some("连接已断开".to_string());
        sess.sftp_path = "/var/log".to_string();
        sess.sftp_entries.push(SftpEntry {
            name: "old.log".to_string(),
            is_dir: false,
            size: 1,
            modified: None,
            permissions: None,
            user: None,
            group: None,
            uid: None,
            gid: None,
        });
        sess.terminal_cwd = Some("/var/log".to_string());
        sess.sftp_auto_sync = true;
        sess.monitor = Some(MonitorStats::default());
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        assert!(app_state.connect_session(id).is_ok());
        let sess = app_state.sessions.get(&id).unwrap();
        assert!(sess.snapshot.is_none());
        assert!(sess.connection_error.is_none());
        assert_eq!(sess.sftp_path, ".");
        assert!(sess.sftp_entries.is_empty());
        assert!(sess.terminal_cwd.is_none());
        assert!(sess.sftp_auto_sync);
        assert!(sess.monitor.is_none());
    }

    #[test]
    fn terminal_cwd_event_refreshes_sftp_when_auto_sync_is_enabled() {
        let mut app_state = app_state();
        let mut sess = session_state(true);
        sess.sftp_auto_sync = true;
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.handle_event(FromCore::Cwd {
            id,
            path: "/srv/app".to_string(),
        });

        let sess = app_state.sessions.get(&id).unwrap();
        assert_eq!(sess.terminal_cwd.as_deref(), Some("/srv/app"));
        assert_eq!(sess.sftp_path, "/srv/app");
        assert!(sess.sftp_loading);
        assert!(sess.sftp_list_request_id.is_some());
    }

    #[test]
    fn terminal_cwd_follow_uses_original_path_when_sftp_returns_canonical_path() {
        let mut app_state = app_state();
        let mut sess = session_state(true);
        sess.sftp_auto_sync = true;
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.handle_event(FromCore::Cwd {
            id,
            path: "/home/demo/link".to_string(),
        });
        let request_id = app_state.sessions[&id].sftp_list_request_id.unwrap();

        app_state.handle_event(FromCore::SftpListing {
            id,
            request_id,
            path: "/srv/real".to_string(),
            entries: Vec::new(),
        });

        let sess = &app_state.sessions[&id];
        assert_eq!(sess.sftp_path, "/srv/real");
        assert_eq!(
            sess.sftp_followed_terminal_cwd.as_deref(),
            Some("/home/demo/link")
        );
        assert!(!sess.sftp_loading);
        assert!(sess.sftp_list_request_id.is_none());

        app_state.handle_event(FromCore::Cwd {
            id,
            path: "/home/demo/link".to_string(),
        });
        assert!(!app_state.sessions[&id].sftp_loading);
        assert!(app_state.sessions[&id].sftp_list_request_id.is_none());
    }

    #[test]
    fn terminal_cwd_event_only_updates_cache_when_auto_sync_is_disabled() {
        let mut app_state = app_state();
        let sess = session_state(true);
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.handle_event(FromCore::Cwd {
            id,
            path: "/srv/app".to_string(),
        });

        let sess = app_state.sessions.get(&id).unwrap();
        assert_eq!(sess.terminal_cwd.as_deref(), Some("/srv/app"));
        assert_eq!(sess.sftp_path, ".");
        assert!(!sess.sftp_loading);
        assert!(sess.sftp_list_request_id.is_none());
    }

    #[test]
    fn simple_cd_input_infers_absolute_and_quoted_paths() {
        let mut sess = session_state(true);
        sess.terminal_cwd = Some("/tmp".to_string());

        assert_eq!(
            sess.observe_terminal_input(b"cd /home\r"),
            Some("/home".to_string())
        );
        assert_eq!(
            sess.observe_terminal_input(b"cd -- '/srv/my logs'\r"),
            Some("/srv/my logs".to_string())
        );
        // 相对路径基于上一条命令推进后的目录解析。
        assert_eq!(
            sess.observe_terminal_input(b"cd ../app\r"),
            Some("/srv/app".to_string())
        );
    }

    #[test]
    fn complex_shell_syntax_is_never_inferred() {
        let mut sess = session_state(true);
        sess.terminal_cwd = Some("/tmp".to_string());
        sess.remote_home = Some("/home/demo".to_string());

        for line in [
            &b"cd /missing && echo nope\r"[..],
            b"cd \"$HOME\"\r",
            b"cd ''\r",
            b"cd ~demo\r",
            b"cd /a /b\r",
            b"pushd\r",
            b"popd 2\r",
        ] {
            assert_eq!(sess.observe_terminal_input(line), None, "行: {line:?}");
        }
        assert_eq!(sess.terminal_cwd.as_deref(), Some("/tmp"));
    }

    #[test]
    fn home_shorthand_is_inferred_once_remote_home_is_known() {
        let mut sess = session_state(true);
        sess.terminal_cwd = Some("/tmp".to_string());

        // home 未知时不猜。
        assert_eq!(sess.observe_terminal_input(b"cd\r"), None);
        assert_eq!(sess.observe_terminal_input(b"cd ~\r"), None);

        sess.remote_home = Some("/home/demo".to_string());
        assert_eq!(
            sess.observe_terminal_input(b"cd\r"),
            Some("/home/demo".to_string())
        );
        assert_eq!(
            sess.observe_terminal_input(b"cd ~\r"),
            Some("/home/demo".to_string()),
            "已经在 home 时仍解析出 home，去重交给下游跟随判断"
        );
        assert_eq!(
            sess.observe_terminal_input(b"cd ~/logs\r"),
            Some("/home/demo/logs".to_string())
        );
    }

    #[test]
    fn cd_dash_returns_to_the_previous_directory() {
        let mut sess = session_state(true);
        sess.terminal_cwd = Some("/tmp".to_string());

        // 没有 OLDPWD 时不猜。
        assert_eq!(sess.observe_terminal_input(b"cd -\r"), None);

        assert_eq!(
            sess.observe_terminal_input(b"cd /var/log\r"),
            Some("/var/log".to_string())
        );
        assert_eq!(
            sess.observe_terminal_input(b"cd -\r"),
            Some("/tmp".to_string())
        );
        assert_eq!(
            sess.observe_terminal_input(b"cd -\r"),
            Some("/var/log".to_string()),
            "再次 cd - 应在两个目录之间来回"
        );
    }

    #[test]
    fn pushd_and_popd_track_the_directory_stack() {
        let mut sess = session_state(true);
        sess.terminal_cwd = Some("/srv/app".to_string());

        assert_eq!(
            sess.observe_terminal_input(b"pushd /etc\r"),
            Some("/etc".to_string())
        );
        assert_eq!(sess.terminal_dir_stack, vec!["/srv/app".to_string()]);
        assert_eq!(
            sess.observe_terminal_input(b"popd\r"),
            Some("/srv/app".to_string())
        );
        assert!(sess.terminal_dir_stack.is_empty());
        // 栈空时 popd 无从推断。
        assert_eq!(sess.observe_terminal_input(b"popd\r"), None);
    }

    #[test]
    fn unresolvable_pushd_target_does_not_grow_the_stack() {
        let mut sess = session_state(true);
        sess.terminal_cwd = None;

        // 当前目录未知时相对路径无法解析，栈必须保持为空，否则会与远端漂移。
        assert_eq!(sess.observe_terminal_input(b"pushd logs\r"), None);
        assert!(sess.terminal_dir_stack.is_empty());
    }

    #[test]
    fn input_editing_controls_disable_inference_for_the_current_line() {
        let mut sess = session_state(true);

        assert_eq!(sess.observe_terminal_input(b"cd /home\x1b[A\r"), None);
        assert_eq!(sess.observe_terminal_input(b"cd\t/home\r"), None);
        assert!(!sess.terminal_input_invalid);
        assert!(sess.terminal_input_buffer.is_empty());
    }

    #[test]
    fn overlong_terminal_input_is_not_kept_or_inferred() {
        let mut sess = session_state(true);
        let input = vec![b'a'; MAX_TERMINAL_INPUT_OBSERVATION_BYTES + 1];

        assert_eq!(sess.observe_terminal_input(&input), None);
        assert!(sess.terminal_input_invalid);
        assert!(sess.terminal_input_buffer.is_empty());
        assert_eq!(sess.observe_terminal_input(b"\r"), None);
        assert!(!sess.terminal_input_invalid);
    }

    #[test]
    fn submitted_cd_input_updates_auto_sync_without_extra_probe_command() {
        let mut app_state = app_state();
        let mut sess = session_state(true);
        sess.sftp_auto_sync = true;
        sess.terminal_cwd = Some("/tmp".to_string());
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        assert!(app_state.send_terminal_input(id, b"cd /ho".to_vec()));
        assert!(app_state.send_terminal_input(id, b"me\r".to_vec()));

        let sess = &app_state.sessions[&id];
        assert_eq!(sess.terminal_cwd.as_deref(), Some("/home"));
        assert!(sess.sftp_loading);
        assert!(sess.sftp_list_request_id.is_some());
        assert!(sess.terminal_input_buffer.is_empty());
    }

    #[test]
    fn stale_osc7_after_submitted_cd_does_not_override_inferred_directory() {
        let mut app_state = app_state();
        let mut sess = session_state(true);
        sess.sftp_auto_sync = true;
        sess.terminal_cwd = Some("/root".to_string());
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        assert!(app_state.send_terminal_input(id, b"cd /home\r".to_vec()));
        let request_id = app_state.sessions[&id].sftp_list_request_id.unwrap();
        assert_eq!(
            app_state.sessions[&id]
                .terminal_cwd_inference_target
                .as_deref(),
            Some("/home")
        );

        // 这是用户输入前已经排队的旧 OSC 7，不应把文件管理器带回旧目录。
        app_state.handle_event(FromCore::Cwd {
            id,
            path: "/root".to_string(),
        });
        let sess = &app_state.sessions[&id];
        assert_eq!(sess.terminal_cwd.as_deref(), Some("/home"));
        assert_eq!(sess.sftp_path, "/home");
        assert_eq!(sess.sftp_list_request_id, Some(request_id));

        app_state.handle_event(FromCore::SftpListing {
            id,
            request_id,
            path: "/home".to_string(),
            entries: Vec::new(),
        });
        let sess = &app_state.sessions[&id];
        assert_eq!(sess.sftp_path, "/home");
        assert!(!sess.sftp_loading);
        assert!(sess.sftp_list_request_id.is_none());
    }

    #[test]
    fn checked_auto_sync_follows_per_key_cd_after_previous_listing_finishes() {
        let mut app_state = app_state();
        let mut sess = session_state(true);
        sess.terminal_cwd = Some("/root".to_string());
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.set_sftp_auto_sync(id, true).unwrap();
        let previous_request_id = app_state.sessions[&id].sftp_list_request_id.unwrap();

        for byte in b"cd /home\r" {
            assert!(app_state.send_terminal_input(id, vec![*byte]));
        }
        app_state.handle_event(FromCore::Cwd {
            id,
            path: "/root".to_string(),
        });
        app_state.handle_event(FromCore::SftpListing {
            id,
            request_id: previous_request_id,
            path: "/root".to_string(),
            entries: Vec::new(),
        });

        let home_request_id = app_state.sessions[&id].sftp_list_request_id.unwrap();
        assert_ne!(home_request_id, previous_request_id);
        assert_eq!(app_state.sessions[&id].sftp_path, "/home");

        app_state.handle_event(FromCore::SftpListing {
            id,
            request_id: home_request_id,
            path: "/home".to_string(),
            entries: Vec::new(),
        });

        let sess = &app_state.sessions[&id];
        assert!(sess.sftp_auto_sync);
        assert_eq!(sess.terminal_cwd.as_deref(), Some("/home"));
        assert_eq!(sess.sftp_path, "/home");
        assert!(!sess.sftp_loading);
        assert!(sess.sftp_list_request_id.is_none());
    }

    #[test]
    fn later_terminal_input_releases_unconfirmed_cd_target() {
        let mut sess = session_state(true);
        sess.terminal_cwd = Some("/home".to_string());
        sess.terminal_cwd_inference_target = Some("/home".to_string());

        // 即使这条命令本身不产生任何目录推断，命令行时序也已经向前推进，
        // 旧的待确认目标不得继续压制后续的 OSC 7。
        assert_eq!(sess.observe_terminal_input(b"ls -la\r"), None);
        assert!(sess.terminal_cwd_inference_target.is_none());
    }

    #[test]
    fn file_manager_cd_replaces_previous_inferred_cwd_confirmation() {
        let mut app_state = app_state();
        let mut sess = session_state(true);
        sess.sftp_auto_sync = true;
        sess.sftp_path = "/var/log".to_string();
        sess.terminal_cwd = Some("/home".to_string());
        sess.terminal_cwd_inference_target = Some("/home".to_string());
        sess.sftp_terminal_sync_request = Some((SftpRequestId(9), Some("/home".to_string())));
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.sync_terminal_to_path(id, "/var/log".to_string());
        assert!(app_state.sessions[&id]
            .terminal_cwd_inference_target
            .is_none());

        app_state.handle_event(FromCore::Cwd {
            id,
            path: "/var/log".to_string(),
        });
        let sess = &app_state.sessions[&id];
        assert_eq!(sess.terminal_cwd.as_deref(), Some("/var/log"));
        assert!(sess.sftp_terminal_sync_request.is_none());
    }

    #[test]
    fn auto_sync_coalesces_terminal_cwd_changes_while_listing() {
        let mut app_state = app_state();
        let mut sess = session_state(true);
        sess.sftp_auto_sync = true;
        sess.sftp_path = "/srv/old".to_string();
        sess.sftp_loading = true;
        sess.sftp_list_request_id = Some(SftpRequestId(41));
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.handle_event(FromCore::Cwd {
            id,
            path: "/srv/latest".to_string(),
        });
        assert_eq!(app_state.sessions[&id].sftp_path, "/srv/old");

        app_state.handle_event(FromCore::SftpListing {
            id,
            request_id: SftpRequestId(41),
            path: "/srv/old".to_string(),
            entries: Vec::new(),
        });

        let sess = &app_state.sessions[&id];
        assert_eq!(sess.sftp_path, "/srv/latest");
        assert!(sess.sftp_loading);
        assert_ne!(sess.sftp_list_request_id, Some(SftpRequestId(41)));
    }

    #[test]
    fn file_manager_sync_waits_for_canonical_path_and_ignores_old_cwd() {
        let mut app_state = app_state();
        let mut sess = session_state(true);
        sess.sftp_auto_sync = true;
        sess.sftp_path = "./logs".to_string();
        sess.sftp_loading = true;
        sess.sftp_list_request_id = Some(SftpRequestId(42));
        sess.terminal_cwd = Some("/home/demo".to_string());
        sess.sftp_terminal_sync_request = Some((SftpRequestId(42), Some("/home/demo".to_string())));
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.handle_event(FromCore::SftpListing {
            id,
            request_id: SftpRequestId(42),
            path: "/home/demo/logs".to_string(),
            entries: Vec::new(),
        });

        let sess = &app_state.sessions[&id];
        assert_eq!(sess.sftp_path, "/home/demo/logs");
        assert_eq!(sess.terminal_cwd.as_deref(), Some("/home/demo"));
        assert!(sess.sftp_terminal_sync_request.is_some());

        app_state.handle_event(FromCore::Cwd {
            id,
            path: "/home/demo".to_string(),
        });
        let sess = &app_state.sessions[&id];
        assert_eq!(sess.sftp_path, "/home/demo/logs");
        assert!(!sess.sftp_loading);
        assert!(sess.sftp_terminal_sync_request.is_some());

        app_state.handle_event(FromCore::Cwd {
            id,
            path: "/home/demo/previous-target".to_string(),
        });
        let sess = &app_state.sessions[&id];
        assert_eq!(sess.sftp_path, "/home/demo/logs");
        assert!(sess.sftp_terminal_sync_request.is_some());

        app_state.handle_event(FromCore::Cwd {
            id,
            path: "/home/demo/logs".to_string(),
        });
        let sess = &app_state.sessions[&id];
        assert_eq!(sess.terminal_cwd.as_deref(), Some("/home/demo/logs"));
        assert_eq!(
            sess.sftp_followed_terminal_cwd.as_deref(),
            Some("/home/demo/logs")
        );
        assert!(sess.sftp_terminal_sync_request.is_none());
        assert!(!sess.sftp_loading);
    }

    #[test]
    fn cancelling_one_unknown_host_keeps_other_host_pending() {
        let mut app_state = app_state();
        app_state
            .sessions
            .insert(SessionId(1), pending_session(1, "alpha.example.com", 22));
        app_state
            .sessions
            .insert(SessionId(2), pending_session(2, "beta.example.com", 2222));

        assert_eq!(
            app_state.clear_host_key_pending_for("alpha.example.com", 22),
            1
        );
        assert!(!app_state.sessions[&SessionId(1)].host_key_pending);
        assert!(app_state.sessions[&SessionId(2)].host_key_pending);
    }

    #[test]
    fn trusting_same_host_reconnects_all_matching_sessions_only() {
        let mut app_state = app_state();
        app_state
            .sessions
            .insert(SessionId(1), pending_session(1, "EXAMPLE.com", 22));
        app_state
            .sessions
            .insert(SessionId(2), pending_session(2, "example.com", 22));
        app_state
            .sessions
            .insert(SessionId(3), pending_session(3, "other.example.com", 22));

        assert_eq!(
            app_state.reconnect_host_key_pending_for("example.com", 22),
            2
        );
        assert!(!app_state.sessions[&SessionId(1)].host_key_pending);
        assert!(!app_state.sessions[&SessionId(2)].host_key_pending);
        assert!(app_state.sessions[&SessionId(3)].host_key_pending);
    }

    #[test]
    fn terminal_title_event_does_not_clear_session_name() {
        let mut app_state = app_state();
        let sess = session_state(true);
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.handle_event(FromCore::Title {
            id,
            title: String::new(),
        });

        let sess = app_state.sessions.get(&id).unwrap();
        assert_eq!(sess.title, "demo");
    }

    #[test]
    fn terminal_title_event_does_not_replace_session_name() {
        let mut app_state = app_state();
        let sess = session_state(true);
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.handle_event(FromCore::Title {
            id,
            title: "htop".to_string(),
        });

        let sess = app_state.sessions.get(&id).unwrap();
        assert_eq!(sess.title, "demo");
    }

    #[test]
    fn auth_challenge_is_recorded_and_cleared_on_close() {
        let mut app_state = app_state();
        let sess = session_state(false);
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.handle_event(FromCore::AuthChallenge {
            id,
            challenge: AuthChallenge::Password {
                user: "root".to_string(),
                host: "example.com".to_string(),
                port: 22,
            },
        });
        assert!(app_state.sessions[&id].auth_challenge.is_some());

        app_state.handle_event(FromCore::Closed { id, error: None });
        assert!(app_state.sessions[&id].auth_challenge.is_none());
    }

    #[test]
    fn monitor_stopped_clears_loading_without_overwriting_error() {
        let mut app_state = app_state();
        let mut sess = session_state(true);
        sess.monitor_loading = true;
        sess.monitor_error = Some("旧错误".to_string());
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.handle_event(FromCore::MonitorStopped { id });

        let sess = app_state.sessions.get(&id).unwrap();
        assert!(!sess.monitor_loading);
        assert_eq!(sess.monitor_error.as_deref(), Some("旧错误"));
    }

    #[test]
    fn sftp_stopped_clears_loading_and_progress_without_overwriting_error() {
        let mut app_state = app_state();
        let mut sess = session_state(false);
        sess.sftp_loading = true;
        sess.sftp_error = Some("旧错误".to_string());
        sess.sftp_progress = Some(SftpProgressState {
            request_id: SftpRequestId(1),
            name: "demo.bin".to_string(),
            transferred: 10,
            total: 20,
        });
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.handle_event(FromCore::SftpStopped { id });

        let sess = app_state.sessions.get(&id).unwrap();
        assert!(!sess.sftp_loading);
        assert!(sess.sftp_progress.is_none());
        assert_eq!(sess.sftp_error.as_deref(), Some("旧错误"));
    }

    #[test]
    fn stale_sftp_stopped_does_not_clear_reconnected_session_request() {
        let mut app_state = app_state();
        let mut sess = session_state(true);
        sess.sftp_loading = true;
        sess.sftp_list_request_id = Some(SftpRequestId(9));
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.handle_event(FromCore::SftpStopped { id });

        let sess = app_state.sessions.get(&id).unwrap();
        assert!(sess.sftp_loading);
        assert_eq!(sess.sftp_list_request_id, Some(SftpRequestId(9)));
    }

    #[test]
    fn stale_sftp_listing_does_not_replace_current_directory_request() {
        let mut app_state = app_state();
        let mut sess = session_state(true);
        sess.sftp_path = "/new".to_string();
        sess.sftp_loading = true;
        sess.sftp_list_request_id = Some(SftpRequestId(2));
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.handle_event(FromCore::SftpListing {
            id,
            request_id: SftpRequestId(1),
            path: "/old".to_string(),
            entries: Vec::new(),
        });

        let sess = app_state.sessions.get(&id).unwrap();
        assert_eq!(sess.sftp_path, "/new");
        assert!(sess.sftp_loading);
        assert_eq!(sess.sftp_list_request_id, Some(SftpRequestId(2)));

        app_state.handle_event(FromCore::SftpListing {
            id,
            request_id: SftpRequestId(2),
            path: "/new".to_string(),
            entries: Vec::new(),
        });

        let sess = app_state.sessions.get(&id).unwrap();
        assert_eq!(sess.sftp_path, "/new");
        assert!(!sess.sftp_loading);
        assert!(sess.sftp_list_request_id.is_none());
    }
}
