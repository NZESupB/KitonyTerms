//! 工作区级展示组件。

use dioxus::prelude::*;
use kt_config::AppLanguage;

use crate::components::app_logic::SessionConnectionStatus;
use crate::components::icons::{AppLogo, Icon};
use crate::i18n::texts;

#[component]
pub fn TerminalPlaceholder(
    status: SessionConnectionStatus,
    title: String,
    error: Option<String>,
    language: AppLanguage,
) -> Element {
    let t = texts(language).app;
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
pub fn EmptyWorkbench(language: AppLanguage) -> Element {
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
