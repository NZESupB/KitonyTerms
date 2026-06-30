//! 主工作台左侧栏渲染。

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use dioxus::prelude::*;
use kt_config::{AppLanguage, AppSettings, SessionProfile};
use kt_core::{PtySize, SessionId, ToCore};

use super::{ConnectionDialogSignals, ResizeDrag, SFTP_DEFAULT_HEIGHT};
use crate::components::app_logic::{
    duplicate_profile, group_profiles, params_with_ssh_config, session_state_from_profile,
    ActiveSftpView,
};
use crate::components::icons::Icon;
use crate::components::sidebar::{
    ConnectionCard, ContextMenuState, ContextMenuTarget, SidebarSftpTree,
};
use crate::i18n::texts;
use crate::state::AppState;
use crate::store::Store;

pub(super) struct SidebarPanelArgs {
    pub(super) state: Arc<Mutex<AppState>>,
    pub(super) store: Arc<Store>,
    pub(super) settings: Signal<AppSettings>,
    pub(super) language: AppLanguage,
    pub(super) saved_profiles: Vec<SessionProfile>,
    pub(super) saved_groups: Vec<String>,
    pub(super) active_profile_title: Option<String>,
    pub(super) active_sftp: Option<ActiveSftpView>,
    pub(super) dialog_signals: ConnectionDialogSignals,
    pub(super) show_group_dialog: Signal<bool>,
    pub(super) group_dialog_mode: Signal<String>,
    pub(super) group_dialog_name: Signal<String>,
    pub(super) group_dialog_original: Signal<String>,
    pub(super) active_session_id: Signal<Option<SessionId>>,
    pub(super) saved_tick: Signal<u64>,
    pub(super) sidebar_width: Signal<f64>,
    pub(super) sftp_height: Signal<Option<f64>>,
    pub(super) active_resize: Signal<Option<ResizeDrag>>,
    pub(super) context_menu: Signal<Option<ContextMenuState>>,
    pub(super) collapsed_server_groups: Signal<BTreeSet<String>>,
}

pub(super) fn render_sidebar_panel(args: SidebarPanelArgs) -> Element {
    let SidebarPanelArgs {
        state,
        store,
        settings,
        language,
        saved_profiles,
        saved_groups,
        active_profile_title,
        active_sftp,
        dialog_signals,
        mut show_group_dialog,
        mut group_dialog_mode,
        mut group_dialog_name,
        mut group_dialog_original,
        mut active_session_id,
        mut saved_tick,
        sidebar_width,
        sftp_height,
        mut active_resize,
        context_menu,
        mut collapsed_server_groups,
    } = args;

    let t = texts(language).app;
    let sidebar_style = format!(
        "width: {:.0}px; flex-basis: {:.0}px;",
        sidebar_width(),
        sidebar_width()
    );
    let sftp_style = sftp_height()
        .map(|height| format!("height: {height:.0}px; flex-basis: {height:.0}px;"))
        .unwrap_or_else(|| "flex: 1 1 0; min-height: 0;".to_string());
    let grouped = group_profiles(&saved_profiles, &saved_groups);

    rsx! {
        aside {
            class: "sidebar",
            style: "{sidebar_style}",

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
                            dialog_signals.open_new();
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
                            class: if collapsed_server_groups().contains(group_name) { "tree-group is-collapsed" } else { "tree-group" },
                            div {
                                class: "tree-group-title",
                                onclick: {
                                    let group_name = group_name.clone();
                                    move |_| {
                                        let mut next = collapsed_server_groups.peek().clone();
                                        toggle_collapsed_group(&mut next, &group_name);
                                        collapsed_server_groups.set(next);
                                    }
                                },
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
                                span { "{group_name}" }
                            }

                            if !collapsed_server_groups().contains(group_name) {
                                for profile in profiles {
                                    ConnectionCard {
                                        key: "saved-{profile.name}",
                                        profile: profile.clone(),
                                        active: active_profile_title.as_ref().map(|title| title == &profile.name).unwrap_or(false),
                                        language,
                                        on_connect: {
                                            let profile = profile.clone();
                                            let state = state.clone();
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
                                                dialog_signals.open_edit(&profile);
                                            }
                                        },
                                        on_delete: {
                                            let name = profile.name.clone();
                                            let store = store.clone();
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
                                            let store = store.clone();
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

            div {
                class: "sidebar-section sidebar-sftp",
                style: "{sftp_style}",

                div {
                    class: "sidebar-section-head",
                    Icon { name: "chevron-down" }
                    span { "{texts(language).sftp.title}" }
                }

                if let Some(sftp) = active_sftp {
                    SidebarSftpTree {
                        key: "sidebar-sftp-{sftp.session_id.0}",
                        session_id: sftp.session_id,
                        connected: sftp.connected,
                        path: sftp.path.clone(),
                        entries: sftp.entries.clone(),
                        loading: sftp.loading,
                        error: sftp.error.clone(),
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

        }
    }
}

fn show_context_menu(mut menu_signal: Signal<Option<ContextMenuState>>, menu: ContextMenuState) {
    menu_signal.set(None);
    spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
        menu_signal.set(Some(menu));
    });
}

pub(crate) fn toggle_collapsed_group(
    collapsed_groups: &mut BTreeSet<String>,
    group_name: &str,
) -> bool {
    if collapsed_groups.contains(group_name) {
        collapsed_groups.remove(group_name);
        false
    } else {
        collapsed_groups.insert(group_name.to_string());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_collapsed_group_flips_membership() {
        let mut collapsed_groups = BTreeSet::new();

        assert!(toggle_collapsed_group(&mut collapsed_groups, "生产"));
        assert!(collapsed_groups.contains("生产"));

        assert!(!toggle_collapsed_group(&mut collapsed_groups, "生产"));
        assert!(!collapsed_groups.contains("生产"));
    }
}
