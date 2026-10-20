//! 工作区级展示组件。

use dioxus::prelude::*;
use kt_config::AppLanguage;
use kt_core::SessionId;

use crate::components::app_logic::SessionConnectionStatus;
use crate::components::icons::{AppLogo, Icon};
use crate::components::terminal::{focus_terminal, is_reconnect_enter};
use crate::i18n::texts;

#[component]
pub fn TerminalPlaceholder(
    session_id: SessionId,
    status: SessionConnectionStatus,
    title: String,
    error: Option<String>,
    language: AppLanguage,
    #[props(default)] desktop_focus: bool,
) -> Element {
    let t = texts(language).app;
    let state = crate::components::app::get_state().clone();
    let terminal_id = format!("terminal-placeholder-{}", session_id.0);
    let terminal_id_for_click = terminal_id.clone();
    use_effect(use_reactive(
        (&session_id, &status, &desktop_focus),
        move |(id, status, desktop_focus)| {
            if desktop_focus && status == SessionConnectionStatus::Disconnected {
                focus_terminal(&format!("terminal-placeholder-{}", id.0), true);
            }
        },
    ));
    let state_line = if let Some(error) = error.as_deref() {
        error
    } else {
        match status {
            SessionConnectionStatus::Connected => t.terminal_waiting,
            SessionConnectionStatus::Authenticating => t.authenticating,
            SessionConnectionStatus::HostKeyPending => t.host_key_pending,
            SessionConnectionStatus::Disconnected | SessionConnectionStatus::Connecting => {
                t.terminal_connecting
            }
        }
    };

    rsx! {
        div {
            class: "terminal-placeholder",
            id: "{terminal_id}",
            tabindex: desktop_focus.then_some(0),
            onclick: move |_| {
                if desktop_focus {
                    focus_terminal(&terminal_id_for_click, false);
                }
            },
            onkeydown: move |evt| {
                if !is_reconnect_enter(&evt.data()) {
                    return;
                }
                evt.prevent_default();
                evt.stop_propagation();
                if let Ok(mut app_state) = state.lock() {
                    if let Err(error) = app_state.reconnect_disconnected_session(session_id) {
                        tracing::error!("会话重连失败: {error}");
                    }
                }
            },
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
            if desktop_focus && status == SessionConnectionStatus::Disconnected {
                p { class: "terminal-reconnect-hint", "{t.reconnect_enter_hint}" }
            }
            span { class: "terminal-caret" }
        }
    }
}

#[component]
pub fn EmptyWorkbench(language: AppLanguage) -> Element {
    let t = texts(language).app;

    rsx! {
        div {
            class: "empty-workbench",
            div { class: "empty-logo", AppLogo { size: "empty" } }
            h2 { "{t.empty_title}" }
        }
    }
}

#[component]
pub fn MonitorPlaceholder(language: AppLanguage) -> Element {
    let t = texts(language).app;
    let labels = ["CPU", t.memory, t.disk, t.load, t.network];

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
