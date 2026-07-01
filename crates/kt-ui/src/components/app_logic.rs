//! App 主组件可复用的纯逻辑。

use std::path::{Path, PathBuf};

use kt_config::{lookup_ssh_config, normalize_group_name, ConnectParams, SessionProfile};
use kt_core::term::GridSnapshot;
use kt_core::{AuthChallenge, SessionId, SftpEntry};

use crate::i18n::AppText;
use crate::state::SessionState;

pub const DEFAULT_GROUP_NAME: &str = "NoBrand";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionConnectionStatus {
    Connected,
    Authenticating,
    Disconnected,
    Connecting,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionTabView {
    pub id: SessionId,
    pub title: String,
    pub status: SessionConnectionStatus,
}

#[derive(Clone, Debug)]
pub struct ActiveTerminalView {
    pub id: SessionId,
    pub title: String,
    pub status: SessionConnectionStatus,
    pub snapshot: Option<GridSnapshot>,
    pub connected: bool,
    pub connection_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveSftpView {
    pub session_id: SessionId,
    pub connected: bool,
    pub path: String,
    pub entries: Vec<SftpEntry>,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveMonitorView {
    pub session_id: SessionId,
    pub loading: bool,
    pub error: Option<String>,
    pub has_sample: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusBarSessionView {
    pub title: String,
    pub status: SessionConnectionStatus,
}

pub type AuthChallengeView = (SessionId, String, AuthChallenge);

/// 按 group 字段分组，无 group 的归入默认组名，并保留空分组。
pub fn group_profiles(
    profiles: &[SessionProfile],
    groups: &[String],
) -> Vec<(String, Vec<SessionProfile>)> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<String, Vec<SessionProfile>> = BTreeMap::new();
    for group in groups {
        if let Some(group) = normalize_group_name(group) {
            map.entry(group).or_default();
        }
    }
    for profile in profiles {
        let key = profile
            .group
            .as_deref()
            .and_then(normalize_group_name)
            .unwrap_or_else(|| DEFAULT_GROUP_NAME.to_string());
        map.entry(key).or_default().push(profile.clone());
    }
    map.into_iter().collect()
}

pub fn session_state_from_profile(id: SessionId, profile: &SessionProfile) -> SessionState {
    SessionState {
        id,
        title: profile.name.clone(),
        snapshot: None,
        connected: false,
        connection_error: None,
        auth_challenge: None,
        sftp_path: ".".to_string(),
        sftp_entries: Vec::new(),
        sftp_loading: false,
        sftp_error: None,
        sftp_last_done: None,
        sftp_progress: None,
        terminal_cwd: None,
        monitor: None,
        monitor_loading: false,
        monitor_error: None,
    }
}

pub fn session_connection_status(sess: &SessionState) -> SessionConnectionStatus {
    if sess.connected {
        SessionConnectionStatus::Connected
    } else if sess.auth_challenge.is_some() {
        SessionConnectionStatus::Authenticating
    } else if sess.connection_error.is_some() {
        SessionConnectionStatus::Disconnected
    } else {
        SessionConnectionStatus::Connecting
    }
}

pub fn session_dot_class(sess: &SessionState) -> &'static str {
    session_dot_class_for_status(session_connection_status(sess))
}

pub fn session_dot_class_for_status(status: SessionConnectionStatus) -> &'static str {
    match status {
        SessionConnectionStatus::Connected => "status-dot online",
        SessionConnectionStatus::Authenticating => "status-dot connecting",
        SessionConnectionStatus::Disconnected => "status-dot idle",
        SessionConnectionStatus::Connecting => "status-dot connecting",
    }
}

pub fn session_status_pill_class(status: SessionConnectionStatus) -> &'static str {
    match status {
        SessionConnectionStatus::Connected => "status-pill connected",
        SessionConnectionStatus::Authenticating
        | SessionConnectionStatus::Disconnected
        | SessionConnectionStatus::Connecting => "status-pill pending",
    }
}

pub fn session_status_label(status: SessionConnectionStatus, text: &AppText) -> &'static str {
    match status {
        SessionConnectionStatus::Connected => text.connected,
        SessionConnectionStatus::Authenticating => text.authenticating,
        SessionConnectionStatus::Disconnected => text.disconnected,
        SessionConnectionStatus::Connecting => text.connecting,
    }
}

pub fn active_session(
    sessions: &[SessionState],
    active_id: Option<SessionId>,
) -> Option<&SessionState> {
    active_id.and_then(|id| sessions.iter().find(|sess| sess.id == id))
}

pub fn session_tab_views(sessions: &[SessionState]) -> Vec<SessionTabView> {
    sessions
        .iter()
        .map(|sess| SessionTabView {
            id: sess.id,
            title: sess.title.clone(),
            status: session_connection_status(sess),
        })
        .collect()
}

pub fn active_terminal_view(active: Option<&SessionState>) -> Option<ActiveTerminalView> {
    active.map(|sess| ActiveTerminalView {
        id: sess.id,
        title: sess.title.clone(),
        status: session_connection_status(sess),
        snapshot: sess.snapshot.clone(),
        connected: sess.connected,
        connection_error: sess.connection_error.clone(),
    })
}

pub fn active_sftp_view(active: Option<&SessionState>) -> Option<ActiveSftpView> {
    active.map(|sess| ActiveSftpView {
        session_id: sess.id,
        connected: sess.connected,
        path: sess.sftp_path.clone(),
        entries: sess.sftp_entries.clone(),
        loading: sess.sftp_loading,
        error: sess.sftp_error.clone(),
    })
}

pub fn active_monitor_view(active: Option<&SessionState>) -> Option<ActiveMonitorView> {
    active.map(|sess| ActiveMonitorView {
        session_id: sess.id,
        loading: sess.monitor_loading,
        error: sess.monitor_error.clone(),
        has_sample: sess.monitor.is_some(),
    })
}

pub fn status_bar_session_view(active: Option<&SessionState>) -> Option<StatusBarSessionView> {
    active.map(|sess| StatusBarSessionView {
        title: sess.title.clone(),
        status: session_connection_status(sess),
    })
}

pub fn auth_challenge_view(sessions: &[SessionState]) -> Option<AuthChallengeView> {
    sessions.iter().find_map(|sess| {
        sess.auth_challenge
            .clone()
            .map(|challenge| (sess.id, sess.title.clone(), challenge))
    })
}

pub fn params_with_ssh_config(params: ConnectParams, use_ssh_config: bool) -> ConnectParams {
    let Some(config_path) = home_ssh_config_file() else {
        return params;
    };
    merge_ssh_config_from_path(params, use_ssh_config, &config_path)
}

pub fn merge_ssh_config_from_path(
    mut params: ConnectParams,
    use_ssh_config: bool,
    config_path: &Path,
) -> ConnectParams {
    let original_vault_id = params.effective_vault_id();
    if use_ssh_config {
        let queried_host = params.host.clone();
        if let Ok(Some(host_cfg)) = lookup_ssh_config(config_path, &queried_host) {
            params.merge_ssh_config(&host_cfg);
        }
    }
    if params.vault_id.is_none() {
        params.vault_id = Some(original_vault_id);
    }
    params
}

fn home_ssh_config_file() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .map(|home| home.join(".ssh").join("config"))
}

pub fn clamp_dimension(value: f64, min: f64, max: f64) -> f64 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        min
    }
}

pub fn duplicate_profile(profile: &SessionProfile, existing: &[SessionProfile]) -> SessionProfile {
    let mut next = profile.clone();
    let names = existing
        .iter()
        .map(|profile| profile.name.as_str())
        .collect::<Vec<_>>();
    next.name = unique_copy_name(&profile.name, &names);
    next
}

pub fn unique_copy_name(base: &str, existing_names: &[&str]) -> String {
    let mut candidate = format!("{base} 副本");
    if !existing_names.contains(&candidate.as_str()) {
        return candidate;
    }

    let mut index = 2;
    loop {
        candidate = format!("{base} 副本 {index}");
        if !existing_names.contains(&candidate.as_str()) {
            return candidate;
        }
        index += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kt_config::AuthMethod;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn session_state_from_profile_initializes_ui_defaults() {
        let profile = SessionProfile {
            name: "Web Server 01".to_string(),
            group: None,
            params: ConnectParams {
                host: "10.0.1.10".to_string(),
                port: 22,
                user: "root".to_string(),
                auth: vec![AuthMethod::Password],
                vault_id: None,
                proxy_jump: None,
                proxy: kt_config::ProxyConfig::Direct,
                forward_agent: false,
            },
        };

        let state = session_state_from_profile(SessionId(7), &profile);

        assert_eq!(state.id, SessionId(7));
        assert_eq!(state.title, "Web Server 01");
        assert!(!state.connected);
        assert_eq!(state.sftp_path, ".");
        assert!(state.sftp_entries.is_empty());
        assert!(state.snapshot.is_none());
        assert!(state.connection_error.is_none());
        assert!(state.monitor.is_none());
        assert!(!state.monitor_loading);
        assert!(state.monitor_error.is_none());
    }

    #[test]
    fn session_dot_class_distinguishes_connecting_and_failed() {
        let profile = SessionProfile {
            name: "Web Server 01".to_string(),
            group: None,
            params: ConnectParams::new("10.0.1.10", "root"),
        };
        let mut state = session_state_from_profile(SessionId(7), &profile);

        assert_eq!(
            session_connection_status(&state),
            SessionConnectionStatus::Connecting
        );
        assert_eq!(session_dot_class(&state), "status-dot connecting");

        state.auth_challenge = Some(kt_core::AuthChallenge::Password {
            user: "root".to_string(),
            host: "10.0.1.10".to_string(),
            port: 22,
        });
        assert_eq!(
            session_connection_status(&state),
            SessionConnectionStatus::Authenticating
        );
        assert_eq!(session_dot_class(&state), "status-dot connecting");

        state.auth_challenge = None;
        state.connection_error = Some("连接失败".to_string());
        assert_eq!(
            session_connection_status(&state),
            SessionConnectionStatus::Disconnected
        );
        assert_eq!(session_dot_class(&state), "status-dot idle");

        state.connected = true;
        state.connection_error = None;
        assert_eq!(
            session_connection_status(&state),
            SessionConnectionStatus::Connected
        );
        assert_eq!(session_dot_class(&state), "status-dot online");
        assert_eq!(
            session_status_pill_class(SessionConnectionStatus::Connected),
            "status-pill connected"
        );
    }

    #[test]
    fn session_selectors_project_independent_ui_views() {
        let profile = SessionProfile {
            name: "Web Server 01".to_string(),
            group: None,
            params: ConnectParams::new("10.0.1.10", "root"),
        };
        let mut state = session_state_from_profile(SessionId(7), &profile);
        state.connected = true;
        state.sftp_path = "/var/log".to_string();
        state.sftp_loading = true;
        state.sftp_error = Some("读取失败".to_string());
        state.sftp_entries = vec![SftpEntry {
            name: "system.log".to_string(),
            is_dir: false,
            size: 42,
            modified: None,
            permissions: None,
            user: None,
            group: None,
            uid: None,
            gid: None,
        }];
        state.monitor_loading = true;
        state.monitor_error = Some("监控启动失败".to_string());

        let sessions = vec![state];
        let active = active_session(&sessions, Some(SessionId(7))).unwrap();
        let tabs = session_tab_views(&sessions);
        let sftp = active_sftp_view(Some(active)).unwrap();
        let monitor = active_monitor_view(Some(active)).unwrap();
        let status = status_bar_session_view(Some(active)).unwrap();

        assert_eq!(tabs[0].status, SessionConnectionStatus::Connected);
        assert_eq!(sftp.session_id, SessionId(7));
        assert_eq!(sftp.path, "/var/log");
        assert!(sftp.loading);
        assert_eq!(sftp.error.as_deref(), Some("读取失败"));
        assert_eq!(sftp.entries[0].name, "system.log");
        assert_eq!(monitor.session_id, SessionId(7));
        assert!(monitor.loading);
        assert_eq!(monitor.error.as_deref(), Some("监控启动失败"));
        assert!(!monitor.has_sample);
        assert_eq!(status.title, "Web Server 01");
        assert_eq!(status.status, SessionConnectionStatus::Connected);
    }

    #[test]
    fn auth_challenge_selector_returns_first_pending_prompt() {
        let first = SessionProfile {
            name: "A".to_string(),
            group: None,
            params: ConnectParams::new("a.test", "root"),
        };
        let second = SessionProfile {
            name: "B".to_string(),
            group: None,
            params: ConnectParams::new("b.test", "root"),
        };
        let mut a = session_state_from_profile(SessionId(1), &first);
        let mut b = session_state_from_profile(SessionId(2), &second);
        b.auth_challenge = Some(kt_core::AuthChallenge::Password {
            user: "root".to_string(),
            host: "b.test".to_string(),
            port: 22,
        });
        a.connection_error = Some("旧错误".to_string());

        let selected = auth_challenge_view(&[a, b]).unwrap();

        assert_eq!(selected.0, SessionId(2));
        assert_eq!(selected.1, "B");
    }

    #[test]
    fn sidebar_drag_is_clamped() {
        assert_eq!(clamp_dimension(100.0, 176.0, 320.0), 176.0);
        assert_eq!(clamp_dimension(400.0, 176.0, 320.0), 320.0);
        assert_eq!(clamp_dimension(250.0, 176.0, 320.0), 250.0);
    }

    #[test]
    fn group_profiles_splits_by_group_field() {
        let profiles = vec![
            SessionProfile {
                name: "A".to_string(),
                group: Some("Web".to_string()),
                params: ConnectParams {
                    host: "a.test".to_string(),
                    port: 22,
                    user: "root".to_string(),
                    auth: vec![AuthMethod::Password],
                    vault_id: None,
                    proxy_jump: None,
                    proxy: kt_config::ProxyConfig::Direct,
                    forward_agent: false,
                },
            },
            SessionProfile {
                name: "B".to_string(),
                group: None,
                params: ConnectParams {
                    host: "b.test".to_string(),
                    port: 22,
                    user: "root".to_string(),
                    auth: vec![AuthMethod::Password],
                    vault_id: None,
                    proxy_jump: None,
                    proxy: kt_config::ProxyConfig::Direct,
                    forward_agent: false,
                },
            },
        ];

        let grouped = group_profiles(&profiles, &["Cache".to_string()]);
        assert_eq!(grouped.len(), 3);
        assert_eq!(grouped[0].0, "Cache");
        assert!(grouped[0].1.is_empty());
        assert_eq!(grouped[1].0, DEFAULT_GROUP_NAME);
        assert_eq!(grouped[1].1.len(), 1);
        assert_eq!(grouped[2].0, "Web");
        assert_eq!(grouped[2].1.len(), 1);
    }

    #[test]
    fn ui_connect_params_merge_ssh_config_proxy_jump() {
        let dir = std::env::temp_dir().join(format!(
            "kitonyterms-ui-ssh-config-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(
            b"Host prod\n    HostName 10.0.0.8\n    User deploy\n    ProxyJump ops@bastion:2222\n",
        )
        .unwrap();

        let params = merge_ssh_config_from_path(ConnectParams::new("prod", "root"), true, &path);

        assert_eq!(params.host, "10.0.0.8");
        assert_eq!(params.user, "root");
        assert_eq!(params.proxy_jump.as_deref(), Some("ops@bastion:2222"));
        assert_eq!(params.effective_vault_id(), "root@prod:22");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unique_copy_name_avoids_existing_names() {
        assert_eq!(unique_copy_name("A", &["A"]), "A 副本");
        assert_eq!(
            unique_copy_name("A", &["A", "A 副本", "A 副本 2"]),
            "A 副本 3"
        );
    }
}
