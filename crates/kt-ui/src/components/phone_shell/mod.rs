//! 手机端主界面。
//!
//! 触屏手机与桌面/平板的差异大到无法用样式弥合：没有软键盘就无法输入，没有右键就
//! 够不到 SFTP 与服务器的绝大多数功能，多栏工作台在 6 寸屏上每一栏都放不下。因此
//! 手机走这套独立 Shell，平板与桌面继续用 [`crate::components::main_shell`]。
//! 由 [`crate::device`] 在运行时按视口短边分派。
//!
//! 布局为「顶栏 + 全屏视图 + 底部标签栏」，四个视图各自独占屏幕：
//! 服务器 / 终端 / 文件 / 监控。
//!
//! **hook 约束**：本函数与 `render_main_shell` 一样必须保持无 hook。设备类型在旋转
//! 或折叠屏展开时会切换，两个 Shell 在同一层条件渲染，任何一侧在此层调用 hook 都会
//! 破坏 Dioxus 的 hook 顺序。需要局部状态的部分一律下沉到 `#[component]`。

mod action_sheet;
mod files_view;
mod keyboard;
mod servers_view;
mod tab;
mod terminal_view;

use std::sync::{Arc, Mutex};

use dioxus::prelude::*;
use kt_core::{SessionId, SftpRequest, ToCore};

use action_sheet::{phone_sheet_actions, PhoneActionSheet, SheetActionId};
use files_view::{render_files_view, FilesViewArgs};
use keyboard::blur_phone_keyboard;
use servers_view::{render_servers_view, ServersViewArgs};
use terminal_view::{
    render_terminal_view, sync_terminal_to_sftp_path, terminal_pane_id, TerminalViewArgs,
};

pub use action_sheet::PhoneSheet;
pub use tab::{PhoneTab, PHONE_TABS};

use crate::components::app_logic::duplicate_profile;
use crate::components::icons::Icon;
use crate::components::main_shell::{ConnectionDialogSignals, ShellArgs};
use crate::components::operations::OperationsPanel;
use crate::components::sftp::join_path;
use crate::components::sidebar::SftpEntryContext;
use crate::components::terminal::{
    copy_selected_terminal_text, paste_clipboard_to_terminal, select_terminal_contents,
    terminal_screen_id,
};
use crate::i18n::texts;
use crate::state::AppState;

/// 手机端独有、`ShellArgs` 中没有的入口。SFTP 命名对话框由 `app.rs` 持有信号，
/// 桌面端经右键菜单打开，手机端经动作面板打开同一个对话框。
pub struct PhoneExtras {
    pub phone_tab: Signal<PhoneTab>,
    pub phone_sheet: Signal<Option<PhoneSheet>>,
    pub on_sftp_mkdir: Callback<(SessionId, String)>,
    pub on_sftp_rename: Callback<SftpEntryContext>,
    pub on_sftp_inline_edit: Callback<SftpEntryContext>,
}

pub fn render_phone_shell(args: ShellArgs, extras: PhoneExtras) -> Element {
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
        mut show_group_dialog,
        mut group_dialog_mode,
        mut group_dialog_name,
        mut group_dialog_original,
        active_session_id,
        saved_tick,
        split_mode,
        on_sftp_entry_open,
        ..
    } = args;

    let PhoneExtras {
        mut phone_tab,
        mut phone_sheet,
        on_sftp_mkdir,
        on_sftp_rename,
        on_sftp_inline_edit,
    } = extras;

    let t = texts(language);
    let has_session = !session_tabs.is_empty();
    let tab = tab::resolved_tab(phone_tab(), has_session);
    let active_id = active_terminal.as_ref().map(|session| session.id);
    let connected = active_terminal
        .as_ref()
        .map(|session| session.connected)
        .unwrap_or(false);
    let title = active_terminal
        .as_ref()
        .map(|session| session.title.clone())
        .unwrap_or_else(|| t.phone.no_session_short.to_string());
    let active_profile_title = active_terminal
        .as_ref()
        .map(|session| session.title.clone());

    let on_new_connection = Callback::new(move |_| dialog_signals.open_new());
    let on_new_group = Callback::new(move |_| {
        group_dialog_mode.set("new".to_string());
        group_dialog_original.set(String::new());
        group_dialog_name.set(String::new());
        show_group_dialog.set(true);
    });

    rsx! {
        div {
            class: "phone-shell",

            header {
                class: "phone-topbar",

                button {
                    class: "phone-session-picker",
                    disabled: !has_session,
                    "aria-label": "{t.phone.switch_session}",
                    onclick: move |_| phone_sheet.set(Some(PhoneSheet::Sessions)),
                    span { class: "phone-session-title", "{title}" }
                    if has_session {
                        Icon { name: "chevron-down" }
                    }
                }

                button {
                    class: "phone-topbar-button",
                    "aria-label": "{t.app.settings}",
                    onclick: move |_| on_settings_open.call(()),
                    Icon { name: "settings" }
                }

                if let Some(id) = active_id {
                    button {
                        class: "phone-topbar-button",
                        "aria-label": "{t.phone.more_actions}",
                        onclick: move |_| phone_sheet.set(Some(PhoneSheet::Terminal(id))),
                        "⋮"
                    }
                }
            }

            main {
                class: "phone-main",

                match tab {
                    PhoneTab::Servers => render_servers_view(ServersViewArgs {
                        state: Arc::clone(&state),
                        settings,
                        language,
                        saved_profiles: saved_profiles.clone(),
                        saved_groups,
                        active_profile_title,
                        active_session_id,
                        phone_tab,
                        phone_sheet,
                        on_new_connection,
                        on_new_group,
                    }),
                    PhoneTab::Terminal => render_terminal_view(TerminalViewArgs {
                        settings,
                        language,
                        active_terminal: active_terminal.clone(),
                        split_mode,
                    }),
                    PhoneTab::Files => render_files_view(FilesViewArgs {
                        state: Arc::clone(&state),
                        store: Arc::clone(&store),
                        settings,
                        language,
                        active_sftp,
                        phone_sheet,
                        on_entry_open: on_sftp_entry_open,
                        on_mkdir: on_sftp_mkdir,
                    }),
                    PhoneTab::Monitor => rsx! {
                        div {
                            class: "phone-view phone-view-monitor",
                            OperationsPanel {
                                session_id: active_monitor.as_ref().map(|monitor| monitor.session_id),
                                connected,
                                language,
                                mobile: true,
                                settings,
                            }
                        }
                    },
                }
            }

            nav {
                class: "phone-tabbar",

                for entry in PHONE_TABS {
                    button {
                        key: "{entry.label(&t.phone)}",
                        class: if entry == tab { "phone-tab is-active" } else { "phone-tab" },
                        disabled: entry.requires_session() && !has_session,
                        onclick: move |_| {
                            // 离开终端页时收起软键盘，否则键盘会盖住新页面。
                            if tab == PhoneTab::Terminal && entry != PhoneTab::Terminal {
                                if let Some(id) = active_id {
                                    blur_phone_keyboard(id);
                                }
                            }
                            phone_tab.set(entry);
                        },
                        Icon { name: entry.icon() }
                        span { "{entry.label(&t.phone)}" }
                    }
                }
            }

            if let Some(sheet) = phone_sheet() {
                {
                    let (sheet_title, actions) =
                        phone_sheet_actions(&sheet, &session_tabs, connected, language);
                    let state_for_action = Arc::clone(&state);
                    let store_for_action = Arc::clone(&store);
                    let profiles_for_action = saved_profiles.clone();
                    rsx! {
                        PhoneActionSheet {
                            title: sheet_title,
                            actions,
                            on_dismiss: move |_| phone_sheet.set(None),
                            on_action: move |id: SheetActionId| {
                                apply_sheet_action(SheetActionArgs {
                                    action: id,
                                    sheet: sheet.clone(),
                                    state: Arc::clone(&state_for_action),
                                    store: Arc::clone(&store_for_action),
                                    saved_profiles: profiles_for_action.clone(),
                                    settings,
                                    language,
                                    dialog_signals,
                                    group_dialog_mode,
                                    group_dialog_name,
                                    group_dialog_original,
                                    show_group_dialog,
                                    active_session_id,
                                    phone_tab,
                                    saved_tick,
                                    on_sftp_entry_open,
                                    on_sftp_rename,
                                    on_sftp_inline_edit,
                                });
                                phone_sheet.set(None);
                            },
                        }
                    }
                }
            }

            if let Some(status) = status_detail {
                aside {
                    class: "phone-status-notification",
                    role: "status",
                    span { "{status}" }
                    button {
                        class: "phone-status-close",
                        "aria-label": "{t.app.close}",
                        onclick: move |evt| {
                            evt.stop_propagation();
                            on_status_dismiss.call(());
                        },
                        Icon { name: "close" }
                    }
                }
            }
        }
    }
}

struct SheetActionArgs {
    action: SheetActionId,
    sheet: PhoneSheet,
    state: Arc<Mutex<AppState>>,
    store: Arc<crate::store::Store>,
    saved_profiles: Vec<kt_config::SessionProfile>,
    settings: Signal<kt_config::AppSettings>,
    language: kt_config::AppLanguage,
    dialog_signals: ConnectionDialogSignals,
    group_dialog_mode: Signal<String>,
    group_dialog_name: Signal<String>,
    group_dialog_original: Signal<String>,
    show_group_dialog: Signal<bool>,
    active_session_id: Signal<Option<SessionId>>,
    phone_tab: Signal<PhoneTab>,
    saved_tick: Signal<u64>,
    on_sftp_entry_open: Callback<SftpEntryContext>,
    on_sftp_rename: Callback<SftpEntryContext>,
    on_sftp_inline_edit: Callback<SftpEntryContext>,
}

fn apply_sheet_action(args: SheetActionArgs) {
    let SheetActionArgs {
        action,
        sheet,
        state,
        store,
        saved_profiles,
        settings,
        language,
        dialog_signals,
        mut group_dialog_mode,
        mut group_dialog_name,
        mut group_dialog_original,
        mut show_group_dialog,
        mut active_session_id,
        mut phone_tab,
        mut saved_tick,
        on_sftp_entry_open,
        on_sftp_rename,
        on_sftp_inline_edit,
    } = args;

    match (action, &sheet) {
        (SheetActionId::Connect, PhoneSheet::Profile(profile)) => {
            let current = settings.peek().clone();
            if let Some(id) = servers_view::connect_profile(&state, profile, &current) {
                active_session_id.set(Some(id));
                phone_tab.set(tab::tab_after_connect());
            }
        }
        (SheetActionId::Edit, PhoneSheet::Profile(profile)) => dialog_signals.open_edit(profile),
        (SheetActionId::Duplicate, PhoneSheet::Profile(profile)) => {
            let duplicate = duplicate_profile(profile, &saved_profiles);
            match store.save_session(duplicate) {
                Ok(()) => saved_tick.set(saved_tick() + 1),
                Err(error) => tracing::error!("复制连接失败: {error}"),
            }
        }
        (SheetActionId::Delete, PhoneSheet::Profile(profile)) => {
            match store.delete_session(&profile.name) {
                Ok(()) => saved_tick.set(saved_tick() + 1),
                Err(error) => tracing::error!("删除连接失败: {error}"),
            }
        }
        (SheetActionId::GroupRename, PhoneSheet::Group(name)) => {
            group_dialog_mode.set("rename".to_string());
            group_dialog_original.set(name.clone());
            group_dialog_name.set(
                if name == crate::components::app_logic::DEFAULT_GROUP_NAME {
                    String::new()
                } else {
                    name.clone()
                },
            );
            show_group_dialog.set(true);
        }
        (SheetActionId::GroupDelete, PhoneSheet::Group(name))
            if name != crate::components::app_logic::DEFAULT_GROUP_NAME =>
        {
            match store.delete_group(name) {
                Ok(()) => saved_tick.set(saved_tick() + 1),
                Err(error) => tracing::error!("删除分组失败: {error}"),
            }
        }
        (SheetActionId::Open, PhoneSheet::SftpEntry(ctx)) => on_sftp_entry_open.call(ctx.clone()),
        (SheetActionId::InlineEdit, PhoneSheet::SftpEntry(ctx)) => {
            on_sftp_inline_edit.call(ctx.clone())
        }
        (SheetActionId::Rename, PhoneSheet::SftpEntry(ctx)) => on_sftp_rename.call(ctx.clone()),
        (SheetActionId::CopyPath, PhoneSheet::SftpEntry(ctx)) => {
            crate::components::app::copy_to_clipboard(&join_path(&ctx.base_path, &ctx.entry.name));
        }
        (SheetActionId::CopyName, PhoneSheet::SftpEntry(ctx)) => {
            crate::components::app::copy_to_clipboard(&ctx.entry.name);
        }
        (SheetActionId::Delete, PhoneSheet::SftpEntry(ctx)) => {
            let path = join_path(&ctx.base_path, &ctx.entry.name);
            if let Ok(mut app_state) = state.lock() {
                if let Err(error) = app_state.send_sftp_request(
                    ctx.session_id,
                    SftpRequest::Remove {
                        path,
                        is_dir: ctx.entry.is_dir,
                    },
                ) {
                    tracing::error!("SFTP 删除请求投递失败: {error}");
                }
            }
        }
        (SheetActionId::Copy, PhoneSheet::Terminal(id)) => {
            copy_selected_terminal_text(&terminal_pane_id(*id));
        }
        (SheetActionId::Paste, PhoneSheet::Terminal(id)) => {
            paste_clipboard_to_terminal(Arc::clone(&state), *id);
        }
        (SheetActionId::SelectAll, PhoneSheet::Terminal(id)) => {
            select_terminal_contents(&terminal_screen_id(&terminal_pane_id(*id)));
        }
        (SheetActionId::SyncToTerminal, PhoneSheet::Terminal(id)) => {
            sync_terminal_to_sftp_path(&state, *id, language);
        }
        (SheetActionId::Reconnect, PhoneSheet::Terminal(id)) => {
            if let Ok(mut app_state) = state.lock() {
                if let Err(error) = app_state.connect_session(*id) {
                    tracing::error!("会话重连失败: {error}");
                }
            }
        }
        (SheetActionId::Disconnect, PhoneSheet::Terminal(id)) => {
            close_session(&state, *id, active_session_id);
        }
        (SheetActionId::SelectSession(id), PhoneSheet::Sessions) => {
            active_session_id.set(Some(id));
            phone_tab.set(tab::tab_after_connect());
        }
        (SheetActionId::NewConnection, PhoneSheet::Sessions) => dialog_signals.open_new(),
        // 动作与面板类型不匹配时不做任何事，不猜测用户意图。
        _ => {}
    }
}

fn close_session(
    state: &Arc<Mutex<AppState>>,
    id: SessionId,
    mut active_session_id: Signal<Option<SessionId>>,
) {
    if let Ok(mut app_state) = state.lock() {
        app_state.manager.send(ToCore::Disconnect { id });
        app_state.sessions.remove(&id);
        if active_session_id() == Some(id) {
            active_session_id.set(app_state.sessions.keys().next().copied());
        }
    }
}
