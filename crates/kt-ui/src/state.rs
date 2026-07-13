//! 全局应用状态

use std::collections::{HashMap, VecDeque};

use kt_config::ConnectParams;
use kt_core::monitor::MonitorStats;
use kt_core::term::GridSnapshot;
use kt_core::PtySize;
use kt_core::{
    AuthChallenge, FromCore, SessionId, SessionManager, SftpEntry, SftpOp, SftpRequest,
    SftpRequestId, ToCore,
};

const MAX_SFTP_OUTCOMES: usize = 256;

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
    /// 远端 shell 通过 OSC 7 上报的当前工作目录（供"跟随终端目录"使用）。
    pub terminal_cwd: Option<String>,

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
                if let Some(sess) = self.sessions.get_mut(&id) {
                    sess.terminal_cwd = Some(path);
                }
            }
            FromCore::Bell { .. } => {}
            FromCore::Closed { id, error } => {
                if let Some(sess) = self.sessions.get_mut(&id) {
                    sess.connected = false;
                    if sess.host_key_pending {
                        sess.connection_error = None;
                    } else {
                        sess.connection_error = error.clone();
                    }
                    sess.auth_challenge = None;
                    sess.monitor_loading = false;
                    sess.monitor_error = None;
                    sess.sftp_loading = false;
                    sess.sftp_list_request_id = None;
                    sess.sftp_progress = None;
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
                if let Some(sess) = self.sessions.get_mut(&id) {
                    if sess.sftp_list_request_id != Some(request_id) {
                        tracing::debug!(
                            "忽略迟到的 SFTP 列表，会话 {:?}，request={:?}",
                            id,
                            request_id
                        );
                        return;
                    }
                    sess.sftp_path = path;
                    sess.sftp_entries = entries;
                    sess.sftp_loading = false;
                    sess.sftp_error = None;
                    sess.sftp_list_request_id = None;
                }
            }
            FromCore::SftpError {
                id,
                request_id,
                message,
            } => {
                tracing::error!("SFTP 错误，会话 {:?}: {}", id, message);
                if let Some(sess) = self.sessions.get_mut(&id) {
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
                    }
                    if is_current_progress {
                        sess.sftp_progress = None;
                    }
                }
            }
            FromCore::SftpStopped { id } => {
                if let Some(sess) = self.sessions.get_mut(&id) {
                    sess.sftp_loading = false;
                    sess.sftp_list_request_id = None;
                    sess.sftp_progress = None;
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
        let mut reconnects = Vec::new();
        for sess in self.sessions.values_mut() {
            if sess.host_key_pending && session_matches_host_key_target(sess, host, port) {
                sess.connected = false;
                sess.connection_error = None;
                sess.host_key_pending = false;
                sess.auth_challenge = None;
                sess.monitor_loading = false;
                sess.monitor_error = None;
                sess.sftp_loading = false;
                reconnects.push((sess.id, sess.connect_params.clone(), sess.pty));
            }
        }

        let reconnect_count = reconnects.len();
        for (id, params, pty) in reconnects {
            self.manager.send(ToCore::Connect {
                id,
                params: Box::new(params),
                pty,
            });
        }
        reconnect_count
    }
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
    fn session_close_clears_monitor_pending_and_error_state() {
        let mut app_state = app_state();
        let mut sess = session_state(true);
        sess.monitor_loading = true;
        sess.monitor_error = Some("资源监控通道已关闭".to_string());
        let id = sess.id;
        app_state.sessions.insert(id, sess);

        app_state.handle_event(FromCore::Closed { id, error: None });

        let sess = app_state.sessions.get(&id).unwrap();
        assert!(!sess.connected);
        assert!(sess.connection_error.is_none());
        assert!(!sess.monitor_loading);
        assert!(sess.monitor_error.is_none());
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
        let mut sess = session_state(true);
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
