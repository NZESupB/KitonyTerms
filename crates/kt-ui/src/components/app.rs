//! 主应用组件 —— 深色工作台。
//!
//! 布局参照截图优化:
//! - 左侧: 分组连接树 + SFTP 目录树 + 底部设置
//! - 中央: 会话标签 + 终端
//! - 底部: 系统监控横条 + 精简状态栏

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dioxus::prelude::*;
use kt_config::{
    lookup_ssh_config, normalize_group_name, AppLanguage, ConnectParams, KnownHostCheck,
    SessionProfile,
};
use kt_core::ssh::HostKeyDecision;
use kt_core::{
    AuthProvider, HostKeyVerifier, PtySize, SessionId, SessionManager, SftpEntry, SftpRequest,
    ToCore,
};

use crate::components::dialog::ConnectionDialog;
use crate::components::icons::{AppLogo, Icon};
use crate::components::monitor::MonitorPanel;
use crate::components::sftp::{display_path, join_path, parent_path, request_directory};
use crate::components::terminal::{SnapshotWrapper, Terminal};
use crate::i18n::texts;
use crate::state::{AppState, SessionState};
use crate::store::Store;

/// 全局 Store(只初始化一次)。
static GLOBAL_STORE: OnceLock<Arc<Store>> = OnceLock::new();

/// 全局 AppState(只初始化一次)。
static GLOBAL_STATE: OnceLock<Arc<Mutex<AppState>>> = OnceLock::new();

const SIDEBAR_DEFAULT_WIDTH: f64 = 220.0;
const SIDEBAR_MIN_WIDTH: f64 = 176.0;
const SIDEBAR_MAX_WIDTH: f64 = 320.0;
const SFTP_DEFAULT_HEIGHT: f64 = 320.0;
const SFTP_MIN_HEIGHT: f64 = 120.0;
const SFTP_MAX_HEIGHT: f64 = 420.0;
const DEFAULT_GROUP_NAME: &str = "NoBrand";

#[derive(Clone, Copy, Debug, PartialEq)]
enum ResizeDrag {
    SidebarWidth { start_x: f64, start_width: f64 },
    SftpHeight { start_y: f64, start_height: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SplitMode {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, PartialEq)]
enum ContextMenuTarget {
    Profile(String),
    Group(String),
    SftpEntry(SftpEntryContext),
    SftpBlank { session_id: SessionId, path: String },
}

#[derive(Clone, Debug, PartialEq)]
struct ContextMenuState {
    target: ContextMenuTarget,
    x: f64,
    y: f64,
}

#[derive(Clone, Debug, PartialEq)]
struct SftpEntryContext {
    session_id: SessionId,
    base_path: String,
    entry: SftpEntry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ExternalEditStatus {
    Downloading,
    Watching,
    PromptPending,
    UploadingOnce,
    UploadingAuto,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ExternalEditSyncMode {
    Ask,
    AutoUpload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ExternalEdit {
    id: u64,
    session_id: SessionId,
    remote_path: String,
    local_path: PathBuf,
    file_name: String,
    after_revision: u64,
    status: ExternalEditStatus,
    sync_mode: ExternalEditSyncMode,
    last_seen_modified: Option<SystemTime>,
    pending_modified: Option<SystemTime>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ExternalEditAction {
    OpenLocal {
        edit_id: u64,
        path: PathBuf,
        file_name: String,
    },
    Upload {
        session_id: SessionId,
        local_path: PathBuf,
        remote_path: String,
        file_name: String,
    },
    DeleteLocal(PathBuf),
    UploadCompleted {
        file_name: String,
    },
}

/// 获取全局 state(供其他模块使用)。
pub fn get_state() -> &'static Arc<Mutex<AppState>> {
    GLOBAL_STATE.get_or_init(|| {
        tracing::info!("初始化 SessionManager");

        let store = get_store();
        let manager = SessionManager::spawn(
            Arc::new(KnownHostsVerifier {
                store: store.clone(),
            }),
            Arc::new(StoreAuthFactory {
                store: store.clone(),
            }),
        )
        .expect("无法启动 SessionManager");

        Arc::new(Mutex::new(AppState::new(manager)))
    })
}

fn get_store() -> &'static Arc<Store> {
    GLOBAL_STORE.get_or_init(|| Arc::new(Store::load().expect("无法加载配置")))
}

fn show_context_menu(mut menu_signal: Signal<Option<ContextMenuState>>, menu: ContextMenuState) {
    menu_signal.set(None);
    spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
        menu_signal.set(Some(menu));
    });
}

/// AuthProvider 实现(从 Store 读取密码)。
struct StoreAuthProvider {
    store: Arc<Store>,
    vault_id: String,
}

impl AuthProvider for StoreAuthProvider {
    fn password(&mut self, user: &str, host: &str, port: u16) -> Option<String> {
        let scoped_vault_id = format!("{user}@{host}:{port}");
        self.store
            .get_secret(&scoped_vault_id)
            .or_else(|| self.store.get_secret(&self.vault_id))
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

/// AuthProviderFactory 实现。
struct StoreAuthFactory {
    store: Arc<Store>,
}

impl kt_core::session::AuthProviderFactory for StoreAuthFactory {
    fn create(&self, _id: kt_core::SessionId, params: &ConnectParams) -> Box<dyn AuthProvider> {
        Box::new(StoreAuthProvider {
            store: self.store.clone(),
            vault_id: params.effective_vault_id(),
        })
    }
}

/// 持久化 known_hosts 校验器。
struct KnownHostsVerifier {
    store: Arc<Store>,
}

impl HostKeyVerifier for KnownHostsVerifier {
    fn verify(
        &self,
        host: &str,
        port: u16,
        _key: &russh::keys::PublicKey,
        fingerprint: &str,
    ) -> HostKeyDecision {
        match self.store.check_or_trust_host_key(host, port, fingerprint) {
            Ok(KnownHostCheck::Trusted | KnownHostCheck::NewlyTrusted) => HostKeyDecision::Accept,
            Ok(KnownHostCheck::Changed { expected, actual }) => {
                tracing::error!(
                    "主机密钥已变化: {}:{}, stored={}, received={}",
                    host,
                    port,
                    expected,
                    actual
                );
                HostKeyDecision::Reject
            }
            Err(err) => {
                tracing::error!("known_hosts 校验失败: {}:{} {}", host, port, err);
                HostKeyDecision::Reject
            }
        }
    }
}

#[component]
pub fn App() -> Element {
    let store = get_store();
    let state = get_state();

    let mut show_dialog = use_signal(|| false);
    let mut dialog_mode = use_signal(|| "new".to_string());
    let mut edit_original_name = use_signal(String::new);
    let mut edit_name = use_signal(String::new);
    let mut edit_host = use_signal(String::new);
    let mut edit_port = use_signal(|| String::from("22"));
    let mut edit_user = use_signal(String::new);
    let mut edit_group = use_signal(String::new);
    let mut edit_password = use_signal(String::new);
    let mut edit_proxy_jump = use_signal(String::new);
    let mut edit_use_agent = use_signal(|| false);
    let mut edit_forward_agent = use_signal(|| false);

    let mut settings = use_signal(|| store.settings());
    let mut show_settings = use_signal(|| false);
    let mut show_group_dialog = use_signal(|| false);
    let mut group_dialog_mode = use_signal(|| "new".to_string());
    let mut group_dialog_name = use_signal(String::new);
    let mut group_dialog_original = use_signal(String::new);
    let mut show_sftp_name_dialog = use_signal(|| false);
    let mut sftp_name_dialog_mode = use_signal(String::new);
    let mut sftp_name_dialog_session = use_signal(|| None::<SessionId>);
    let mut sftp_name_dialog_base_path = use_signal(String::new);
    let mut sftp_name_dialog_target_path = use_signal(String::new);
    let mut sftp_name_dialog_is_dir = use_signal(|| false);
    let mut sftp_name_dialog_value = use_signal(String::new);
    let mut external_edits = use_signal(Vec::<ExternalEdit>::new);
    let mut external_edit_notice = use_signal(|| None::<String>);
    let mut next_external_edit_id = use_signal(|| 1u64);
    let mut active_session_id = use_signal(|| None::<SessionId>);
    let mut all_sessions = use_signal(Vec::<SessionState>::new);
    let mut saved_tick = use_signal(|| 0u64);
    let mut sidebar_width = use_signal(|| SIDEBAR_DEFAULT_WIDTH);
    let mut sftp_height = use_signal(|| None::<f64>);
    let mut active_resize = use_signal(|| None::<ResizeDrag>);
    let mut context_menu = use_signal(|| None::<ContextMenuState>);
    let mut split_mode = use_signal(|| None::<SplitMode>);

    use_future(move || async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(16)).await;
            if let Ok(mut app_state) = state.lock() {
                app_state.pump_events();
            }
        }
    });

    use_effect(move || {
        spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                if let Ok(s) = state.lock() {
                    let sessions: Vec<SessionState> = s.sessions.values().cloned().collect();
                    all_sessions.set(sessions.clone());

                    if active_session_id().is_none() && !sessions.is_empty() {
                        active_session_id.set(Some(sessions[0].id));
                    }
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
                    match action {
                        ExternalEditAction::OpenLocal {
                            edit_id,
                            path,
                            file_name,
                        } => {
                            if let Err(e) = open_local_file(&path) {
                                tracing::error!("打开外部编辑器失败: {}", e);
                                external_edit_notice.set(Some(format!(
                                    "{} {}: {}",
                                    texts(settings.peek().language).sftp.edit_status_open_failed,
                                    file_name,
                                    e
                                )));
                                let mut edits = external_edits.peek().clone();
                                edits.retain(|edit| edit.id != edit_id);
                                external_edits.set(edits);
                                let _ = std::fs::remove_file(path);
                            } else {
                                external_edit_notice.set(Some(format!(
                                    "{} {}",
                                    texts(settings.peek().language).sftp.edit_status_opened,
                                    file_name
                                )));
                            }
                        }
                        ExternalEditAction::Upload {
                            session_id,
                            local_path,
                            remote_path,
                            file_name,
                        } => {
                            external_edit_notice.set(Some(format!(
                                "{} {}",
                                texts(settings.peek().language).sftp.edit_status_uploading,
                                file_name
                            )));
                            send_sftp_request(
                                state.clone(),
                                session_id,
                                SftpRequest::Upload {
                                    local: local_path,
                                    remote: remote_path,
                                },
                            );
                        }
                        ExternalEditAction::DeleteLocal(path) => {
                            let _ = std::fs::remove_file(path);
                        }
                        ExternalEditAction::UploadCompleted { file_name } => {
                            external_edit_notice.set(Some(format!(
                                "{} {}",
                                texts(settings.peek().language).sftp.edit_status_uploaded,
                                file_name
                            )));
                        }
                    }
                }
            }
        });
    });

    let active_session =
        move || active_session_id().and_then(|id| all_sessions().into_iter().find(|s| s.id == id));

    let saved_profiles = {
        let _ = saved_tick();
        store.saved_sessions()
    };
    let saved_groups = {
        let _ = saved_tick();
        store.saved_groups()
    };

    let language = settings().language;
    let t = texts(language).app;
    let sftp_t = texts(language).sftp;
    let sidebar_style = format!(
        "width: {:.0}px; flex-basis: {:.0}px;",
        sidebar_width(),
        sidebar_width()
    );
    let sftp_style = sftp_height()
        .map(|height| format!("height: {height:.0}px; flex-basis: {height:.0}px;"))
        .unwrap_or_else(|| "flex: 1 1 0; min-height: 0;".to_string());
    let window_class = match active_resize() {
        Some(ResizeDrag::SidebarWidth { .. }) => "kt-window is-resizing is-resizing-x",
        Some(ResizeDrag::SftpHeight { .. }) => "kt-window is-resizing is-resizing-y",
        None => "kt-window",
    };

    let grouped = group_profiles(&saved_profiles, &saved_groups);
    let active_status_session = active_session();
    let external_edit_status = external_edit_status_text(
        &external_edits(),
        active_status_session.as_ref(),
        external_edit_notice().as_deref(),
        language,
    );

    rsx! {
        style { {include_str!("../assets/app.css")} }

        div {
            class: "{window_class}",
            onmousemove: move |evt| {
                match active_resize() {
                    Some(ResizeDrag::SidebarWidth { start_x, start_width }) => {
                        let delta = evt.client_coordinates().x - start_x;
                        sidebar_width.set(clamp_dimension(
                            start_width + delta,
                            SIDEBAR_MIN_WIDTH,
                            SIDEBAR_MAX_WIDTH,
                        ));
                    }
                    Some(ResizeDrag::SftpHeight { start_y, start_height }) => {
                        let delta = start_y - evt.client_coordinates().y;
                        sftp_height.set(Some(clamp_dimension(
                            start_height + delta,
                            SFTP_MIN_HEIGHT,
                            SFTP_MAX_HEIGHT,
                        )));
                    }
                    None => {}
                }
            },
            onmouseup: move |_| active_resize.set(None),
            onmouseleave: move |_| active_resize.set(None),
            onclick: move |_| context_menu.set(None),
            oncontextmenu: move |evt| {
                evt.prevent_default();
                context_menu.set(None);
            },

            div {
                class: "kt-content",

                // 左侧边栏
                aside {
                    class: "sidebar",
                    style: "{sidebar_style}",

                    // 分组区域
                    div {
                        class: "sidebar-section sidebar-groups",

                        div {
                            class: "sidebar-section-head",
                            Icon { name: "chevron-down" }
                            span { "{t.groups}" }
                            button {
                                class: "sidebar-add-btn",
                                title: "{t.new_group}",
                                onclick: move |evt| {
                                    evt.stop_propagation();
                                    group_dialog_mode.set("new".to_string());
                                    group_dialog_original.set(String::new());
                                    group_dialog_name.set(String::new());
                                    show_group_dialog.set(true);
                                },
                                Icon { name: "folder" }
                            }
                            button {
                                class: "sidebar-add-btn",
                                title: "{t.new_connection}",
                                onclick: move |_| {
                                    dialog_mode.set("new".to_string());
                                    edit_original_name.set(String::new());
                                    edit_name.set(String::new());
                                    edit_host.set(String::new());
                                    edit_port.set("22".to_string());
                                    edit_user.set(String::new());
                                    edit_group.set(String::new());
                                    edit_password.set(String::new());
                                    edit_proxy_jump.set(String::new());
                                    edit_use_agent.set(false);
                                    edit_forward_agent.set(false);
                                    show_dialog.set(true);
                                },
                                Icon { name: "add" }
                            }
                        }

                        div {
                            class: "connection-tree",

                            if saved_profiles.is_empty() {
                                div {
                                    class: "empty-resource",
                                    strong { "{t.no_saved_connections}" }
                                    p { "{t.saved_connections_hint}" }
                                }
                            }

                            for (group_name, profiles) in grouped.iter() {
                                div {
                                    class: "tree-group",
                                    div {
                                        class: "tree-group-title",
                                        oncontextmenu: {
                                            let group_name = group_name.clone();
                                            move |evt| {
                                                evt.prevent_default();
                                                evt.stop_propagation();
                                                show_context_menu(context_menu, ContextMenuState {
                                                    target: ContextMenuTarget::Group(group_name.clone()),
                                                    x: evt.client_coordinates().x,
                                                    y: evt.client_coordinates().y,
                                                });
                                            }
                                        },
                                        Icon { name: "chevron-down" }
                                        "{group_name}"
                                    }

                                    for profile in profiles {
                                        ConnectionCard {
                                            key: "saved-{profile.name}",
                                            profile: profile.clone(),
                                            active: active_session().map(|s| s.title == profile.name).unwrap_or(false),
                                            language,
                                            on_connect: {
                                                let profile = profile.clone();
                                                move |_| {
                                                    let params = params_with_ssh_config(
                                                        profile.params.clone(),
                                                        settings.peek().use_ssh_config,
                                                    );
                                                    if let Ok(mut app_state) = state.lock() {
                                                        let id = app_state.next_session_id();
                                                        app_state.sessions.insert(
                                                            id,
                                                            session_state_from_profile(id, &profile),
                                                        );
                                                        app_state.manager.send(ToCore::Connect {
                                                            id,
                                                            params: Box::new(params),
                                                            pty: PtySize { cols: 100, rows: 30 },
                                                        });
                                                        active_session_id.set(Some(id));
                                                    }
                                                }
                                            },
                                            on_edit: {
                                                let profile = profile.clone();
                                                move |_| {
                                                    dialog_mode.set("edit".to_string());
                                                    edit_original_name.set(profile.name.clone());
                                                    edit_name.set(profile.name.clone());
                                                    edit_host.set(profile.params.host.clone());
                                                    edit_port.set(profile.params.port.to_string());
                                                    edit_user.set(profile.params.user.clone());
                                                    edit_group.set(profile.group.clone().unwrap_or_default());
                                                    edit_password.set(String::new());
                                                    edit_proxy_jump.set(profile.params.proxy_jump.clone().unwrap_or_default());
                                                    edit_use_agent.set(profile.params.auth.contains(&kt_config::AuthMethod::Agent));
                                                    edit_forward_agent.set(profile.params.forward_agent);
                                                    show_dialog.set(true);
                                                }
                                            },
                                            on_delete: {
                                                let name = profile.name.clone();
                                                move |_| {
                                                    if let Err(e) = store.delete_session(&name) {
                                                        tracing::error!("删除失败: {}", e);
                                                    } else {
                                                        saved_tick.set(saved_tick() + 1);
                                                    }
                                                }
                                            },
                                            on_copy: {
                                                let profile = profile.clone();
                                                let saved_profiles = saved_profiles.clone();
                                                move |_| {
                                                    let duplicate = duplicate_profile(&profile, &saved_profiles);
                                                    if let Err(e) = store.save_session(duplicate) {
                                                        tracing::error!("复制连接失败: {}", e);
                                                    } else {
                                                        saved_tick.set(saved_tick() + 1);
                                                    }
                                                }
                                            },
                                            on_context_menu: {
                                                let profile_name = profile.name.clone();
                                                move |point: (f64, f64)| {
                                                    show_context_menu(context_menu, ContextMenuState {
                                                        target: ContextMenuTarget::Profile(profile_name.clone()),
                                                        x: point.0,
                                                        y: point.1,
                                                    });
                                                }
                                            },
                                        }
                                    }
                                }
                            }
                        }
                    }

                    div {
                        class: if matches!(active_resize(), Some(ResizeDrag::SftpHeight { .. })) { "sidebar-y-splitter is-active" } else { "sidebar-y-splitter" },
                        title: "{t.resize_sftp}",
                        onmousedown: move |evt| {
                            evt.stop_propagation();
                            evt.prevent_default();
                            active_resize.set(Some(ResizeDrag::SftpHeight {
                                start_y: evt.client_coordinates().y,
                                start_height: sftp_height().unwrap_or(SFTP_DEFAULT_HEIGHT),
                            }));
                        },
                    }

                    // SFTP 区域
                    div {
                        class: "sidebar-section sidebar-sftp",
                        style: "{sftp_style}",

                        div {
                            class: "sidebar-section-head",
                            Icon { name: "chevron-down" }
                            span { "SFTP" }
                            if let Some(sess) = active_session() {
                                button {
                                    class: "sidebar-add-btn",
                                    title: "{sftp_t.refresh}",
                                    onclick: {
                                        let sess = sess.clone();
                                        move |evt| {
                                            evt.stop_propagation();
                                            if let Err(e) = request_directory(
                                                state.clone(),
                                                sess.id,
                                                sess.sftp_path.clone(),
                                                language,
                                            ) {
                                                tracing::error!("SFTP 刷新失败: {}", e);
                                            }
                                        }
                                    },
                                    Icon { name: "refresh" }
                                }
                            }
                        }

                        if let Some(sess) = active_session() {
                            SidebarSftpTree {
                                key: "sidebar-sftp-{sess.id.0}",
                                session_id: sess.id,
                                connected: sess.connected,
                                path: sess.sftp_path.clone(),
                                entries: sess.sftp_entries.clone(),
                                loading: sess.sftp_loading,
                                error: sess.sftp_error.clone(),
                                language,
                                on_context_menu: move |menu| show_context_menu(context_menu, menu),
                            }
                        } else {
                            div {
                                class: "sidebar-sftp-tree",
                                div { class: "sftp-empty", "{t.ready_hint}" }
                            }
                        }
                    }

                    // 底部设置
                    div {
                        class: "sidebar-footer",
                        button {
                            class: "sidebar-settings-btn",
                            onclick: move |_| show_settings.set(true),
                            Icon { name: "settings" }
                            span { "{t.settings}" }
                        }
                    }
                }

                // 分隔条
                div {
                    class: if active_resize().is_some() { "splitter is-active" } else { "splitter" },
                    title: "{t.resize_explorer}",
                    onmousedown: move |evt| {
                        evt.stop_propagation();
                        evt.prevent_default();
                        active_resize.set(Some(ResizeDrag::SidebarWidth {
                            start_x: evt.client_coordinates().x,
                            start_width: sidebar_width(),
                        }));
                    },
                }

                // 主内容区
                div {
                    class: "main-column",

                    // 终端区域
                    section {
                        class: "terminal-panel",

                        // 标签栏
                        div {
                            class: "session-tabs",

                            for sess in all_sessions() {
                                div {
                                    key: "tab-{sess.id.0}",
                                    class: if active_session_id() == Some(sess.id) { "session-tab is-active" } else { "session-tab" },
                                    onclick: {
                                        let id = sess.id;
                                        move |_| active_session_id.set(Some(id))
                                    },

                                    span {
                                        class: session_dot_class(&sess),
                                    }
                                    span { class: "tab-title", "{sess.title}" }
                                    button {
                                        class: "tab-close",
                                        title: "{t.close_session}",
                                        onclick: {
                                            let id = sess.id;
                                            move |evt| {
                                                evt.stop_propagation();
                                                if let Ok(mut app_state) = state.lock() {
                                                    app_state.manager.send(ToCore::Disconnect { id });
                                                    app_state.sessions.remove(&id);
                                                    if active_session_id() == Some(id) {
                                                        let next = app_state.sessions.keys().next().copied();
                                                        active_session_id.set(next);
                                                    }
                                                }
                                            }
                                        },
                                        Icon { name: "close" }
                                    }
                                }
                            }

                            button {
                                class: "new-tab-button",
                                title: "{t.new_connection}",
                                onclick: move |_| {
                                    dialog_mode.set("new".to_string());
                                    edit_original_name.set(String::new());
                                    edit_name.set(String::new());
                                    edit_host.set(String::new());
                                    edit_port.set("22".to_string());
                                    edit_user.set(String::new());
                                    edit_group.set(String::new());
                                    edit_password.set(String::new());
                                    edit_proxy_jump.set(String::new());
                                    edit_use_agent.set(false);
                                    edit_forward_agent.set(false);
                                    show_dialog.set(true);
                                },
                                Icon { name: "add" }
                            }
                        }

                        // 终端工具栏
                        div {
                            class: "terminal-toolbar",

                            div {
                                class: "breadcrumb",
                                span { class: "protocol-badge", "ssh" }
                                span { class: "chevron", "›" }
                                if let Some(sess) = active_session() {
                                    span {
                                        class: "host-pill",
                                        span { class: session_dot_class(&sess) }
                                        "{sess.title}"
                                    }
                                } else {
                                    span { class: "host-pill muted", "{t.disconnected}" }
                                }
                            }

                            div { class: "toolbar-spacer" }
                            button {
                                class: "icon-button slim",
                                title: "{t.split}",
                                onclick: move |_| split_mode.set(None),
                                Icon { name: "split" }
                            }
                            button {
                                class: "icon-button slim",
                                title: "{t.split_horizontal}",
                                onclick: move |_| split_mode.set(Some(SplitMode::Horizontal)),
                                Icon { name: "split-horizontal" }
                            }
                            button {
                                class: "icon-button slim",
                                title: "{t.split_vertical}",
                                onclick: move |_| split_mode.set(Some(SplitMode::Vertical)),
                                Icon { name: "split-vertical" }
                            }
                            button { class: "icon-button slim", title: "{t.clear}", Icon { name: "clear" } }
                            button { class: "icon-button slim", title: "{t.more}", Icon { name: "more" } }
                        }

                        // 终端内容
                        div {
                            class: match split_mode() {
                                Some(SplitMode::Horizontal) => "terminal-body is-split-horizontal",
                                Some(SplitMode::Vertical) => "terminal-body is-split-vertical",
                                None => "terminal-body",
                            },

                            if let Some(sess) = active_session() {
                                if let Some(snapshot) = sess.snapshot.clone() {
                                    div {
                                        class: "terminal-pane",
                                        Terminal {
                                            snapshot: SnapshotWrapper(snapshot.clone()),
                                            session_id: sess.id,
                                            pane_id: "primary".to_string(),
                                            trigger_highlights: settings().trigger_highlights,
                                        }
                                    }
                                    if split_mode().is_some() {
                                        div {
                                            class: "terminal-pane",
                                            Terminal {
                                                snapshot: SnapshotWrapper(snapshot),
                                                session_id: sess.id,
                                                pane_id: "secondary".to_string(),
                                                trigger_highlights: settings().trigger_highlights,
                                            }
                                        }
                                    }
                                } else {
                                    TerminalPlaceholder {
                                        connected: sess.connected,
                                        title: sess.title.clone(),
                                        error: sess.connection_error.clone(),
                                        language,
                                    }
                                }
                            } else {
                                EmptyWorkbench { language }
                            }
                        }
                    }

                    // 底部监控区
                    div {
                        class: "monitor-dock",
                        if let Some(sess) = active_session() {
                            MonitorPanel {
                                key: "monitor-{sess.id.0}",
                                session_id: sess.id,
                                language,
                            }
                        } else {
                            MonitorPlaceholder { language }
                        }
                    }
                }
            }

            // 状态栏
            footer {
                class: "status-bar",
                if let Some(sess) = active_session() {
                    span {
                        class: if sess.connected { "status-pill connected" } else { "status-pill pending" },
                        if sess.connected {
                            "{t.connected}"
                        } else if sess.connection_error.is_some() {
                            "{t.disconnected}"
                        } else {
                            "{t.connecting}"
                        }
                    }
                    span { "{sess.title}" }
                    if let Some(status) = external_edit_status.clone() {
                        span { class: "status-detail", "{status}" }
                    }
                } else {
                    span { class: "status-pill pending", "{t.ready}" }
                    span { "{t.ready_hint}" }
                    if let Some(status) = external_edit_status.clone() {
                        span { class: "status-detail", "{status}" }
                    }
                }
            }

            if let Some(menu) = context_menu() {
                ContextMenu {
                    menu,
                    language,
                    on_profile_edit: {
                        let saved_profiles = saved_profiles.clone();
                        move |name: String| {
                            if let Some(profile) = saved_profiles.iter().find(|profile| profile.name == name) {
                                dialog_mode.set("edit".to_string());
                                edit_original_name.set(profile.name.clone());
                                edit_name.set(profile.name.clone());
                                edit_host.set(profile.params.host.clone());
                                edit_port.set(profile.params.port.to_string());
                                edit_user.set(profile.params.user.clone());
                                edit_group.set(profile.group.clone().unwrap_or_default());
                                edit_password.set(String::new());
                                edit_proxy_jump.set(profile.params.proxy_jump.clone().unwrap_or_default());
                                edit_use_agent.set(profile.params.auth.contains(&kt_config::AuthMethod::Agent));
                                edit_forward_agent.set(profile.params.forward_agent);
                                show_dialog.set(true);
                            }
                            context_menu.set(None);
                        }
                    },
                    on_profile_delete: move |name: String| {
                        if let Err(e) = store.delete_session(&name) {
                            tracing::error!("删除失败: {}", e);
                        } else {
                            saved_tick.set(saved_tick() + 1);
                        }
                        context_menu.set(None);
                    },
                    on_profile_copy: {
                        let saved_profiles = saved_profiles.clone();
                        move |name: String| {
                            if let Some(profile) = saved_profiles.iter().find(|profile| profile.name == name) {
                                let duplicate = duplicate_profile(profile, &saved_profiles);
                                if let Err(e) = store.save_session(duplicate) {
                                    tracing::error!("复制连接失败: {}", e);
                                } else {
                                    saved_tick.set(saved_tick() + 1);
                                }
                            }
                            context_menu.set(None);
                        }
                    },
                    on_group_new: move |_| {
                        group_dialog_mode.set("new".to_string());
                        group_dialog_original.set(String::new());
                        group_dialog_name.set(String::new());
                        show_group_dialog.set(true);
                        context_menu.set(None);
                    },
                    on_group_rename: move |name: String| {
                        group_dialog_mode.set("rename".to_string());
                        group_dialog_original.set(name.clone());
                        group_dialog_name.set(if name == DEFAULT_GROUP_NAME { String::new() } else { name });
                        show_group_dialog.set(true);
                        context_menu.set(None);
                    },
                    on_group_delete: move |name: String| {
                        if let Err(e) = store.delete_group(&name) {
                            tracing::error!("删除分组失败: {}", e);
                        } else {
                            saved_tick.set(saved_tick() + 1);
                        }
                        context_menu.set(None);
                    },
                    on_sftp_open: {
                        let state = state.clone();
                        move |ctx: SftpEntryContext| {
                            if ctx.entry.is_dir {
                                let next = join_path(&ctx.base_path, &ctx.entry.name);
                                if let Err(e) = request_directory(state.clone(), ctx.session_id, next, language) {
                                    tracing::error!("SFTP 打开目录失败: {}", e);
                                }
                            }
                            context_menu.set(None);
                        }
                    },
                    on_sftp_refresh: {
                        let state = state.clone();
                        move |(session_id, path): (SessionId, String)| {
                            if let Err(e) = request_directory(state.clone(), session_id, path, language) {
                                tracing::error!("SFTP 刷新失败: {}", e);
                            }
                            context_menu.set(None);
                        }
                    },
                    on_sftp_mkdir: move |(session_id, path): (SessionId, String)| {
                        sftp_name_dialog_mode.set("mkdir".to_string());
                        sftp_name_dialog_session.set(Some(session_id));
                        sftp_name_dialog_base_path.set(path);
                        sftp_name_dialog_target_path.set(String::new());
                        sftp_name_dialog_is_dir.set(true);
                        sftp_name_dialog_value.set(String::new());
                        show_sftp_name_dialog.set(true);
                        context_menu.set(None);
                    },
                    on_sftp_rename: move |ctx: SftpEntryContext| {
                        sftp_name_dialog_mode.set("rename".to_string());
                        sftp_name_dialog_session.set(Some(ctx.session_id));
                        sftp_name_dialog_base_path.set(ctx.base_path.clone());
                        sftp_name_dialog_target_path.set(join_path(&ctx.base_path, &ctx.entry.name));
                        sftp_name_dialog_is_dir.set(ctx.entry.is_dir);
                        sftp_name_dialog_value.set(ctx.entry.name.clone());
                        show_sftp_name_dialog.set(true);
                        context_menu.set(None);
                    },
                    on_sftp_delete: {
                        let state = state.clone();
                        move |ctx: SftpEntryContext| {
                            let path = join_path(&ctx.base_path, &ctx.entry.name);
                            send_sftp_request(
                                state.clone(),
                                ctx.session_id,
                                SftpRequest::Remove {
                                    path,
                                    is_dir: ctx.entry.is_dir,
                                },
                            );
                            context_menu.set(None);
                        }
                    },
                    on_sftp_external_edit: {
                        let state = state.clone();
                        move |ctx: SftpEntryContext| {
                            if ctx.entry.is_dir {
                                context_menu.set(None);
                                return;
                            }
                            let remote_path = join_path(&ctx.base_path, &ctx.entry.name);
                            let local_path = external_edit_local_path(ctx.session_id, &remote_path);
                            if let Some(parent) = local_path.parent() {
                                if let Err(e) = std::fs::create_dir_all(parent) {
                                    tracing::error!("创建本地编辑目录失败: {}", e);
                                    context_menu.set(None);
                                    return;
                                }
                            }
                            let after_revision = latest_sftp_completion_revision(state.clone(), ctx.session_id);
                            let edit = ExternalEdit {
                                id: next_external_edit_id(),
                                session_id: ctx.session_id,
                                remote_path: remote_path.clone(),
                                local_path: local_path.clone(),
                                file_name: ctx.entry.name.clone(),
                                after_revision,
                                status: ExternalEditStatus::Downloading,
                                sync_mode: ExternalEditSyncMode::Ask,
                                last_seen_modified: None,
                                pending_modified: None,
                            };
                            next_external_edit_id.set(next_external_edit_id() + 1);
                            let mut edits = external_edits.peek().clone();
                            edits.push(edit);
                            external_edits.set(edits);
                            send_sftp_request(
                                state.clone(),
                                ctx.session_id,
                                SftpRequest::Download {
                                    remote: remote_path,
                                    local: local_path,
                                },
                            );
                            context_menu.set(None);
                        }
                    },
                    on_copy_text: move |value: String| {
                        copy_to_clipboard(&value);
                        context_menu.set(None);
                    },
                }
                }
            }

            if let Some(edit) = external_edits()
                .into_iter()
                .find(|edit| edit.status == ExternalEditStatus::PromptPending)
            {
                ExternalEditSaveDialog {
                    edit,
                    language,
                    on_upload_once: {
                        let state = state.clone();
                        move |edit_id: u64| {
                            let mut edits = external_edits.peek().clone();
                            if let Some(edit) = edits.iter_mut().find(|edit| edit.id == edit_id) {
                                edit.status = ExternalEditStatus::UploadingOnce;
                                edit.after_revision = latest_sftp_completion_revision(state.clone(), edit.session_id);
                                edit.last_seen_modified = edit
                                    .pending_modified
                                    .or_else(|| local_file_modified(&edit.local_path));
                                edit.pending_modified = None;
                                external_edit_notice.set(Some(format!(
                                    "{} {}",
                                    texts(language).sftp.edit_status_uploading,
                                    edit.file_name
                                )));
                                send_sftp_request(
                                    state.clone(),
                                    edit.session_id,
                                    SftpRequest::Upload {
                                        local: edit.local_path.clone(),
                                        remote: edit.remote_path.clone(),
                                    },
                                );
                            }
                            external_edits.set(edits);
                        }
                    },
                    on_auto_upload: {
                        let state = state.clone();
                        move |edit_id: u64| {
                            let mut edits = external_edits.peek().clone();
                            if let Some(edit) = edits.iter_mut().find(|edit| edit.id == edit_id) {
                                edit.status = ExternalEditStatus::UploadingAuto;
                                edit.sync_mode = ExternalEditSyncMode::AutoUpload;
                                edit.after_revision = latest_sftp_completion_revision(state.clone(), edit.session_id);
                                edit.last_seen_modified = edit
                                    .pending_modified
                                    .or_else(|| local_file_modified(&edit.local_path));
                                edit.pending_modified = None;
                                external_edit_notice.set(Some(format!(
                                    "{} {}",
                                    texts(language).sftp.edit_status_uploading,
                                    edit.file_name
                                )));
                                send_sftp_request(
                                    state.clone(),
                                    edit.session_id,
                                    SftpRequest::Upload {
                                        local: edit.local_path.clone(),
                                        remote: edit.remote_path.clone(),
                                    },
                                );
                            }
                            external_edits.set(edits);
                        }
                    },
                    on_ignore: move |edit_id: u64| {
                        let mut edits = external_edits.peek().clone();
                        if let Some(edit) = edits.iter().find(|edit| edit.id == edit_id) {
                            external_edit_notice.set(Some(format!(
                                "{} {}",
                                texts(language).sftp.edit_status_ignored,
                                edit.file_name
                            )));
                            let _ = std::fs::remove_file(&edit.local_path);
                        }
                        edits.retain(|edit| edit.id != edit_id);
                        external_edits.set(edits);
                    },
                }
            }

        ConnectionDialog {
            show: show_dialog,
            mode: dialog_mode,
            name: edit_name,
            host: edit_host,
            port: edit_port,
            user: edit_user,
            group: edit_group,
            password: edit_password,
            proxy_jump: edit_proxy_jump,
            use_agent: edit_use_agent,
            forward_agent: edit_forward_agent,
            groups: saved_groups.clone(),
            language,
            on_save: move |profile: SessionProfile| {
                let original_name = edit_original_name();
                let is_rename = dialog_mode() == "edit"
                    && !original_name.is_empty()
                    && original_name != profile.name;

                if let Err(e) = store.save_session(profile.clone()) {
                    tracing::error!("保存连接失败: {}", e);
                } else {
                    if is_rename {
                        if let Err(e) = store.delete_session(&original_name) {
                            tracing::error!("删除旧连接失败: {}", e);
                        }
                    }
                    let pwd = edit_password();
                    if !pwd.is_empty() {
                        let vault_id = profile.params.effective_vault_id();
                        if let Err(e) = store.set_secret(&vault_id, &pwd) {
                            tracing::error!("保存密码失败: {}", e);
                        }
                    }
                    saved_tick.set(saved_tick() + 1);
                }
            },
        }

        GroupDialog {
            show: show_group_dialog,
            mode: group_dialog_mode,
            name: group_dialog_name,
            language,
            on_save: move |name: String| {
                let original = group_dialog_original();
                let result = if group_dialog_mode() == "rename" {
                    if original == DEFAULT_GROUP_NAME {
                        store.rename_default_group(&name)
                    } else {
                        store.rename_group(&original, &name)
                    }
                } else {
                    store.add_group(&name)
                };
                match result {
                    Ok(()) => saved_tick.set(saved_tick() + 1),
                    Err(e) => tracing::error!("保存分组失败: {}", e),
                }
            },
        }

        SftpNameDialog {
            show: show_sftp_name_dialog,
            mode: sftp_name_dialog_mode,
            value: sftp_name_dialog_value,
            language,
            on_save: {
                let state = state.clone();
                move |name: String| {
                    let Some(session_id) = sftp_name_dialog_session() else {
                        return;
                    };
                    let name = name.trim().to_string();
                    if name.is_empty() {
                        return;
                    }

                    match sftp_name_dialog_mode().as_str() {
                        "mkdir" => {
                            let path = join_path(&sftp_name_dialog_base_path(), &name);
                            send_sftp_request(state.clone(), session_id, SftpRequest::Mkdir { path });
                        }
                        "rename" => {
                            let from = sftp_name_dialog_target_path();
                            if from.is_empty() {
                                return;
                            }
                            let to = join_path(&parent_path(&from), &name);
                            if from != to {
                                send_sftp_request(state.clone(), session_id, SftpRequest::Rename { from, to });
                            }
                        }
                        _ => {}
                    }
                }
            },
        }

        SettingsPanel {
            show: show_settings,
            language,
            on_language_change: move |language| {
                let mut next = settings();
                next.language = language;
                match store.update_settings(next.clone()) {
                    Ok(()) => settings.set(next),
                    Err(e) => tracing::error!("保存设置失败: {}", e),
                }
            },
        }
    }
}

/// 按 group 字段分组，无 group 的归入默认组名，并保留空分组。
fn group_profiles(
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
    for p in profiles {
        let key = p
            .group
            .as_deref()
            .and_then(normalize_group_name)
            .unwrap_or_else(|| DEFAULT_GROUP_NAME.to_string());
        map.entry(key).or_default().push(p.clone());
    }
    map.into_iter().collect()
}

fn session_state_from_profile(id: SessionId, profile: &SessionProfile) -> SessionState {
    SessionState {
        id,
        title: profile.name.clone(),
        snapshot: None,
        connected: false,
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

fn session_dot_class(sess: &SessionState) -> &'static str {
    if sess.connected {
        "status-dot online"
    } else if sess.connection_error.is_some() {
        "status-dot idle"
    } else {
        "status-dot connecting"
    }
}

fn params_with_ssh_config(params: ConnectParams, use_ssh_config: bool) -> ConnectParams {
    let Some(config_path) = home_ssh_config_file() else {
        return params;
    };
    merge_ssh_config_from_path(params, use_ssh_config, &config_path)
}

fn merge_ssh_config_from_path(
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

fn clamp_dimension(value: f64, min: f64, max: f64) -> f64 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        min
    }
}

fn duplicate_profile(profile: &SessionProfile, existing: &[SessionProfile]) -> SessionProfile {
    let mut next = profile.clone();
    let names = existing
        .iter()
        .map(|profile| profile.name.as_str())
        .collect::<Vec<_>>();
    next.name = unique_copy_name(&profile.name, &names);
    next
}

fn unique_copy_name(base: &str, existing_names: &[&str]) -> String {
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

fn send_sftp_request(state: Arc<Mutex<AppState>>, session_id: SessionId, req: SftpRequest) {
    if let Ok(app_state) = state.lock() {
        app_state.manager.send(ToCore::Sftp {
            id: session_id,
            req,
        });
    }
}

fn copy_to_clipboard(value: &str) {
    let js_value = format!("{value:?}");
    let script = format!(
        r#"
        (() => {{
            const value = {js_value};
            if (navigator.clipboard && navigator.clipboard.writeText) {{
                navigator.clipboard.writeText(value);
                return;
            }}
            const el = document.createElement("textarea");
            el.value = value;
            el.style.position = "fixed";
            el.style.opacity = "0";
            document.body.appendChild(el);
            el.select();
            document.execCommand("copy");
            document.body.removeChild(el);
        }})();
        "#
    );
    dioxus::document::eval(&script);
}

fn format_sftp_size(size: u64, is_dir: bool) -> String {
    if is_dir {
        String::new()
    } else {
        const KB: u64 = 1024;
        const MB: u64 = 1024 * KB;
        const GB: u64 = 1024 * MB;

        if size >= GB {
            format!("{:.1} GB", size as f64 / GB as f64)
        } else if size >= MB {
            format!("{:.1} MB", size as f64 / MB as f64)
        } else if size >= KB {
            format!("{:.1} KB", size as f64 / KB as f64)
        } else {
            format!("{} B", size)
        }
    }
}

fn format_sftp_time(timestamp: Option<u32>) -> String {
    use std::time::{Duration, UNIX_EPOCH};

    timestamp
        .map(|timestamp| {
            let time = UNIX_EPOCH + Duration::from_secs(timestamp as u64);
            let datetime: chrono::DateTime<chrono::Local> = time.into();
            datetime.format("%Y/%m/%d %H:%M:%S").to_string()
        })
        .unwrap_or_default()
}

fn format_sftp_permissions(permissions: Option<u32>, is_dir: bool) -> String {
    let Some(permissions) = permissions else {
        return String::new();
    };

    let file_type = if is_dir {
        'd'
    } else if permissions & 0o120000 == 0o120000 {
        'l'
    } else {
        '-'
    };
    let mut out = String::with_capacity(10);
    out.push(file_type);
    for bit in [
        0o400, 0o200, 0o100, 0o040, 0o020, 0o010, 0o004, 0o002, 0o001,
    ] {
        out.push(match bit {
            0o400 | 0o040 | 0o004 => {
                if permissions & bit != 0 {
                    'r'
                } else {
                    '-'
                }
            }
            0o200 | 0o020 | 0o002 => {
                if permissions & bit != 0 {
                    'w'
                } else {
                    '-'
                }
            }
            _ => {
                if permissions & bit != 0 {
                    'x'
                } else {
                    '-'
                }
            }
        });
    }
    out
}

fn format_sftp_owner(entry: &SftpEntry) -> String {
    let user = entry
        .user
        .clone()
        .or_else(|| entry.uid.map(|uid| uid.to_string()))
        .unwrap_or_else(|| "-".to_string());
    let group = entry
        .group
        .clone()
        .or_else(|| entry.gid.map(|gid| gid.to_string()))
        .unwrap_or_else(|| "-".to_string());
    format!("{user}/{group}")
}

fn sync_external_edits(
    edits: Vec<ExternalEdit>,
    sessions: &std::collections::HashMap<SessionId, SessionState>,
) -> (Vec<ExternalEdit>, Vec<ExternalEditAction>) {
    let mut next = Vec::with_capacity(edits.len());
    let mut actions = Vec::new();

    for mut edit in edits {
        let completion = sessions
            .get(&edit.session_id)
            .and_then(|session| session.sftp_last_done.as_ref())
            .filter(|completion| {
                completion.revision > edit.after_revision && completion.path == edit.remote_path
            });
        let completion_op = completion.map(|completion| completion.op);
        let completion_revision = completion
            .map(|completion| completion.revision)
            .unwrap_or(edit.after_revision);

        match (&edit.status, completion_op) {
            (ExternalEditStatus::Downloading, Some(kt_core::SftpOp::Download)) => {
                edit.after_revision = completion_revision;
                edit.status = ExternalEditStatus::Watching;
                edit.last_seen_modified = local_file_modified(&edit.local_path);
                edit.pending_modified = None;
                actions.push(ExternalEditAction::OpenLocal {
                    edit_id: edit.id,
                    path: edit.local_path.clone(),
                    file_name: edit.file_name.clone(),
                });
                next.push(edit);
            }
            (ExternalEditStatus::UploadingOnce, Some(kt_core::SftpOp::Upload)) => {
                actions.push(ExternalEditAction::UploadCompleted {
                    file_name: edit.file_name.clone(),
                });
                actions.push(ExternalEditAction::DeleteLocal(edit.local_path.clone()));
            }
            (ExternalEditStatus::UploadingAuto, Some(kt_core::SftpOp::Upload)) => {
                edit.after_revision = completion_revision;
                edit.status = ExternalEditStatus::Watching;
                edit.last_seen_modified = local_file_modified(&edit.local_path)
                    .or(edit.pending_modified)
                    .or(edit.last_seen_modified);
                edit.pending_modified = None;
                actions.push(ExternalEditAction::UploadCompleted {
                    file_name: edit.file_name.clone(),
                });
                next.push(edit);
            }
            (ExternalEditStatus::Watching, _) => {
                let modified = local_file_modified(&edit.local_path);
                if is_newer_modified(modified, edit.last_seen_modified) {
                    edit.pending_modified = modified;
                    match edit.sync_mode {
                        ExternalEditSyncMode::Ask => {
                            edit.status = ExternalEditStatus::PromptPending;
                        }
                        ExternalEditSyncMode::AutoUpload => {
                            edit.status = ExternalEditStatus::UploadingAuto;
                            edit.after_revision = latest_sftp_completion_revision_from_sessions(
                                sessions,
                                edit.session_id,
                            );
                            edit.last_seen_modified = modified;
                            edit.pending_modified = None;
                            actions.push(ExternalEditAction::Upload {
                                session_id: edit.session_id,
                                local_path: edit.local_path.clone(),
                                remote_path: edit.remote_path.clone(),
                                file_name: edit.file_name.clone(),
                            });
                        }
                    }
                }
                next.push(edit);
            }
            _ => next.push(edit),
        }
    }

    (next, actions)
}

fn local_file_modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn is_newer_modified(current: Option<SystemTime>, previous: Option<SystemTime>) -> bool {
    match (current, previous) {
        (Some(current), Some(previous)) => current
            .duration_since(previous)
            .map(|duration| duration > Duration::from_millis(0))
            .unwrap_or(false),
        (Some(_), None) => true,
        _ => false,
    }
}

fn latest_sftp_completion_revision_from_sessions(
    sessions: &std::collections::HashMap<SessionId, SessionState>,
    session_id: SessionId,
) -> u64 {
    sessions
        .get(&session_id)
        .and_then(|session| session.sftp_last_done.as_ref())
        .map(|completion| completion.revision)
        .unwrap_or(0)
}

fn latest_sftp_completion_revision(state: Arc<Mutex<AppState>>, session_id: SessionId) -> u64 {
    state
        .lock()
        .ok()
        .and_then(|app_state| {
            app_state
                .sessions
                .get(&session_id)
                .and_then(|session| session.sftp_last_done.as_ref())
                .map(|completion| completion.revision)
        })
        .unwrap_or(0)
}

fn external_edit_local_path(session_id: SessionId, remote_path: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    std::env::temp_dir()
        .join("kitonyterms")
        .join("sftp-edit")
        .join(format!("session-{}", session_id.0))
        .join(format!(
            "{}-{}",
            stamp,
            sanitize_local_file_name(&remote_file_name(remote_path))
        ))
}

fn remote_file_name(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("remote-file")
        .to_string()
}

fn sanitize_local_file_name(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "remote-file".to_string()
    } else {
        sanitized
    }
}

fn open_local_file(path: &Path) -> Result<(), String> {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("/usr/bin/open");
        command.arg(&path);
        command
    };

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.arg("/C").arg("start").arg("").arg(&path);
        command
    };

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(&path);
        command
    };

    command.spawn().map(|_| ()).map_err(|e| e.to_string())
}

fn external_edit_status_text(
    edits: &[ExternalEdit],
    session: Option<&SessionState>,
    notice: Option<&str>,
    language: AppLanguage,
) -> Option<String> {
    let t = texts(language).sftp;

    if let Some(edit) = edits
        .iter()
        .find(|edit| edit.status == ExternalEditStatus::Downloading)
    {
        return Some(format!("{} {}", t.edit_status_downloading, edit.file_name));
    }

    if let Some(edit) = edits
        .iter()
        .find(|edit| edit.status == ExternalEditStatus::PromptPending)
    {
        return Some(format!("{} {}", t.edit_status_saved, edit.file_name));
    }

    if let Some(edit) = edits.iter().find(|edit| {
        matches!(
            edit.status,
            ExternalEditStatus::UploadingOnce | ExternalEditStatus::UploadingAuto
        )
    }) {
        let progress = session
            .and_then(|session| session.sftp_progress.as_ref())
            .filter(|progress| progress.name == edit.file_name)
            .and_then(format_sftp_progress_percent)
            .map(|percent| format!(" {percent}%"))
            .unwrap_or_default();
        return Some(format!(
            "{} {}{}",
            t.edit_status_uploading, edit.file_name, progress
        ));
    }

    notice.map(str::to_string)
}

fn format_sftp_progress_percent(progress: &crate::state::SftpProgressState) -> Option<u64> {
    if progress.total == 0 {
        return None;
    }
    Some(((progress.transferred.saturating_mul(100)) / progress.total).min(100))
}

#[component]
fn ConnectionCard(
    profile: SessionProfile,
    active: bool,
    language: AppLanguage,
    on_connect: EventHandler<()>,
    on_edit: EventHandler<()>,
    on_delete: EventHandler<()>,
    on_copy: EventHandler<()>,
    on_context_menu: EventHandler<(f64, f64)>,
) -> Element {
    let t = texts(language).app;

    rsx! {
        div {
            class: if active { "connection-card is-active" } else { "connection-card" },
            oncontextmenu: move |evt| {
                evt.prevent_default();
                evt.stop_propagation();
                on_context_menu.call((evt.client_coordinates().x, evt.client_coordinates().y));
            },

            div {
                class: "connection-main",
                onclick: move |_| on_connect.call(()),

                span { class: "status-dot online" }
                div {
                    class: "connection-copy",
                    strong { "{profile.name}" }
                }
            }

            div {
                class: "connection-actions",
                button {
                    title: "{t.edit}",
                    onclick: move |evt| {
                        evt.stop_propagation();
                        on_edit.call(());
                    },
                    Icon { name: "edit" }
                }
                button {
                    title: "{t.copy}",
                    onclick: move |evt| {
                        evt.stop_propagation();
                        on_copy.call(());
                    },
                    Icon { name: "file" }
                }
                button {
                    title: "{t.delete}",
                    onclick: move |evt| {
                        evt.stop_propagation();
                        on_delete.call(());
                    },
                    Icon { name: "trash" }
                }
            }
        }
    }
}

#[component]
fn SettingsPanel(
    show: Signal<bool>,
    language: AppLanguage,
    on_language_change: EventHandler<AppLanguage>,
) -> Element {
    if !show() {
        return rsx! {};
    }

    let t = texts(language).app;

    rsx! {
        div {
            class: "settings-overlay",
            onclick: move |_| show.set(false),

            section {
                class: "settings-panel",
                onclick: move |evt| evt.stop_propagation(),

                div {
                    class: "settings-head",
                    h2 { "{t.settings}" }
                    button {
                        class: "icon-button slim",
                        title: "{t.close}",
                        onclick: move |_| show.set(false),
                        Icon { name: "close" }
                    }
                }

                div {
                    class: "settings-row",
                    div {
                        strong { "{t.language}" }
                        p { "{t.language_hint}" }
                    }

                    div {
                        class: "segmented-control",
                        button {
                            class: if language == AppLanguage::Chinese { "is-selected" } else { "" },
                            onclick: move |_| on_language_change.call(AppLanguage::Chinese),
                            "{t.chinese}"
                        }
                        button {
                            class: if language == AppLanguage::English { "is-selected" } else { "" },
                            onclick: move |_| on_language_change.call(AppLanguage::English),
                            "{t.english}"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn TerminalPlaceholder(
    connected: bool,
    title: String,
    error: Option<String>,
    language: AppLanguage,
) -> Element {
    let t = texts(language).app;
    let state_line = if let Some(error) = error.as_deref() {
        error
    } else if connected {
        t.terminal_waiting
    } else {
        t.terminal_connecting
    };

    rsx! {
        div {
            class: "terminal-placeholder",
            pre {
                r#"
 _  ___ _                         _____
| |/ (_) |                       |_   _|
| ' / _| |_ ___  _ __  _   _      | | ___ _ __ _ __ ___  ___
|  < | | __/ _ \| '_ \| | | |     | |/ _ \ '__| '_ ` _ \/ __|
| . \| | || (_) | | | | |_| |     | |  __/ |  | | | | | \__ \
|_|\_\_|\__\___/|_| |_|\__, |     \_/\___|_|  |_| |_| |_|___/
                         __/ |
                        |___/
"#
            }
            p { "{t.session_label}: {title}" }
            p { "{state_line}" }
            span { class: "terminal-caret" }
        }
    }
}

#[component]
fn EmptyWorkbench(language: AppLanguage) -> Element {
    let t = texts(language).app;

    rsx! {
        div {
            class: "empty-workbench",
            div { class: "empty-logo", AppLogo { size: "empty" } }
            h2 { "{t.empty_title}" }
            p { "{t.empty_hint}" }
        }
    }
}

#[component]
fn MonitorPlaceholder(language: AppLanguage) -> Element {
    let t = texts(language).app;
    let labels = ["CPU", t.memory, t.load, t.network];

    rsx! {
        div {
            class: "monitor-panel compact",
            div {
                class: "monitor-title",
                Icon { name: "monitor" }
                "{t.system_monitor}"
            }
            div {
                class: "monitor-grid",
                for label in labels {
                    div {
                        class: "metric-card",
                        span { class: "metric-label", "{label}" }
                        strong { "--" }
                        div { class: "sparkline muted" }
                    }
                }
            }
        }
    }
}

#[component]
fn ExternalEditSaveDialog(
    edit: ExternalEdit,
    language: AppLanguage,
    on_upload_once: EventHandler<u64>,
    on_auto_upload: EventHandler<u64>,
    on_ignore: EventHandler<u64>,
) -> Element {
    let t = texts(language).sftp;
    let dialog_t = texts(language).dialog;
    let edit_id = edit.id;

    rsx! {
        div {
            class: "settings-overlay",

            section {
                class: "settings-panel external-edit-dialog",
                onclick: move |evt| evt.stop_propagation(),

                div {
                    class: "settings-head",
                    h2 { "{t.edit_save_prompt_title}" }
                    button {
                        class: "icon-button slim",
                        title: "{dialog_t.cancel}",
                        onclick: move |_| on_ignore.call(edit_id),
                        Icon { name: "close" }
                    }
                }

                div {
                    class: "external-edit-dialog-body",
                    Icon { name: "edit" }
                    div {
                        strong { "{edit.file_name}" }
                        p { "{t.edit_save_prompt_body}" }
                        code { "{edit.remote_path}" }
                    }
                }

                div {
                    class: "external-edit-dialog-actions",
                    button {
                        class: "primary",
                        onclick: move |_| on_upload_once.call(edit_id),
                        "{t.edit_save_once}"
                    }
                    button {
                        onclick: move |_| on_auto_upload.call(edit_id),
                        "{t.edit_save_auto}"
                    }
                    button {
                        onclick: move |_| on_ignore.call(edit_id),
                        "{t.edit_save_ignore}"
                    }
                }
            }
        }
    }
}

#[component]
fn SidebarSftpTree(
    session_id: SessionId,
    connected: bool,
    path: String,
    entries: Vec<SftpEntry>,
    loading: bool,
    error: Option<String>,
    language: AppLanguage,
    on_context_menu: EventHandler<ContextMenuState>,
) -> Element {
    let state = get_state().clone();
    let t = texts(language).sftp;
    let item_count = entries.len();
    let total_size = entries
        .iter()
        .filter(|entry| !entry.is_dir)
        .map(|entry| entry.size)
        .sum::<u64>();

    rsx! {
        div {
            class: "sidebar-sftp-tree",

            div {
                class: "sftp-path-line",
                Icon { name: "folder" }
                span { "{display_path(&path)}" }
            }

            div {
                class: "sftp-tree-actions",
                button {
                    title: "{t.back}",
                    disabled: !connected || path == "/" || path == ".",
                    onclick: {
                        let path = path.clone();
                        let state = state.clone();
                        move |evt| {
                            evt.stop_propagation();
                            if path != "/" && path != "." {
                                let parent = parent_path(&path);
                                if let Err(e) = request_directory(state.clone(), session_id, parent, language) {
                                    tracing::error!("SFTP 返回上级失败: {}", e);
                                }
                            }
                        }
                    },
                    Icon { name: "back" }
                }
                button {
                    title: "{t.refresh}",
                    disabled: !connected,
                    onclick: {
                        let path = path.clone();
                        let state = state.clone();
                        move |evt| {
                            evt.stop_propagation();
                            if let Err(e) = request_directory(state.clone(), session_id, path.clone(), language) {
                                tracing::error!("SFTP 刷新失败: {}", e);
                            }
                        }
                    },
                    Icon { name: "refresh" }
                }
            }

            div {
                class: "sftp-tree-list",
                oncontextmenu: {
                    let path = path.clone();
                    move |evt| {
                        evt.prevent_default();
                        evt.stop_propagation();
                        on_context_menu.call(ContextMenuState {
                            target: ContextMenuTarget::SftpBlank {
                                session_id,
                                path: path.clone(),
                            },
                            x: evt.client_coordinates().x,
                            y: evt.client_coordinates().y,
                        });
                    }
                },

                div {
                    class: "sftp-table-grid",

                    div {
                        class: "sftp-table-head-row",
                        span { class: "sftp-name-col", "{t.name}" }
                        span { "{t.modified}" }
                        span { "{t.size}" }
                        span { "{t.permissions}" }
                        span { "{t.owner_group}" }
                    }

                    if loading {
                        div { class: "sftp-table-message", "{t.loading}" }
                    } else if let Some(error) = error.clone() {
                        div { class: "sftp-table-message error", "{t.error_prefix}: {error}" }
                    } else if entries.is_empty() {
                        div { class: "sftp-table-message", "0 {t.items}" }
                    } else {
                        for entry in entries.clone() {
                            SidebarSftpEntry {
                                key: "{entry.name}",
                                session_id,
                                base_path: path.clone(),
                                entry,
                                language,
                                on_context_menu,
                            }
                        }
                    }
                }
            }

            div {
                class: "sftp-table-status",
                span { "{item_count} {t.items}" }
                span { "{format_sftp_size(total_size, false)}" }
            }
        }
    }
}

#[component]
fn SidebarSftpEntry(
    session_id: SessionId,
    base_path: String,
    entry: SftpEntry,
    language: AppLanguage,
    on_context_menu: EventHandler<ContextMenuState>,
) -> Element {
    let state = get_state().clone();
    let icon = if entry.is_dir { "folder" } else { "file" };
    let row_class = if entry.is_dir {
        "sftp-table-row is-dir"
    } else {
        "sftp-table-row is-file"
    };
    let name = entry.name.clone();
    let modified = format_sftp_time(entry.modified);
    let size = format_sftp_size(entry.size, entry.is_dir);
    let permissions = format_sftp_permissions(entry.permissions, entry.is_dir);
    let owner = format_sftp_owner(&entry);
    let full_path = join_path(&base_path, &name);
    let click_base_path = base_path.clone();
    let click_entry = entry.clone();

    rsx! {
        div {
            class: "{row_class}",
            title: "{full_path}",
            onclick: move |evt| {
                evt.stop_propagation();
                if click_entry.is_dir {
                    let next = join_path(&click_base_path, &click_entry.name);
                    if let Err(e) = request_directory(state.clone(), session_id, next, language) {
                        tracing::error!("SFTP 打开目录失败: {}", e);
                    }
                }
            },
            oncontextmenu: {
                let base_path = base_path.clone();
                let entry = entry.clone();
                move |evt| {
                    evt.prevent_default();
                    evt.stop_propagation();
                    on_context_menu.call(ContextMenuState {
                        target: ContextMenuTarget::SftpEntry(SftpEntryContext {
                            session_id,
                            base_path: base_path.clone(),
                            entry: entry.clone(),
                        }),
                        x: evt.client_coordinates().x,
                        y: evt.client_coordinates().y,
                    });
                }
            },

            span {
                class: "sftp-name-cell",
                Icon { name: icon }
                span { "{name}" }
            }
            span { "{modified}" }
            span { "{size}" }
            span { "{permissions}" }
            span { "{owner}" }
        }
    }
}

#[component]
fn ContextMenu(
    menu: ContextMenuState,
    language: AppLanguage,
    on_profile_edit: EventHandler<String>,
    on_profile_delete: EventHandler<String>,
    on_profile_copy: EventHandler<String>,
    on_group_new: EventHandler<()>,
    on_group_rename: EventHandler<String>,
    on_group_delete: EventHandler<String>,
    on_sftp_open: EventHandler<SftpEntryContext>,
    on_sftp_refresh: EventHandler<(SessionId, String)>,
    on_sftp_mkdir: EventHandler<(SessionId, String)>,
    on_sftp_rename: EventHandler<SftpEntryContext>,
    on_sftp_delete: EventHandler<SftpEntryContext>,
    on_sftp_external_edit: EventHandler<SftpEntryContext>,
    on_copy_text: EventHandler<String>,
) -> Element {
    let t = texts(language).app;
    let sftp_t = texts(language).sftp;
    let style = format!("left: {:.0}px; top: {:.0}px;", menu.x, menu.y);
    let menu_x = menu.x;
    let menu_y = menu.y;

    use_effect(move || {
        let script = format!(
            r#"
            requestAnimationFrame(() => {{
                const menu = document.querySelector('[data-kt-context-menu="active"]');
                if (!menu) return;
                const margin = 8;
                menu.style.left = '{menu_x}px';
                menu.style.top = '{menu_y}px';
                menu.style.right = 'auto';
                menu.style.bottom = 'auto';
                const rect = menu.getBoundingClientRect();
                if (rect.right > window.innerWidth - margin) {{
                    menu.style.left = `${{Math.max(margin, window.innerWidth - rect.width - margin)}}px`;
                }}
                if (rect.bottom > window.innerHeight - margin) {{
                    menu.style.top = 'auto';
                    menu.style.bottom = `${{Math.max(margin, window.innerHeight - {menu_y})}}px`;
                }}
            }});
            "#
        );
        dioxus::document::eval(&script);
    });

    rsx! {
        div {
            class: "context-menu",
            "data-kt-context-menu": "active",
            style: "{style}",
            onclick: move |evt| evt.stop_propagation(),
            oncontextmenu: move |evt| {
                evt.prevent_default();
                evt.stop_propagation();
            },

            match menu.target.clone() {
                ContextMenuTarget::Profile(name) => rsx! {
                    button {
                        onclick: {
                            let name = name.clone();
                            move |_| on_profile_edit.call(name.clone())
                        },
                        Icon { name: "edit" }
                        span { "{t.edit}" }
                    }
                    button {
                        onclick: {
                            let name = name.clone();
                            move |_| on_profile_copy.call(name.clone())
                        },
                        Icon { name: "file" }
                        span { "{t.copy}" }
                    }
                    button {
                        class: "danger",
                        onclick: {
                            let name = name.clone();
                            move |_| on_profile_delete.call(name.clone())
                        },
                        Icon { name: "trash" }
                        span { "{t.delete}" }
                    }
                },
                ContextMenuTarget::Group(name) => rsx! {
                    button {
                        onclick: move |_| on_group_new.call(()),
                        Icon { name: "folder" }
                        span { "{t.new_group}" }
                    }
                    button {
                        onclick: {
                            let name = name.clone();
                            move |_| on_group_rename.call(name.clone())
                        },
                        Icon { name: "edit" }
                        span { "{t.rename_group}" }
                    }
                    if name != DEFAULT_GROUP_NAME {
                        button {
                            class: "danger",
                            onclick: {
                                let name = name.clone();
                                move |_| on_group_delete.call(name.clone())
                            },
                            Icon { name: "trash" }
                            span { "{t.delete_group}" }
                        }
                    }
                },
                ContextMenuTarget::SftpEntry(ctx) => {
                    let full_path = join_path(&ctx.base_path, &ctx.entry.name);
                    let is_dir = ctx.entry.is_dir;
                    rsx! {
                        button {
                            onclick: {
                                let ctx = ctx.clone();
                                move |_| on_sftp_refresh.call((ctx.session_id, ctx.base_path.clone()))
                            },
                            Icon { name: "refresh" }
                            span { "{sftp_t.refresh}" }
                            small { "F5" }
                        }
                        if is_dir {
                            button {
                                onclick: {
                                    let ctx = ctx.clone();
                                    move |_| on_sftp_open.call(ctx.clone())
                                },
                                Icon { name: "folder" }
                                span { "{sftp_t.open}" }
                                small { "Return" }
                            }
                        }
                        if !is_dir {
                            button {
                                onclick: {
                                    let ctx = ctx.clone();
                                    move |_| on_sftp_external_edit.call(ctx.clone())
                                },
                                Icon { name: "edit" }
                                span { "{sftp_t.edit_external}" }
                            }
                        }
                        div { class: "context-separator" }
                        button {
                            onclick: {
                                let full_path = full_path.clone();
                                move |_| on_copy_text.call(full_path.clone())
                            },
                            Icon { name: "file" }
                            span { "{sftp_t.copy_path}" }
                        }
                        button {
                            onclick: {
                                let name = ctx.entry.name.clone();
                                move |_| on_copy_text.call(name.clone())
                            },
                            Icon { name: "file" }
                            span { "{sftp_t.copy_name}" }
                        }
                        div { class: "context-separator" }
                        button {
                            onclick: {
                                let ctx = ctx.clone();
                                move |_| on_sftp_mkdir.call((ctx.session_id, ctx.base_path.clone()))
                            },
                            Icon { name: "folder" }
                            span { "{sftp_t.new_folder}" }
                        }
                        button {
                            onclick: {
                                let ctx = ctx.clone();
                                move |_| on_sftp_rename.call(ctx.clone())
                            },
                            Icon { name: "edit" }
                            span { "{sftp_t.rename}" }
                            small { "F2" }
                        }
                        button {
                            class: "danger",
                            onclick: {
                                let ctx = ctx.clone();
                                move |_| on_sftp_delete.call(ctx.clone())
                            },
                            Icon { name: "trash" }
                            span { "{sftp_t.delete}" }
                            small { "Del" }
                        }
                    }
                },
                ContextMenuTarget::SftpBlank { session_id, path } => rsx! {
                    button {
                        onclick: {
                            let path = path.clone();
                            move |_| on_sftp_refresh.call((session_id, path.clone()))
                        },
                        Icon { name: "refresh" }
                        span { "{sftp_t.refresh}" }
                        small { "F5" }
                    }
                    button {
                        onclick: {
                            let path = path.clone();
                            move |_| on_sftp_mkdir.call((session_id, path.clone()))
                        },
                        Icon { name: "folder" }
                        span { "{sftp_t.new_folder}" }
                    }
                },
            }
        }
    }
}

#[component]
fn GroupDialog(
    show: Signal<bool>,
    mode: Signal<String>,
    name: Signal<String>,
    language: AppLanguage,
    on_save: EventHandler<String>,
) -> Element {
    if !show() {
        return rsx! {};
    }

    let t = texts(language).app;
    let title = if mode() == "rename" {
        t.rename_group
    } else {
        t.new_group
    };

    rsx! {
        div {
            class: "settings-overlay",
            onclick: move |_| show.set(false),

            section {
                class: "settings-panel group-dialog",
                onclick: move |evt| evt.stop_propagation(),

                div {
                    class: "settings-head",
                    h2 { "{title}" }
                    button {
                        class: "icon-button slim",
                        title: "{t.close}",
                        onclick: move |_| show.set(false),
                        Icon { name: "close" }
                    }
                }

                div {
                    class: "group-form",
                    input {
                        r#type: "text",
                        value: "{name()}",
                        oninput: move |evt| name.set(evt.value().clone()),
                        placeholder: "{texts(language).dialog.group_placeholder}",
                    }
                }

                div {
                    class: "group-actions",
                    button {
                        onclick: move |_| show.set(false),
                        "{texts(language).dialog.cancel}"
                    }
                    button {
                        class: "primary",
                        onclick: move |_| {
                            let value = name();
                            if normalize_group_name(&value).is_some() {
                                on_save.call(value);
                                show.set(false);
                            }
                        },
                        "{texts(language).dialog.save}"
                    }
                }
            }
        }
    }
}

#[component]
fn SftpNameDialog(
    show: Signal<bool>,
    mode: Signal<String>,
    value: Signal<String>,
    language: AppLanguage,
    on_save: EventHandler<String>,
) -> Element {
    if !show() {
        return rsx! {};
    }

    let t = texts(language).sftp;
    let dialog_t = texts(language).dialog;
    let title = if mode() == "rename" {
        t.rename
    } else {
        t.new_folder
    };

    rsx! {
        div {
            class: "settings-overlay",
            onclick: move |_| show.set(false),

            section {
                class: "settings-panel group-dialog",
                onclick: move |evt| evt.stop_propagation(),

                div {
                    class: "settings-head",
                    h2 { "{title}" }
                    button {
                        class: "icon-button slim",
                        title: "{t.close}",
                        onclick: move |_| show.set(false),
                        Icon { name: "close" }
                    }
                }

                div {
                    class: "group-form",
                    input {
                        r#type: "text",
                        value: "{value()}",
                        oninput: move |evt| value.set(evt.value().clone()),
                        placeholder: "{t.name}",
                    }
                }

                div {
                    class: "group-actions",
                    button {
                        onclick: move |_| show.set(false),
                        "{dialog_t.cancel}"
                    }
                    button {
                        class: "primary",
                        onclick: move |_| {
                            let next = value();
                            if !next.trim().is_empty() {
                                on_save.call(next);
                                show.set(false);
                            }
                        },
                        "{dialog_t.save}"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kt_config::{AuthMethod, ConnectParams, SessionProfile};
    use std::io::Write;

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

        assert_eq!(session_dot_class(&state), "status-dot connecting");

        state.connection_error = Some("连接失败".to_string());
        assert_eq!(session_dot_class(&state), "status-dot idle");

        state.connected = true;
        state.connection_error = None;
        assert_eq!(session_dot_class(&state), "status-dot online");
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
                    forward_agent: false,
                },
            },
        ];

        let grouped = group_profiles(&profiles, &["Cache".to_string()]);
        assert_eq!(grouped.len(), 3);
        assert_eq!(grouped[0].0, "Cache");
        assert!(grouped[0].1.is_empty());
        assert_eq!(grouped[1].0, "NoBrand");
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

    #[test]
    fn sftp_permissions_are_rendered_like_unix_modes() {
        assert_eq!(format_sftp_permissions(Some(0o040755), true), "drwxr-xr-x");
        assert_eq!(format_sftp_permissions(Some(0o100600), false), "-rw-------");
        assert_eq!(format_sftp_permissions(None, false), "");
    }

    #[test]
    fn sftp_owner_prefers_names_and_falls_back_to_ids() {
        let named = SftpEntry {
            name: "demo".to_string(),
            is_dir: false,
            size: 1,
            modified: None,
            permissions: None,
            user: Some("root".to_string()),
            group: Some("wheel".to_string()),
            uid: Some(0),
            gid: Some(0),
        };
        assert_eq!(format_sftp_owner(&named), "root/wheel");

        let numeric = SftpEntry {
            user: None,
            group: None,
            ..named
        };
        assert_eq!(format_sftp_owner(&numeric), "0/0");
    }

    fn temp_edit_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "kitonyterms-test-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
            name
        ))
    }

    #[test]
    fn external_edit_download_completion_opens_local_file() {
        let profile = SessionProfile {
            name: "demo".to_string(),
            group: None,
            params: ConnectParams::new("host", "root"),
        };
        let mut session = session_state_from_profile(SessionId(1), &profile);
        session.sftp_last_done = Some(crate::state::SftpCompletion {
            op: kt_core::SftpOp::Download,
            path: "/root/demo.txt".to_string(),
            revision: 2,
        });
        let sessions = std::collections::HashMap::from([(SessionId(1), session)]);
        let edit = ExternalEdit {
            id: 1,
            session_id: SessionId(1),
            remote_path: "/root/demo.txt".to_string(),
            local_path: PathBuf::from("/tmp/demo.txt"),
            file_name: "demo.txt".to_string(),
            after_revision: 1,
            status: ExternalEditStatus::Downloading,
            sync_mode: ExternalEditSyncMode::Ask,
            last_seen_modified: None,
            pending_modified: None,
        };

        let (edits, actions) = sync_external_edits(vec![edit], &sessions);

        assert_eq!(edits[0].status, ExternalEditStatus::Watching);
        assert_eq!(edits[0].after_revision, 2);
        assert_eq!(
            actions,
            vec![ExternalEditAction::OpenLocal {
                edit_id: 1,
                path: PathBuf::from("/tmp/demo.txt"),
                file_name: "demo.txt".to_string(),
            }]
        );
    }

    #[test]
    fn external_edit_upload_once_completion_removes_pending_item() {
        let profile = SessionProfile {
            name: "demo".to_string(),
            group: None,
            params: ConnectParams::new("host", "root"),
        };
        let mut session = session_state_from_profile(SessionId(1), &profile);
        session.sftp_last_done = Some(crate::state::SftpCompletion {
            op: kt_core::SftpOp::Upload,
            path: "/root/demo.txt".to_string(),
            revision: 5,
        });
        let sessions = std::collections::HashMap::from([(SessionId(1), session)]);
        let edit = ExternalEdit {
            id: 1,
            session_id: SessionId(1),
            remote_path: "/root/demo.txt".to_string(),
            local_path: PathBuf::from("/tmp/demo.txt"),
            file_name: "demo.txt".to_string(),
            after_revision: 4,
            status: ExternalEditStatus::UploadingOnce,
            sync_mode: ExternalEditSyncMode::Ask,
            last_seen_modified: None,
            pending_modified: None,
        };

        let (edits, actions) = sync_external_edits(vec![edit], &sessions);

        assert!(edits.is_empty());
        assert_eq!(
            actions,
            vec![
                ExternalEditAction::UploadCompleted {
                    file_name: "demo.txt".to_string(),
                },
                ExternalEditAction::DeleteLocal(PathBuf::from("/tmp/demo.txt")),
            ]
        );
    }

    #[test]
    fn external_edit_saved_file_prompts_before_upload() {
        let path = temp_edit_path("prompt.txt");
        std::fs::write(&path, b"changed").unwrap();
        let sessions = std::collections::HashMap::new();
        let edit = ExternalEdit {
            id: 2,
            session_id: SessionId(1),
            remote_path: "/root/prompt.txt".to_string(),
            local_path: path.clone(),
            file_name: "prompt.txt".to_string(),
            after_revision: 0,
            status: ExternalEditStatus::Watching,
            sync_mode: ExternalEditSyncMode::Ask,
            last_seen_modified: Some(UNIX_EPOCH),
            pending_modified: None,
        };

        let (edits, actions) = sync_external_edits(vec![edit], &sessions);

        assert_eq!(edits[0].status, ExternalEditStatus::PromptPending);
        assert!(edits[0].pending_modified.is_some());
        assert!(actions.is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn external_edit_auto_uploads_when_saved() {
        let path = temp_edit_path("auto.txt");
        std::fs::write(&path, b"changed").unwrap();
        let profile = SessionProfile {
            name: "demo".to_string(),
            group: None,
            params: ConnectParams::new("host", "root"),
        };
        let mut session = session_state_from_profile(SessionId(1), &profile);
        session.sftp_last_done = Some(crate::state::SftpCompletion {
            op: kt_core::SftpOp::Download,
            path: "/root/auto.txt".to_string(),
            revision: 7,
        });
        let sessions = std::collections::HashMap::from([(SessionId(1), session)]);
        let edit = ExternalEdit {
            id: 3,
            session_id: SessionId(1),
            remote_path: "/root/auto.txt".to_string(),
            local_path: path.clone(),
            file_name: "auto.txt".to_string(),
            after_revision: 2,
            status: ExternalEditStatus::Watching,
            sync_mode: ExternalEditSyncMode::AutoUpload,
            last_seen_modified: Some(UNIX_EPOCH),
            pending_modified: None,
        };

        let (edits, actions) = sync_external_edits(vec![edit], &sessions);

        assert_eq!(edits[0].status, ExternalEditStatus::UploadingAuto);
        assert_eq!(edits[0].after_revision, 7);
        assert_eq!(
            actions,
            vec![ExternalEditAction::Upload {
                session_id: SessionId(1),
                local_path: path.clone(),
                remote_path: "/root/auto.txt".to_string(),
                file_name: "auto.txt".to_string(),
            }]
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn external_edit_temp_names_are_sanitized() {
        assert_eq!(remote_file_name("/root/.bashrc"), ".bashrc");
        assert_eq!(sanitize_local_file_name("../a b.txt"), ".._a_b.txt");
        let path = external_edit_local_path(SessionId(9), "/root/a b.txt");
        assert!(path.to_string_lossy().contains("session-9"));
        assert!(path.to_string_lossy().contains("a_b.txt"));
    }
}
