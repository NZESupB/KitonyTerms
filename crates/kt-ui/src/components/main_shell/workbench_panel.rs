//! 主工作台中央区域渲染。

use std::sync::{Arc, Mutex};

use dioxus::prelude::*;
use kt_config::{AppLanguage, AppSettings};
use kt_core::{SessionId, ToCore};

use super::{ConnectionDialogSignals, SplitMode};
use crate::components::app_logic::{
    session_dot_class_for_status, ActiveMonitorView, ActiveTerminalView, SessionTabView,
};
use crate::components::icons::Icon;
use crate::components::monitor::MonitorPanel;
use crate::components::terminal::{SnapshotWrapper, Terminal};
use crate::components::workbench::{EmptyWorkbench, MonitorPlaceholder, TerminalPlaceholder};
use crate::i18n::texts;
use crate::state::AppState;

pub(super) struct WorkbenchPanelArgs {
    pub(super) state: Arc<Mutex<AppState>>,
    pub(super) settings: Signal<AppSettings>,
    pub(super) language: AppLanguage,
    pub(super) active_terminal: Option<ActiveTerminalView>,
    pub(super) active_monitor: Option<ActiveMonitorView>,
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
        active_monitor,
        session_tabs,
        dialog_signals,
        mut active_session_id,
        mut split_mode,
    } = args;

    let t = texts(language).app;

    rsx! {
        div {
            class: "main-column",

            section {
                class: "terminal-panel",

                div {
                    class: "session-tabs",

                    for sess in session_tabs {
                        div {
                            key: "tab-{sess.id.0}",
                            class: if active_session_id() == Some(sess.id) { "session-tab is-active" } else { "session-tab" },
                            onclick: {
                                let id = sess.id;
                                move |_| active_session_id.set(Some(id))
                            },

                            span { class: session_dot_class_for_status(sess.status) }
                            span { class: "tab-title", "{sess.title}" }
                            button {
                                class: "tab-close",
                                title: "{t.close_session}",
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
                        class: "new-tab-button",
                        title: "{t.new_connection}",
                        onclick: move |_| {
                            dialog_signals.open_new();
                        },
                        Icon { name: "add" }
                    }
                }

                div {
                    class: "terminal-toolbar",

                    div {
                        class: "breadcrumb",
                        span { class: "protocol-badge", "ssh" }
                        span { class: "chevron", "›" }
                        if let Some(sess) = active_terminal.clone() {
                            span {
                                class: "host-pill",
                                span { class: session_dot_class_for_status(sess.status) }
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
                                    language,
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
                                        language,
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

            div {
                class: "monitor-dock",
                if let Some(monitor) = active_monitor {
                    MonitorPanel {
                        key: "monitor-{monitor.session_id.0}",
                        session_id: monitor.session_id,
                        language,
                    }
                } else {
                    MonitorPlaceholder { language }
                }
            }
        }
    }
}
