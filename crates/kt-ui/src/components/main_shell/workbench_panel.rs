//! 主工作台中央区域渲染。

use std::sync::{Arc, Mutex};

use dioxus::prelude::*;
use kt_config::{AppLanguage, AppSettings};
use kt_core::{SessionId, ToCore};

use super::{ConnectionDialogSignals, SplitMode};
use crate::components::app_logic::{
    session_dot_class_for_status, ActiveTerminalView, SessionConnectionStatus, SessionTabView,
};
use crate::components::icons::Icon;
use crate::components::terminal::{SnapshotWrapper, Terminal};
use crate::components::workbench::{EmptyWorkbench, TerminalPlaceholder};
use crate::i18n::texts;
use crate::state::AppState;

pub(super) struct WorkbenchPanelArgs {
    pub(super) state: Arc<Mutex<AppState>>,
    pub(super) settings: Signal<AppSettings>,
    pub(super) language: AppLanguage,
    pub(super) active_terminal: Option<ActiveTerminalView>,
    pub(super) session_tabs: Vec<SessionTabView>,
    pub(super) dialog_signals: ConnectionDialogSignals,
    pub(super) active_session_id: Signal<Option<SessionId>>,
    pub(super) split_mode: Signal<Option<SplitMode>>,
}

pub(super) fn render_workbench_panel(args: WorkbenchPanelArgs) -> Element {
    let WorkbenchPanelArgs {
        state,
        settings,
        language,
        active_terminal,
        session_tabs,
        dialog_signals,
        mut active_session_id,
        split_mode,
    } = args;

    let t = texts(language).app;
    let active_tab_id = active_session_id();

    rsx! {
        div {
            class: "main-column",

            section {
                class: "terminal-panel",

                div {
                    class: "session-tabs workbench-session-tabs",

                    for sess in session_tabs {
                        div {
                            key: "tab-{sess.id.0}",
                            class: if active_tab_id == Some(sess.id) { "session-tab is-active" } else { "session-tab" },
                            onclick: {
                                let id = sess.id;
                                move |_| active_session_id.set(Some(id))
                            },

                            span { class: session_dot_class_for_status(sess.status) }
                            span { class: "tab-title", "{sess.title}" }
                            if active_tab_id == Some(sess.id) {
                                div {
                                    class: "tab-actions",
                                    onclick: move |evt| evt.stop_propagation(),
                                    if matches!(sess.status, SessionConnectionStatus::Disconnected) {
                                        button {
                                            class: "tab-action tooltip-trigger",
                                            "data-tooltip": "{t.reconnect}",
                                            aria_label: "{t.reconnect}",
                                            onclick: {
                                                let id = sess.id;
                                                let state = state.clone();
                                                move |_| {
                                                    if let Ok(mut app_state) = state.lock() {
                                                        if let Err(error) = app_state.connect_session(id) {
                                                            tracing::error!("会话重连失败: {error}");
                                                        }
                                                    }
                                                }
                                            },
                                            Icon { name: "refresh" }
                                        }
                                    }
                                }
                            }
                            button {
                                class: "tab-close tooltip-trigger",
                                "data-tooltip": "{t.close_session}",
                                aria_label: "{t.close_session}",
                                onclick: {
                                    let id = sess.id;
                                    let state = state.clone();
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
                        class: "new-tab-button tooltip-trigger",
                        "data-tooltip": "{t.new_connection}",
                        aria_label: "{t.new_connection}",
                        onclick: move |_| {
                            dialog_signals.open_new();
                        },
                        Icon { name: "add" }
                    }
                }

                div {
                    class: match split_mode() {
                        Some(SplitMode::Horizontal) => "terminal-body is-split-horizontal",
                        Some(SplitMode::Vertical) => "terminal-body is-split-vertical",
                        None => "terminal-body",
                    },

                    if let Some(sess) = active_terminal.clone() {
                        if let Some(snapshot) = sess.snapshot.clone() {
                            div {
                                class: "terminal-pane",
                                Terminal {
                                    snapshot: SnapshotWrapper(snapshot.clone()),
                                    session_id: sess.id,
                                    pane_id: "primary".to_string(),
                                    trigger_highlights: settings().trigger_highlights,
                                    show_line_numbers: settings().show_line_numbers,
                                    show_timestamps: settings().show_timestamps,
                                    language,
                                    split_mode,
                                    exec_id: None,
                                    allow_split: true,
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
                                        show_line_numbers: settings().show_line_numbers,
                                        show_timestamps: settings().show_timestamps,
                                        language,
                                        split_mode,
                                        exec_id: None,
                                        allow_split: true,
                                    }
                                }
                            }
                        } else {
                            TerminalPlaceholder {
                                status: sess.status,
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
        }
    }
}
