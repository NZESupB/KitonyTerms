//! 主应用组件 —— 深色工作台。
//!
//! 布局保留现有 core 通信链路，只重塑 Dioxus 界面层:
//! - 左侧:图标导航 + 可拖动资源管理器
//! - 中央:会话标签、终端
//! - 右侧:可拖动 SFTP 抽屉
//! - 底部:系统监控与状态栏

use std::sync::{Arc, Mutex, OnceLock};

use dioxus::prelude::*;
use kt_config::{ConnectParams, SessionProfile};
use kt_core::ssh::HostKeyDecision;
use kt_core::{AuthProvider, HostKeyVerifier, PtySize, SessionId, SessionManager, ToCore};

use crate::components::dialog::ConnectionDialog;
use crate::components::monitor::MonitorPanel;
use crate::components::sftp::SftpPanel;
use crate::components::terminal::{SnapshotWrapper, Terminal};
use crate::state::{AppState, SessionState};
use crate::store::Store;

/// 全局 Store(只初始化一次)。
static GLOBAL_STORE: OnceLock<Arc<Store>> = OnceLock::new();

/// 全局 AppState(只初始化一次)。
static GLOBAL_STATE: OnceLock<Arc<Mutex<AppState>>> = OnceLock::new();

const RESOURCE_PANEL_DEFAULT_WIDTH: f64 = 206.0;
const RESOURCE_PANEL_MIN_WIDTH: f64 = 176.0;
const RESOURCE_PANEL_MAX_WIDTH: f64 = 320.0;
const SFTP_PANEL_DEFAULT_WIDTH: f64 = 330.0;
const SFTP_PANEL_MIN_WIDTH: f64 = 280.0;
const SFTP_PANEL_MAX_WIDTH: f64 = 500.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResizeTarget {
    Resource,
    Sftp,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ResizeDrag {
    target: ResizeTarget,
    start_x: f64,
    start_width: f64,
}

/// 获取全局 state(供其他模块使用)。
pub fn get_state() -> &'static Arc<Mutex<AppState>> {
    GLOBAL_STATE.get_or_init(|| {
        tracing::info!("初始化 SessionManager");

        let store = get_store();
        let manager = SessionManager::spawn(
            Arc::new(AcceptAllVerifier),
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

/// AuthProvider 实现(从 Store 读取密码)。
struct StoreAuthProvider {
    store: Arc<Store>,
    vault_id: String,
}

impl AuthProvider for StoreAuthProvider {
    fn password(&mut self, _user: &str, _host: &str) -> Option<String> {
        self.store.get_secret(&self.vault_id)
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

/// 简单的 HostKeyVerifier(接受所有主机密钥, TOFU)。
struct AcceptAllVerifier;

impl HostKeyVerifier for AcceptAllVerifier {
    fn verify(
        &self,
        _host: &str,
        _port: u16,
        _key: &russh::keys::PublicKey,
        _fingerprint: &str,
    ) -> HostKeyDecision {
        HostKeyDecision::Accept
    }
}

#[component]
pub fn App() -> Element {
    let store = get_store();
    let state = get_state();

    let mut show_dialog = use_signal(|| false);
    let mut dialog_mode = use_signal(|| "new".to_string());
    let mut edit_name = use_signal(String::new);
    let mut edit_host = use_signal(String::new);
    let mut edit_port = use_signal(|| String::from("22"));
    let mut edit_user = use_signal(String::new);
    let mut edit_password = use_signal(String::new);

    let mut active_session_id = use_signal(|| None::<SessionId>);
    let mut all_sessions = use_signal(Vec::<SessionState>::new);
    let mut show_sftp = use_signal(|| true);
    let mut show_monitor = use_signal(|| true);
    let mut saved_tick = use_signal(|| 0u64);
    let mut resource_width = use_signal(|| RESOURCE_PANEL_DEFAULT_WIDTH);
    let mut sftp_width = use_signal(|| SFTP_PANEL_DEFAULT_WIDTH);
    let mut active_resize = use_signal(|| None::<ResizeDrag>);

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

    let active_session =
        move || active_session_id().and_then(|id| all_sessions().into_iter().find(|s| s.id == id));

    let saved_profiles = {
        let _ = saved_tick();
        store.saved_sessions()
    };

    let active_title = active_session()
        .map(|s| s.title)
        .unwrap_or_else(|| "未选择会话".to_string());
    let resource_panel_style = format!(
        "width: {:.0}px; flex-basis: {:.0}px;",
        resource_width(),
        resource_width()
    );
    let sftp_panel_style = format!(
        "width: {:.0}px; flex-basis: {:.0}px;",
        sftp_width(),
        sftp_width()
    );
    let window_class = if active_resize().is_some() {
        "kt-window is-resizing"
    } else {
        "kt-window"
    };

    rsx! {
        style { {include_str!("../assets/app.css")} }

        div {
            class: "{window_class}",
            onmousemove: move |evt| {
                if let Some(drag) = active_resize() {
                    let current_x = evt.client_coordinates().x;
                    match drag.target {
                        ResizeTarget::Resource => {
                            resource_width.set(panel_width_after_drag(
                                drag.start_width,
                                drag.start_x,
                                current_x,
                                ResizeTarget::Resource,
                                RESOURCE_PANEL_MIN_WIDTH,
                                RESOURCE_PANEL_MAX_WIDTH,
                            ));
                        }
                        ResizeTarget::Sftp => {
                            sftp_width.set(panel_width_after_drag(
                                drag.start_width,
                                drag.start_x,
                                current_x,
                                ResizeTarget::Sftp,
                                SFTP_PANEL_MIN_WIDTH,
                                SFTP_PANEL_MAX_WIDTH,
                            ));
                        }
                    }
                }
            },
            onmouseup: move |_| active_resize.set(None),
            onmouseleave: move |_| active_resize.set(None),

            div {
                class: "kt-content",

                nav {
                    class: "nav-rail",

                    div { class: "nav-logo", "KT" }

                    button { class: "nav-item is-active", title: "连接", span { "⌘" } small { "连接" } }
                    button { class: "nav-item", title: "会话", span { "▣" } small { "会话" } }
                    button {
                        class: if show_sftp() { "nav-item is-active-subtle" } else { "nav-item" },
                        title: "SFTP",
                        onclick: move |_| show_sftp.set(!show_sftp()),
                        span { "□" }
                        small { "SFTP" }
                    }
                    button {
                        class: if show_monitor() { "nav-item is-active-subtle" } else { "nav-item" },
                        title: "监控",
                        onclick: move |_| show_monitor.set(!show_monitor()),
                        span { "▱" }
                        small { "监控" }
                    }

                    div { class: "nav-grow" }
                    button { class: "nav-item", title: "设置", span { "⚙" } small { "设置" } }
                }

                div {
                    class: "main-column",

                    div {
                        class: "workspace-commandbar",

                        div {
                            class: "workspace-session",
                            if let Some(sess) = active_session() {
                                span {
                                    class: if sess.connected { "status-dot online" } else { "status-dot connecting" },
                                }
                                span { "{sess.title}" }
                            } else {
                                span { class: "status-dot idle" }
                                span { "未选择会话" }
                            }
                        }

                        div { class: "commandbar-spacer" }

                        div {
                            class: "global-search compact",
                            span { class: "search-symbol", "⌕" }
                            input {
                                class: "global-search-input",
                                placeholder: "搜索",
                            }
                            span { class: "search-shortcut", "⌘K" }
                        }

                        button { class: "icon-button slim", title: "通知", "⌁" }
                        button { class: "icon-button slim", title: "设置", "⚙" }
                        button { class: "avatar-button compact", title: "账户", "K" }
                    }

                    div {
                        class: "main-workbench",

                        aside {
                            class: "resource-panel",
                            style: "{resource_panel_style}",

                            div {
                                class: "resource-header",
                                div {
                                    h1 { "KitonyTerms" }
                                    p { "全端 SSH 客户端" }
                                }
                            }

                            div {
                                class: "resource-card",

                                div {
                                    class: "resource-card-head",
                                    strong { "资源管理器" }
                                    button {
                                        class: "tiny-pill",
                                        title: "新建连接",
                                        onclick: move |_| {
                                            dialog_mode.set("new".to_string());
                                            edit_name.set(String::new());
                                            edit_host.set(String::new());
                                            edit_port.set("22".to_string());
                                            edit_user.set(String::new());
                                            edit_password.set(String::new());
                                            show_dialog.set(true);
                                        },
                                        "＋"
                                    }
                                }

                                div {
                                    class: "panel-search",
                                    span { "⌕" }
                                    input {
                                        placeholder: "搜索主机、标签",
                                    }
                                }

                                div {
                                    class: "connection-tree",

                                    div {
                                        class: "tree-group",
                                        div {
                                            class: "tree-group-title",
                                            span { "⌄" }
                                            "我的连接"
                                            button { class: "ghost-more", title: "更多", "…" }
                                        }

                                        if saved_profiles.is_empty() {
                                            div {
                                                class: "empty-resource",
                                                strong { "暂无保存连接" }
                                                p { "新建连接后会显示在这里。" }
                                            }
                                        }

                                        for profile in saved_profiles {
                                            ConnectionCard {
                                                key: "saved-{profile.name}",
                                                profile: profile.clone(),
                                                active: active_title == profile.name,
                                                on_connect: {
                                                    let profile = profile.clone();
                                                    move |_| {
                                                        if let Ok(mut app_state) = state.lock() {
                                                            let id = app_state.next_session_id();
                                                            app_state.sessions.insert(
                                                                id,
                                                                session_state_from_profile(id, &profile),
                                                            );
                                                            app_state.manager.send(ToCore::Connect {
                                                                id,
                                                                params: Box::new(profile.params.clone()),
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
                                                        edit_name.set(profile.name.clone());
                                                        edit_host.set(profile.params.host.clone());
                                                        edit_port.set(profile.params.port.to_string());
                                                        edit_user.set(profile.params.user.clone());
                                                        edit_password.set(String::new());
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
                                            }
                                        }
                                    }
                                }

                                div {
                                    class: "resource-footer",
                                    button { title: "筛选", "≡" }
                                    button { title: "连接设置", "⚙" }
                                }
                            }
                        }

                        div {
                            class: if is_resizing_target(active_resize(), ResizeTarget::Resource) {
                                "splitter is-active"
                            } else {
                                "splitter"
                            },
                            title: "拖动调整资源管理器宽度",
                            onmousedown: move |evt| {
                                evt.stop_propagation();
                                evt.prevent_default();
                                active_resize.set(Some(ResizeDrag {
                                    target: ResizeTarget::Resource,
                                    start_x: evt.client_coordinates().x,
                                    start_width: resource_width(),
                                }));
                            },
                        }

                        div {
                            class: "session-stage",

                            div {
                                class: "stage-main",

                                section {
                                    class: "terminal-panel",

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
                                                    class: if sess.connected { "status-dot online" } else { "status-dot connecting" },
                                                }
                                                span { class: "tab-title", "{sess.title}" }
                                                button {
                                                    class: "tab-close",
                                                    title: "关闭会话",
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
                                                    "×"
                                                }
                                            }
                                        }

                                        button {
                                            class: "new-tab-button",
                                            title: "新建连接",
                                            onclick: move |_| {
                                                dialog_mode.set("new".to_string());
                                                edit_name.set(String::new());
                                                edit_host.set(String::new());
                                                edit_port.set("22".to_string());
                                                edit_user.set(String::new());
                                                edit_password.set(String::new());
                                                show_dialog.set(true);
                                            },
                                            "+"
                                        }
                                    }

                                    div {
                                        class: "terminal-toolbar",

                                        div {
                                            class: "breadcrumb",
                                            span { class: "protocol-badge", "ssh" }
                                            span { class: "chevron", "›" }
                                            if let Some(sess) = active_session() {
                                                span {
                                                    class: "host-pill",
                                                    span { class: if sess.connected { "status-dot online" } else { "status-dot connecting" } }
                                                    "{sess.title}"
                                                }
                                            } else {
                                                span { class: "host-pill muted", "未连接" }
                                            }
                                        }

                                        div { class: "toolbar-spacer" }
                                        button { class: "icon-button slim", title: "分屏", "＋" }
                                        button { class: "icon-button slim", title: "水平分屏", "▣" }
                                        button { class: "icon-button slim", title: "垂直分屏", "▦" }
                                        button { class: "icon-button slim", title: "清空", "⌫" }
                                        button { class: "icon-button slim", title: "更多", "…" }
                                    }

                                    div {
                                        class: "terminal-body",

                                        if let Some(sess) = active_session() {
                                            if let Some(snapshot) = sess.snapshot.clone() {
                                                Terminal {
                                                    snapshot: SnapshotWrapper(snapshot),
                                                    session_id: sess.id,
                                                }
                                            } else {
                                                TerminalPlaceholder {
                                                    connected: sess.connected,
                                                    title: sess.title.clone(),
                                                }
                                            }
                                        } else {
                                            EmptyWorkbench {}
                                        }
                                    }
                                }

                                if show_sftp() {
                                    if let Some(sess) = active_session() {
                                        div {
                                            class: if is_resizing_target(active_resize(), ResizeTarget::Sftp) {
                                                "splitter splitter-right is-active"
                                            } else {
                                                "splitter splitter-right"
                                            },
                                            title: "拖动调整 SFTP 宽度",
                                            onmousedown: move |evt| {
                                                evt.stop_propagation();
                                                evt.prevent_default();
                                                active_resize.set(Some(ResizeDrag {
                                                    target: ResizeTarget::Sftp,
                                                    start_x: evt.client_coordinates().x,
                                                    start_width: sftp_width(),
                                                }));
                                            },
                                        }

                                        aside {
                                            class: "sftp-shell",
                                            style: "{sftp_panel_style}",
                                            SftpPanel {
                                                key: "sftp-{sess.id.0}",
                                                session_id: sess.id,
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if show_monitor() {
                        div {
                            class: "monitor-dock",
                            if let Some(sess) = active_session() {
                                MonitorPanel {
                                    key: "monitor-{sess.id.0}",
                                    session_id: sess.id,
                                }
                            } else {
                                MonitorPlaceholder {}
                            }
                        }
                    }
                }
            }

            footer {
                class: "status-bar",
                if let Some(sess) = active_session() {
                    span { class: if sess.connected { "status-pill connected" } else { "status-pill pending" }, if sess.connected { "已连接" } else { "连接中" } }
                    span { "{sess.title}" }
                    span { class: "status-separator" }
                    span { "SSH 连接" }
                    span { class: "status-grow" }
                    span { "UTF-8" }
                    span { class: "status-separator" }
                    span { "100x30" }
                    span { class: "latency", "28ms" }
                } else {
                    span { class: "status-pill pending", "就绪" }
                    span { "选择或新建连接以开始" }
                }
            }
        }

        ConnectionDialog {
            show: show_dialog,
            mode: dialog_mode,
            name: edit_name,
            host: edit_host,
            port: edit_port,
            user: edit_user,
            password: edit_password,
            on_save: move |profile: SessionProfile| {
                if let Err(e) = store.save_session(profile.clone()) {
                    tracing::error!("保存连接失败: {}", e);
                } else {
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
    }
}

fn session_state_from_profile(id: SessionId, profile: &SessionProfile) -> SessionState {
    SessionState {
        id,
        title: profile.name.clone(),
        snapshot: None,
        connected: false,
        sftp_path: ".".to_string(),
        sftp_entries: Vec::new(),
        sftp_loading: false,
        sftp_error: None,
        monitor: None,
    }
}

fn panel_width_after_drag(
    start_width: f64,
    start_x: f64,
    current_x: f64,
    target: ResizeTarget,
    min_width: f64,
    max_width: f64,
) -> f64 {
    let delta = current_x - start_x;
    let raw = match target {
        ResizeTarget::Resource => start_width + delta,
        ResizeTarget::Sftp => start_width - delta,
    };
    clamp_panel_width(raw, min_width, max_width)
}

fn clamp_panel_width(width: f64, min_width: f64, max_width: f64) -> f64 {
    if width.is_finite() {
        width.clamp(min_width, max_width)
    } else {
        min_width
    }
}

fn is_resizing_target(drag: Option<ResizeDrag>, target: ResizeTarget) -> bool {
    drag.is_some_and(|drag| drag.target == target)
}

#[component]
fn ConnectionCard(
    profile: SessionProfile,
    active: bool,
    on_connect: EventHandler<()>,
    on_edit: EventHandler<()>,
    on_delete: EventHandler<()>,
) -> Element {
    let subtitle = format!(
        "{}@{}",
        profile.params.user,
        if profile.params.port == 22 {
            profile.params.host.clone()
        } else {
            format!("{}:{}", profile.params.host, profile.params.port)
        }
    );

    rsx! {
        div {
            class: if active { "connection-card is-active" } else { "connection-card" },

            div {
                class: "connection-main",
                onclick: move |_| on_connect.call(()),

                span { class: "status-dot online" }
                div {
                    class: "connection-copy",
                    strong { "{profile.name}" }
                    small { "{subtitle}" }
                }
            }

            div {
                class: "connection-actions",
                button {
                    title: "编辑",
                    onclick: move |evt| {
                        evt.stop_propagation();
                        on_edit.call(());
                    },
                    "✎"
                }
                button {
                    title: "删除",
                    onclick: move |evt| {
                        evt.stop_propagation();
                        on_delete.call(());
                    },
                    "×"
                }
            }
        }
    }
}

#[component]
fn TerminalPlaceholder(connected: bool, title: String) -> Element {
    let state_line = if connected {
        "正在等待终端输出"
    } else {
        "正在建立 SSH 连接"
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
            p { "会话: {title}" }
            p { "{state_line}" }
            span { class: "terminal-caret" }
        }
    }
}

#[component]
fn EmptyWorkbench() -> Element {
    rsx! {
        div {
            class: "empty-workbench",
            div { class: "empty-logo", "KT" }
            h2 { "选择一个连接" }
            p { "左侧资源管理器会打开 SSH 会话、SFTP 与监控视图。" }
        }
    }
}

#[component]
fn MonitorPlaceholder() -> Element {
    rsx! {
        div {
            class: "monitor-panel compact",
            div { class: "monitor-title", "系统监控" }
            div {
                class: "monitor-grid",
                for label in ["CPU", "内存", "负载", "网络"] {
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

#[cfg(test)]
mod tests {
    use super::*;
    use kt_config::{AuthMethod, ConnectParams, SessionProfile};

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
            },
        };

        let state = session_state_from_profile(SessionId(7), &profile);

        assert_eq!(state.id, SessionId(7));
        assert_eq!(state.title, "Web Server 01");
        assert!(!state.connected);
        assert_eq!(state.sftp_path, ".");
        assert!(state.sftp_entries.is_empty());
        assert!(state.snapshot.is_none());
        assert!(state.monitor.is_none());
    }

    #[test]
    fn resource_panel_drag_grows_to_the_right() {
        let width =
            panel_width_after_drag(206.0, 100.0, 140.0, ResizeTarget::Resource, 176.0, 320.0);
        assert_eq!(width, 246.0);
    }

    #[test]
    fn sftp_panel_drag_grows_to_the_left() {
        let width = panel_width_after_drag(330.0, 500.0, 450.0, ResizeTarget::Sftp, 280.0, 500.0);
        assert_eq!(width, 380.0);
    }

    #[test]
    fn panel_drag_width_is_clamped() {
        assert_eq!(
            panel_width_after_drag(206.0, 100.0, -100.0, ResizeTarget::Resource, 176.0, 320.0),
            176.0
        );
        assert_eq!(
            panel_width_after_drag(330.0, 500.0, -300.0, ResizeTarget::Sftp, 280.0, 500.0),
            500.0
        );
    }

    #[test]
    fn resize_target_detection_checks_only_target() {
        let drag = Some(ResizeDrag {
            target: ResizeTarget::Sftp,
            start_x: 24.0,
            start_width: 330.0,
        });

        assert!(is_resizing_target(drag, ResizeTarget::Sftp));
        assert!(!is_resizing_target(drag, ResizeTarget::Resource));
        assert!(!is_resizing_target(None, ResizeTarget::Sftp));
    }
}
