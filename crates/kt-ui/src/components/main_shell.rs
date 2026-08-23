//! App 主工作台布局渲染。

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
mod desktop_titlebar;
mod sidebar_panel;
mod status_bar;
mod workbench_panel;

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use dioxus::prelude::*;
use kt_config::{
    normalize_theme_name, AppLanguage, AppSettings, AuthMethod, SessionProfile, DEFAULT_LIGHT_THEME,
};
use kt_core::{SessionId, ToCore};

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
use desktop_titlebar::DesktopTitlebar;
use sidebar_panel::{render_sidebar_panel, SidebarPanelArgs};
use status_bar::{render_status_bar, StatusBarArgs};
use workbench_panel::{render_workbench_panel, WorkbenchPanelArgs};

use crate::components::app_logic::{
    ActiveMonitorView, ActiveSftpView, ActiveTerminalView, SessionTabView,
};
use crate::components::dialog::first_public_key_path;
use crate::components::icons::Icon;
use crate::components::sidebar::{ContextMenuState, SftpEntryContext};
use crate::i18n::{texts, AppText};
use crate::state::AppState;
use crate::store::Store;

pub const SIDEBAR_DEFAULT_WIDTH: f64 = 220.0;
pub const SIDEBAR_MIN_WIDTH: f64 = 176.0;
pub const SIDEBAR_MAX_WIDTH: f64 = 320.0;
pub const SFTP_DEFAULT_HEIGHT: f64 = 320.0;
pub const SFTP_MIN_HEIGHT: f64 = 120.0;
pub const SFTP_MAX_HEIGHT: f64 = 420.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResizeDrag {
    SidebarWidth { start_x: f64, start_width: f64 },
    SftpHeight { start_y: f64, start_height: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitMode {
    Horizontal,
    Vertical,
}

pub fn theme_class(theme: &str) -> &'static str {
    if normalize_theme_name(theme) == DEFAULT_LIGHT_THEME {
        "theme-light"
    } else {
        "theme-dark"
    }
}

/// 顶层窗口 class。`is_phone` 让样式能区分手机 Shell 与桌面工作台：手机上不画窗口
/// 边框与渐变背景，交给 [`crate::components::phone_shell`] 自己的全屏布局。
pub fn window_class(active_resize: Option<ResizeDrag>, theme: &str, is_phone: bool) -> String {
    let resize_class = match active_resize {
        Some(ResizeDrag::SidebarWidth { .. }) => " is-resizing is-resizing-x",
        Some(ResizeDrag::SftpHeight { .. }) => " is-resizing is-resizing-y",
        None => "",
    };
    let device_class = if is_phone { " is-phone" } else { "" };
    format!(
        "kt-window {}{}{}",
        theme_class(theme),
        device_class,
        resize_class
    )
}

fn status_notification_text(status_detail: Option<String>, text: &AppText) -> Option<String> {
    status_detail.filter(|status| {
        status != text.ready_hint
            && !(status.contains(text.connected) && status.contains(text.ready))
    })
}

fn toggled_sidebar_collapsed(sidebar_collapsed: bool) -> bool {
    !sidebar_collapsed
}

pub(super) struct DesktopTitlebarArgs {
    language: AppLanguage,
    session_tabs: Vec<SessionTabView>,
    active_session_id: Signal<Option<SessionId>>,
    sidebar_collapsed: bool,
    on_sidebar_toggle: Callback<()>,
    on_settings_open: Callback<()>,
    on_session_close: Callback<SessionId>,
    on_session_reconnect: Callback<SessionId>,
    on_new_connection: Callback<()>,
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn render_desktop_titlebar(args: DesktopTitlebarArgs) -> Element {
    let DesktopTitlebarArgs {
        language,
        session_tabs,
        active_session_id,
        sidebar_collapsed,
        on_sidebar_toggle,
        on_settings_open,
        on_session_close,
        on_session_reconnect,
        on_new_connection,
    } = args;
    rsx! {
        DesktopTitlebar {
            language,
            session_tabs,
            active_session_id,
            sidebar_collapsed,
            on_sidebar_toggle,
            on_settings_open,
            on_session_close,
            on_session_reconnect,
            on_new_connection,
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn render_desktop_titlebar(_args: DesktopTitlebarArgs) -> Element {
    rsx! {}
}

/// 主界面渲染入参。桌面/平板由 [`render_main_shell`] 消费，手机由
/// [`crate::components::phone_shell::render_phone_shell`] 消费；两套 Shell 需要的
/// 全局状态几乎一致，共用同一个入参结构避免在 `app.rs` 里维护两份。
/// `sidebar_width` / `sftp_height` / `active_resize` / `split_mode` 等是桌面布局状态，
/// 手机 Shell 会忽略。
pub struct ShellArgs {
    pub state: Arc<Mutex<AppState>>,
    pub store: Arc<Store>,
    pub settings: Signal<AppSettings>,
    pub language: AppLanguage,
    pub saved_profiles: Vec<SessionProfile>,
    pub saved_groups: Vec<String>,
    pub active_terminal: Option<ActiveTerminalView>,
    pub active_sftp: Option<ActiveSftpView>,
    pub active_monitor: Option<ActiveMonitorView>,
    pub session_tabs: Vec<SessionTabView>,
    pub status_detail: Option<String>,
    pub on_status_dismiss: Callback<()>,
    pub on_settings_open: Callback<()>,
    pub show_dialog: Signal<bool>,
    pub dialog_mode: Signal<String>,
    pub edit_original_name: Signal<String>,
    pub edit_name: Signal<String>,
    pub edit_host: Signal<String>,
    pub edit_port: Signal<String>,
    pub edit_user: Signal<String>,
    pub edit_group: Signal<String>,
    pub edit_password: Signal<String>,
    pub edit_key_path: Signal<String>,
    pub edit_proxy_jump: Signal<String>,
    pub edit_proxy_type: Signal<String>,
    pub edit_proxy_host: Signal<String>,
    pub edit_proxy_port: Signal<String>,
    pub edit_proxy_username: Signal<String>,
    pub edit_use_agent: Signal<bool>,
    pub edit_forward_agent: Signal<bool>,
    pub show_group_dialog: Signal<bool>,
    pub group_dialog_mode: Signal<String>,
    pub group_dialog_name: Signal<String>,
    pub group_dialog_original: Signal<String>,
    pub active_session_id: Signal<Option<SessionId>>,
    pub saved_tick: Signal<u64>,
    pub sidebar_width: Signal<f64>,
    pub sftp_height: Signal<Option<f64>>,
    pub active_resize: Signal<Option<ResizeDrag>>,
    pub context_menu: Signal<Option<ContextMenuState>>,
    pub collapsed_server_groups: Signal<BTreeSet<String>>,
    pub sidebar_collapsed: Signal<bool>,
    pub split_mode: Signal<Option<SplitMode>>,
    pub on_sftp_entry_open: Callback<SftpEntryContext>,
    pub on_sftp_entry_external_edit: Callback<SftpEntryContext>,
}

#[derive(Clone, Copy)]
pub struct ConnectionDialogSignals {
    show_dialog: Signal<bool>,
    dialog_mode: Signal<String>,
    edit_original_name: Signal<String>,
    edit_name: Signal<String>,
    edit_host: Signal<String>,
    edit_port: Signal<String>,
    edit_user: Signal<String>,
    edit_group: Signal<String>,
    edit_password: Signal<String>,
    edit_key_path: Signal<String>,
    edit_proxy_jump: Signal<String>,
    edit_proxy_type: Signal<String>,
    edit_proxy_host: Signal<String>,
    edit_proxy_port: Signal<String>,
    edit_proxy_username: Signal<String>,
    edit_use_agent: Signal<bool>,
    edit_forward_agent: Signal<bool>,
}

impl ConnectionDialogSignals {
    /// 从共用的 [`ShellArgs`] 取出连接对话框相关的信号。信号是 `Copy`，两套 Shell
    /// 都从这里构造，保证「新建/编辑连接」在手机与桌面上打开的是同一个对话框。
    pub fn from_shell_args(args: &ShellArgs) -> Self {
        ConnectionDialogSignals {
            show_dialog: args.show_dialog,
            dialog_mode: args.dialog_mode,
            edit_original_name: args.edit_original_name,
            edit_name: args.edit_name,
            edit_host: args.edit_host,
            edit_port: args.edit_port,
            edit_user: args.edit_user,
            edit_group: args.edit_group,
            edit_password: args.edit_password,
            edit_key_path: args.edit_key_path,
            edit_proxy_jump: args.edit_proxy_jump,
            edit_proxy_type: args.edit_proxy_type,
            edit_proxy_host: args.edit_proxy_host,
            edit_proxy_port: args.edit_proxy_port,
            edit_proxy_username: args.edit_proxy_username,
            edit_use_agent: args.edit_use_agent,
            edit_forward_agent: args.edit_forward_agent,
        }
    }

    pub(super) fn open_new(mut self) {
        self.dialog_mode.set("new".to_string());
        self.edit_original_name.set(String::new());
        self.edit_name.set(String::new());
        self.edit_host.set(String::new());
        self.edit_port.set("22".to_string());
        self.edit_user.set(String::new());
        self.edit_group.set(String::new());
        self.edit_password.set(String::new());
        self.edit_key_path.set(String::new());
        self.edit_proxy_jump.set(String::new());
        self.edit_proxy_type.set("direct".to_string());
        self.edit_proxy_host.set(String::new());
        self.edit_proxy_port.set(String::new());
        self.edit_proxy_username.set(String::new());
        self.edit_use_agent.set(false);
        self.edit_forward_agent.set(false);
        self.show_dialog.set(true);
    }

    pub(super) fn open_edit(mut self, profile: &SessionProfile) {
        self.dialog_mode.set("edit".to_string());
        self.edit_original_name.set(profile.name.clone());
        self.edit_name.set(profile.name.clone());
        self.edit_host.set(profile.params.host.clone());
        self.edit_port.set(profile.params.port.to_string());
        self.edit_user.set(profile.params.user.clone());
        self.edit_group
            .set(profile.group.clone().unwrap_or_default());
        self.edit_password.set(String::new());
        self.edit_key_path
            .set(first_public_key_path(&profile.params.auth));
        self.edit_proxy_jump
            .set(profile.params.proxy_jump.clone().unwrap_or_default());
        self.edit_proxy_type
            .set(crate::components::dialog::proxy_mode(&profile.params).to_string());
        let (proxy_host_val, proxy_port_val, proxy_user_val) =
            crate::components::dialog::proxy_fields(&profile.params.proxy);
        self.edit_proxy_host.set(proxy_host_val);
        self.edit_proxy_port.set(proxy_port_val);
        self.edit_proxy_username.set(proxy_user_val);
        self.edit_use_agent
            .set(profile.params.auth.contains(&AuthMethod::Agent));
        self.edit_forward_agent.set(profile.params.forward_agent);
        self.show_dialog.set(true);
    }
}

pub fn render_main_shell(args: ShellArgs) -> Element {
    let dialog_signals = ConnectionDialogSignals::from_shell_args(&args);
    let ShellArgs {
        state,
        store,
        settings,
        language,
        saved_profiles,
        saved_groups,
        active_terminal,
        active_sftp,
        active_monitor,
        session_tabs,
        status_detail,
        on_status_dismiss,
        on_settings_open,
        show_group_dialog,
        group_dialog_mode,
        group_dialog_name,
        group_dialog_original,
        active_session_id,
        saved_tick,
        sidebar_width,
        sftp_height,
        mut active_resize,
        context_menu,
        collapsed_server_groups,
        mut sidebar_collapsed,
        split_mode,
        on_sftp_entry_open,
        on_sftp_entry_external_edit,
        // 连接对话框的各个 edit_* 信号已由 `from_shell_args` 取走。
        ..
    } = args;

    let t = texts(language).app;
    let active_profile_title = active_terminal
        .as_ref()
        .map(|session| session.title.clone());
    let on_sidebar_toggle = {
        let mut active_resize = active_resize;
        Callback::new(move |_| {
            active_resize.set(None);
            sidebar_collapsed.set(toggled_sidebar_collapsed(sidebar_collapsed()));
        })
    };
    let on_session_close = {
        let state = state.clone();
        let mut active_session_id = active_session_id;
        Callback::new(move |id: SessionId| {
            if let Ok(mut app_state) = state.lock() {
                app_state.manager.send(ToCore::Disconnect { id });
                app_state.sessions.remove(&id);
                if active_session_id() == Some(id) {
                    active_session_id.set(app_state.sessions.keys().next().copied());
                }
            }
        })
    };
    let on_session_reconnect = {
        let state = state.clone();
        Callback::new(move |id: SessionId| {
            if let Ok(mut app_state) = state.lock() {
                if let Err(error) = app_state.connect_session(id) {
                    tracing::error!("会话重连失败: {error}");
                }
            }
        })
    };
    let on_new_connection = Callback::new(move |_| dialog_signals.open_new());

    rsx! {
        {render_desktop_titlebar(DesktopTitlebarArgs {
            language,
            session_tabs: session_tabs.clone(),
            active_session_id,
            sidebar_collapsed: sidebar_collapsed(),
            on_sidebar_toggle,
            on_settings_open,
            on_session_close,
            on_session_reconnect,
            on_new_connection,
        })}

        div {
            class: "kt-content",

            {render_sidebar_panel(SidebarPanelArgs {
                state: state.clone(),
                store: store.clone(),
                settings,
                language,
                saved_profiles,
                saved_groups,
                active_profile_title,
                active_sftp,
                sidebar_collapsed: sidebar_collapsed(),
                dialog_signals,
                show_group_dialog,
                group_dialog_mode,
                group_dialog_name,
                group_dialog_original,
                active_session_id,
                saved_tick,
                sidebar_width,
                sftp_height,
                active_resize,
                context_menu,
                collapsed_server_groups,
                on_sftp_entry_open,
                on_sftp_entry_external_edit,
            })}

            div {
                class: if sidebar_collapsed() {
                    "splitter is-collapsed"
                } else if active_resize().is_some() {
                    "splitter is-active"
                } else {
                    "splitter"
                },
                title: "{t.resize_explorer}",
                onmousedown: move |evt| {
                    evt.stop_propagation();
                    evt.prevent_default();
                    if sidebar_collapsed() {
                        return;
                    }
                    active_resize.set(Some(ResizeDrag::SidebarWidth {
                        start_x: evt.client_coordinates().x,
                        start_width: sidebar_width(),
                    }));
                },
            }

            {render_workbench_panel(WorkbenchPanelArgs {
                state,
                settings,
                language,
                active_terminal,
                session_tabs,
                dialog_signals,
                active_session_id,
                split_mode,
            })}
        }

        {render_status_bar(StatusBarArgs {
            language,
            active_monitor,
        })}

        if let Some(status) = status_notification_text(status_detail, &t) {
            aside {
                class: "status-notification",
                role: "status",
                span { "{status}" }
                button {
                    class: "status-notification-close tooltip-trigger",
                    "data-tooltip": "{t.close}",
                    aria_label: "{t.close}",
                    onclick: move |event| {
                        event.stop_propagation();
                        on_status_dismiss.call(());
                    },
                    Icon { name: "close" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kt_config::{DEFAULT_DARK_THEME, DEFAULT_LIGHT_THEME};

    #[test]
    fn window_class_applies_theme_and_resize_state() {
        assert_eq!(
            window_class(None, DEFAULT_DARK_THEME, false),
            "kt-window theme-dark"
        );
        assert_eq!(
            window_class(None, DEFAULT_LIGHT_THEME, false),
            "kt-window theme-light"
        );
        assert_eq!(
            window_class(
                Some(ResizeDrag::SidebarWidth {
                    start_x: 10.0,
                    start_width: 220.0,
                }),
                DEFAULT_LIGHT_THEME,
                false,
            ),
            "kt-window theme-light is-resizing is-resizing-x"
        );
        assert_eq!(theme_class("unknown-theme"), "theme-dark");
    }

    #[test]
    fn phone_shell_is_marked_on_the_window_element() {
        assert_eq!(
            window_class(None, DEFAULT_DARK_THEME, true),
            "kt-window theme-dark is-phone"
        );
    }

    #[test]
    fn redundant_connection_statuses_are_not_promoted_to_notifications() {
        let text = texts(AppLanguage::Chinese).app;
        assert_eq!(
            status_notification_text(Some(text.ready_hint.to_string()), &text),
            None
        );
        assert_eq!(
            status_notification_text(Some("已连接 生产服务器 (就绪)".to_string()), &text),
            None
        );
        assert_eq!(
            status_notification_text(Some("文件同步失败".to_string()), &text),
            Some("文件同步失败".to_string())
        );
    }

    #[test]
    fn sidebar_toggle_flips_the_visibility_state() {
        assert!(toggled_sidebar_collapsed(false));
        assert!(!toggled_sidebar_collapsed(true));
    }
}
