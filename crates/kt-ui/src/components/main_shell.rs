//! App 主工作台布局渲染。

mod sidebar_panel;
mod status_bar;
mod workbench_panel;

use std::sync::{Arc, Mutex};

use dioxus::prelude::*;
use kt_config::{AppLanguage, AppSettings, AuthMethod, SessionProfile};
use kt_core::SessionId;

use sidebar_panel::{render_sidebar_panel, SidebarPanelArgs};
use status_bar::{render_status_bar, StatusBarArgs};
use workbench_panel::{render_workbench_panel, WorkbenchPanelArgs};

use crate::components::app_logic::{
    ActiveMonitorView, ActiveSftpView, ActiveTerminalView, SessionTabView, StatusBarSessionView,
};
use crate::components::sidebar::ContextMenuState;
use crate::i18n::texts;
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

pub fn window_class(active_resize: Option<ResizeDrag>) -> &'static str {
    match active_resize {
        Some(ResizeDrag::SidebarWidth { .. }) => "kt-window is-resizing is-resizing-x",
        Some(ResizeDrag::SftpHeight { .. }) => "kt-window is-resizing is-resizing-y",
        None => "kt-window",
    }
}

pub struct MainShellArgs {
    pub state: Arc<Mutex<AppState>>,
    pub store: Arc<Store>,
    pub settings: Signal<AppSettings>,
    pub language: AppLanguage,
    pub saved_profiles: Vec<SessionProfile>,
    pub saved_groups: Vec<String>,
    pub active_terminal: Option<ActiveTerminalView>,
    pub active_sftp: Option<ActiveSftpView>,
    pub active_monitor: Option<ActiveMonitorView>,
    pub status_session: Option<StatusBarSessionView>,
    pub session_tabs: Vec<SessionTabView>,
    pub status_detail: Option<String>,
    pub show_dialog: Signal<bool>,
    pub dialog_mode: Signal<String>,
    pub edit_original_name: Signal<String>,
    pub edit_name: Signal<String>,
    pub edit_host: Signal<String>,
    pub edit_port: Signal<String>,
    pub edit_user: Signal<String>,
    pub edit_group: Signal<String>,
    pub edit_password: Signal<String>,
    pub edit_proxy_jump: Signal<String>,
    pub edit_use_agent: Signal<bool>,
    pub edit_forward_agent: Signal<bool>,
    pub show_group_dialog: Signal<bool>,
    pub group_dialog_mode: Signal<String>,
    pub group_dialog_name: Signal<String>,
    pub group_dialog_original: Signal<String>,
    pub show_settings: Signal<bool>,
    pub active_session_id: Signal<Option<SessionId>>,
    pub saved_tick: Signal<u64>,
    pub sidebar_width: Signal<f64>,
    pub sftp_height: Signal<Option<f64>>,
    pub active_resize: Signal<Option<ResizeDrag>>,
    pub context_menu: Signal<Option<ContextMenuState>>,
    pub split_mode: Signal<Option<SplitMode>>,
}

#[derive(Clone, Copy)]
pub(super) struct ConnectionDialogSignals {
    show_dialog: Signal<bool>,
    dialog_mode: Signal<String>,
    edit_original_name: Signal<String>,
    edit_name: Signal<String>,
    edit_host: Signal<String>,
    edit_port: Signal<String>,
    edit_user: Signal<String>,
    edit_group: Signal<String>,
    edit_password: Signal<String>,
    edit_proxy_jump: Signal<String>,
    edit_use_agent: Signal<bool>,
    edit_forward_agent: Signal<bool>,
}

impl ConnectionDialogSignals {
    pub(super) fn open_new(mut self) {
        self.dialog_mode.set("new".to_string());
        self.edit_original_name.set(String::new());
        self.edit_name.set(String::new());
        self.edit_host.set(String::new());
        self.edit_port.set("22".to_string());
        self.edit_user.set(String::new());
        self.edit_group.set(String::new());
        self.edit_password.set(String::new());
        self.edit_proxy_jump.set(String::new());
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
        self.edit_proxy_jump
            .set(profile.params.proxy_jump.clone().unwrap_or_default());
        self.edit_use_agent
            .set(profile.params.auth.contains(&AuthMethod::Agent));
        self.edit_forward_agent.set(profile.params.forward_agent);
        self.show_dialog.set(true);
    }
}

pub fn render_main_shell(args: MainShellArgs) -> Element {
    let MainShellArgs {
        state,
        store,
        settings,
        language,
        saved_profiles,
        saved_groups,
        active_terminal,
        active_sftp,
        active_monitor,
        status_session,
        session_tabs,
        status_detail,
        show_dialog,
        dialog_mode,
        edit_original_name,
        edit_name,
        edit_host,
        edit_port,
        edit_user,
        edit_group,
        edit_password,
        edit_proxy_jump,
        edit_use_agent,
        edit_forward_agent,
        show_group_dialog,
        group_dialog_mode,
        group_dialog_name,
        group_dialog_original,
        show_settings,
        active_session_id,
        saved_tick,
        sidebar_width,
        sftp_height,
        mut active_resize,
        context_menu,
        split_mode,
    } = args;

    let t = texts(language).app;
    let dialog_signals = ConnectionDialogSignals {
        show_dialog,
        dialog_mode,
        edit_original_name,
        edit_name,
        edit_host,
        edit_port,
        edit_user,
        edit_group,
        edit_password,
        edit_proxy_jump,
        edit_use_agent,
        edit_forward_agent,
    };
    let active_profile_title = active_terminal
        .as_ref()
        .map(|session| session.title.clone());

    rsx! {
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
                dialog_signals,
                show_group_dialog,
                group_dialog_mode,
                group_dialog_name,
                group_dialog_original,
                show_settings,
                active_session_id,
                saved_tick,
                sidebar_width,
                sftp_height,
                active_resize,
                context_menu,
            })}

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

            {render_workbench_panel(WorkbenchPanelArgs {
                state,
                settings,
                language,
                active_terminal,
                active_monitor,
                session_tabs,
                dialog_signals,
                active_session_id,
                split_mode,
            })}
        }

        {render_status_bar(StatusBarArgs {
            language,
            status_session,
            status_detail,
        })}
    }
}
