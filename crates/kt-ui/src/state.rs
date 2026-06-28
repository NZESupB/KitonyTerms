//! 全局应用状态

use std::collections::HashMap;

use kt_core::monitor::MonitorStats;
use kt_core::term::GridSnapshot;
use kt_core::{FromCore, SessionId, SessionManager, SftpEntry, SftpOp, SftpRequest, ToCore};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SftpCompletion {
    pub op: SftpOp,
    pub path: String,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SftpProgressState {
    pub name: String,
    pub transferred: u64,
    pub total: u64,
}

/// 单个会话的 UI 状态
#[derive(Clone)]
pub struct SessionState {
    pub id: SessionId,
    pub title: String,
    pub snapshot: Option<GridSnapshot>,
    pub connected: bool,
    /// 最近一次连接错误。None 表示仍在连接或已连接。
    pub connection_error: Option<String>,

    // SFTP 状态
    pub sftp_path: String,
    pub sftp_entries: Vec<SftpEntry>,
    pub sftp_loading: bool,
    pub sftp_error: Option<String>,
    pub sftp_last_done: Option<SftpCompletion>,
    pub sftp_progress: Option<SftpProgressState>,

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
    next_sftp_completion_revision: u64,
}

impl AppState {
    pub fn new(manager: SessionManager) -> Self {
        Self {
            manager,
            sessions: HashMap::new(),
            next_id: 1,
            next_sftp_completion_revision: 1,
        }
    }

    /// 创建新会话 ID
    pub fn next_session_id(&mut self) -> SessionId {
        let id = SessionId(self.next_id);
        self.next_id += 1;
        id
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
                    self.manager.send(ToCore::Sftp {
                        id,
                        req: SftpRequest::List { path },
                    });
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
                if let Some(sess) = self.sessions.get_mut(&id) {
                    sess.title = title;
                }
            }
            FromCore::Closed { id, error } => {
                if let Some(sess) = self.sessions.get_mut(&id) {
                    sess.connected = false;
                    sess.connection_error = error.clone();
                    sess.monitor_loading = false;
                    sess.monitor_error = None;
                    sess.sftp_loading = false;
                    if let Some(err) = error {
                        tracing::warn!("Session {} closed with error: {}", id.0, err);
                    }
                }
            }
            FromCore::SftpListing { id, path, entries } => {
                tracing::info!(
                    "收到 SFTP 列表，会话 {:?}，路径 {}，{} 项",
                    id,
                    path,
                    entries.len()
                );
                if let Some(sess) = self.sessions.get_mut(&id) {
                    sess.sftp_path = path;
                    sess.sftp_entries = entries;
                    sess.sftp_loading = false;
                    sess.sftp_error = None;
                }
            }
            FromCore::SftpError { id, message } => {
                tracing::error!("SFTP 错误，会话 {:?}: {}", id, message);
                if let Some(sess) = self.sessions.get_mut(&id) {
                    sess.sftp_loading = false;
                    sess.sftp_error = Some(message);
                    sess.sftp_progress = None;
                }
            }
            FromCore::SftpProgress {
                id,
                name,
                transferred,
                total,
            } => {
                if let Some(sess) = self.sessions.get_mut(&id) {
                    sess.sftp_progress = Some(SftpProgressState {
                        name,
                        transferred,
                        total,
                    });
                }
            }
            FromCore::SftpDone { id, op, path } => {
                tracing::info!("SFTP 操作完成，会话 {:?}: {:?} {}", id, op, path);
                let revision = self.next_sftp_completion_revision;
                self.next_sftp_completion_revision += 1;
                if let Some(sess) = self.sessions.get_mut(&id) {
                    sess.sftp_last_done = Some(SftpCompletion {
                        op,
                        path: path.clone(),
                        revision,
                    });
                    sess.sftp_progress = None;
                }
                if should_refresh_after_sftp_op(op) {
                    let sftp_path = self.sessions.get_mut(&id).map(|sess| {
                        sess.sftp_loading = true;
                        sess.sftp_error = None;
                        sess.sftp_path.clone()
                    });
                    if let Some(path) = sftp_path {
                        self.manager.send(ToCore::Sftp {
                            id,
                            req: SftpRequest::List { path },
                        });
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
            _ => {
                // 暂时忽略其他事件
            }
        }
    }
}

/// 全局状态包装器（用于 Dioxus Signal）
pub type GlobalState = std::sync::Arc<std::sync::Mutex<AppState>>;

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
            snapshot: None,
            connected,
            connection_error: None,
            sftp_path: ".".to_string(),
            sftp_entries: Vec::new(),
            sftp_loading: false,
            sftp_error: None,
            sftp_last_done: None,
            sftp_progress: None,
            monitor: None,
            monitor_loading: false,
            monitor_error: None,
        }
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
}
