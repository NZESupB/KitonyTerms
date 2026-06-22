//! 全局应用状态

use std::collections::HashMap;

use kt_core::{FromCore, SessionId, SessionManager, SftpEntry};
use kt_core::term::GridSnapshot;
use kt_core::monitor::MonitorStats;

/// 单个会话的 UI 状态
#[derive(Clone)]
pub struct SessionState {
    pub id: SessionId,
    pub title: String,
    pub snapshot: Option<GridSnapshot>,
    pub connected: bool,

    // SFTP 状态
    pub sftp_path: String,
    pub sftp_entries: Vec<SftpEntry>,
    pub sftp_loading: bool,
    pub sftp_error: Option<String>,

    /// 最近一次资源监控采样。
    pub monitor: Option<MonitorStats>,
}

/// 全局应用状态（跨组件共享）
pub struct AppState {
    pub manager: SessionManager,
    pub sessions: HashMap<SessionId, SessionState>,
    pub next_id: u64,
}

impl AppState {
    pub fn new(manager: SessionManager) -> Self {
        Self {
            manager,
            sessions: HashMap::new(),
            next_id: 1,
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
            tracing::info!("收到事件: {:?}", ev);
            match ev {
                FromCore::Connected { id } => {
                    tracing::info!("会话 {:?} 已连接", id);
                    if let Some(sess) = self.sessions.get_mut(&id) {
                        sess.connected = true;
                    }
                }
                FromCore::Render { id, snapshot } => {
                    tracing::info!("收到终端渲染数据，会话 {:?}，revision {}", id, snapshot.revision);
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
                        if let Some(err) = error {
                            tracing::warn!("Session {} closed with error: {}", id.0, err);
                        }
                    }
                }
                FromCore::SftpListing { id, path, entries } => {
                    tracing::info!("收到 SFTP 列表，会话 {:?}，路径 {}，{} 项", id, path, entries.len());
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
                    }
                }
                FromCore::SftpDone { id, op } => {
                    tracing::info!("SFTP 操作完成，会话 {:?}: {:?}", id, op);
                }
                FromCore::Monitor { id, stats } => {
                    if let Some(sess) = self.sessions.get_mut(&id) {
                        sess.monitor = Some(*stats);
                    }
                }
                _ => {
                    // 暂时忽略其他事件
                }
            }
        }
        if event_count > 0 {
            tracing::info!("本轮处理了 {} 个事件", event_count);
        }
    }
}

/// 全局状态包装器（用于 Dioxus Signal）
pub type GlobalState = std::sync::Arc<std::sync::Mutex<AppState>>;
