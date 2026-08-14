//! App 层状态控制与副作用管理（事件循环/状态同步）。

use std::sync::{Arc, Mutex};

use dioxus::prelude::*;
use kt_core::SessionId;

use crate::components::external_edit::{sync_external_edits, ExternalEdit, ExternalEditAction};
use crate::state::{AppState, SessionState};
use crate::store::{PendingHostKey, Store};

pub struct StoreSignals {
    pub host_key_prompt: Signal<Option<PendingHostKey>>,
    pub status_notice: Signal<Option<String>>,
}

pub fn resolve_active_session_id(
    current: Option<SessionId>,
    sessions: &[SessionState],
) -> Option<SessionId> {
    if current.is_some_and(|id| sessions.iter().any(|sess| sess.id == id)) {
        current
    } else {
        sessions.first().map(|sess| sess.id)
    }
}

/// 运行 App 核心副作用循环：
/// - 定期 pump core 事件
/// - 同步会话列表和主机密钥确认弹窗
/// - 处理外部编辑状态机并派发动作
pub fn use_state_controller(
    state: &'static Arc<Mutex<AppState>>,
    store: Arc<Store>,
    mut all_sessions: Signal<Vec<SessionState>>,
    mut active_session_id: Signal<Option<SessionId>>,
    store_signals: StoreSignals,
    mut external_edits: Signal<Vec<ExternalEdit>>,
    on_external_edit_action: Callback<ExternalEditAction>,
) {
    let StoreSignals {
        mut host_key_prompt,
        mut status_notice,
    } = store_signals;
    use_future(move || async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(16)).await;
            if let Ok(mut app_state) = state.lock() {
                app_state.pump_events();
            }
        }
    });

    use_effect(move || {
        let store = store.clone();
        spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                if let Ok(s) = state.lock() {
                    let mut sessions: Vec<SessionState> = s.sessions.values().cloned().collect();
                    sessions.sort_by_key(|sess| sess.id);
                    all_sessions.set(sessions.clone());

                    let next_active =
                        resolve_active_session_id(*active_session_id.peek(), &sessions);
                    if *active_session_id.peek() != next_active {
                        active_session_id.set(next_active);
                    }
                }
                let pending = store.peek_pending_host_key();
                if *host_key_prompt.peek() != pending {
                    host_key_prompt.set(pending);
                }
                if let Some(notice) = take_latest_status_notice(&store) {
                    status_notice.set(Some(notice));
                }
            }
        });
    });

    use_effect(move || {
        let state = state.clone();
        spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
                let sessions = if let Ok(app_state) = state.lock() {
                    app_state.sessions.clone()
                } else {
                    continue;
                };
                let (next_edits, actions) =
                    sync_external_edits(external_edits.peek().clone(), &sessions);
                if *external_edits.peek() != next_edits {
                    external_edits.set(next_edits);
                }
                for action in actions {
                    on_external_edit_action.call(action);
                }
            }
        });
    });
}

fn take_latest_status_notice(store: &Store) -> Option<String> {
    let mut latest = None;
    while let Some(notice) = store.take_status_notice() {
        latest = Some(notice);
    }
    latest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::app_logic::session_state_from_profile;
    use kt_config::{ConnectParams, KnownHostCheck, KnownHosts, SessionProfile};

    fn session(id: u64, title: &str) -> SessionState {
        let profile = SessionProfile {
            name: title.to_string(),
            group: None,
            params: ConnectParams::new("example.com", "root"),
        };
        session_state_from_profile(SessionId(id), &profile)
    }

    #[test]
    fn active_session_resolution_keeps_valid_current_session() {
        let sessions = vec![session(1, "a"), session(2, "b")];

        assert_eq!(
            resolve_active_session_id(Some(SessionId(2)), &sessions),
            Some(SessionId(2))
        );
    }

    #[test]
    fn active_session_resolution_picks_first_when_missing_or_stale() {
        let sessions = vec![session(1, "a"), session(2, "b")];

        assert_eq!(
            resolve_active_session_id(None, &sessions),
            Some(SessionId(1))
        );
        assert_eq!(
            resolve_active_session_id(Some(SessionId(9)), &sessions),
            Some(SessionId(1))
        );
    }

    #[test]
    fn active_session_resolution_clears_when_no_sessions_exist() {
        assert_eq!(resolve_active_session_id(Some(SessionId(1)), &[]), None);
    }

    #[test]
    fn persistence_notice_is_drained_for_status_bar() {
        let dir = tempfile::tempdir().unwrap();
        let known_hosts_path = dir.path().join("known_hosts.toml");
        let mut known_hosts = KnownHosts::default();
        known_hosts.trust("example.com", 22, "SHA256:key");
        known_hosts.save_to(&known_hosts_path).unwrap();
        let store = Store::load_from_files(
            dir.path().join("config.toml"),
            dir.path().join("secrets.vault"),
            known_hosts_path.clone(),
        )
        .unwrap();
        std::fs::remove_file(&known_hosts_path).unwrap();
        std::fs::create_dir(&known_hosts_path).unwrap();

        assert_eq!(
            store
                .check_host_key("example.com", 22, "SHA256:key")
                .unwrap(),
            KnownHostCheck::Trusted
        );
        assert!(take_latest_status_notice(&store)
            .unwrap()
            .contains("最近访问时间失败"));
        assert_eq!(take_latest_status_notice(&store), None);
    }
}
