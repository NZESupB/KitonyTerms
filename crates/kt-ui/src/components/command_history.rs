//! 当前 SSH 会话的内存命令历史，只提供复制，不向终端执行命令。

use dioxus::prelude::*;
use kt_config::AppLanguage;
use kt_core::SessionId;

use crate::components::icons::Icon;
use crate::i18n::texts;
use crate::state::CommandEntry;

#[component]
pub fn CommandHistoryPanel(
    session_id: SessionId,
    entries: Vec<CommandEntry>,
    language: AppLanguage,
    on_clear: EventHandler<()>,
    on_close: EventHandler<()>,
) -> Element {
    let t = texts(language).app;
    let mut copy_status = use_signal(|| None::<bool>);
    use_effect(use_reactive((&session_id,), move |_| copy_status.set(None)));
    let count = entries.len();

    rsx! {
        section {
            id: "command-history-panel",
            class: "command-history-panel",
            aria_label: "{t.command_history}",
            div { class: "command-history-head",
                strong { "{t.command_history}" }
                span { class: "command-history-count", "{count} / 200" }
                button {
                    class: "dialog-btn",
                    disabled: count == 0,
                    onclick: move |_| { copy_status.set(None); on_clear.call(()); },
                    "{t.history_clear}"
                }
                button {
                    class: "icon-button slim",
                    aria_label: "{t.close}",
                    title: "{t.close}",
                    onclick: move |_| on_close.call(()),
                    Icon { name: "close" }
                }
            }
            p { class: "command-history-hint", "{t.history_hint}" }
            div { class: "command-history-list",
                if count == 0 {
                    p { class: "command-history-empty", "{t.history_empty}" }
                }
                for entry in entries.into_iter().rev() {
                    div { class: "command-history-row", key: "{session_id.0}-{entry.sequence}",
                        time { "{entry.time}" }
                        code { "{entry.command}" }
                        button {
                            class: "icon-button slim",
                            aria_label: "{t.copy}",
                            title: "{t.copy}",
                            onclick: move |_| {
                                copy_status.set(Some(crate::clipboard::write_text(&entry.command).is_ok()));
                            },
                            Icon { name: "file" }
                        }
                    }
                }
            }
            if let Some(success) = copy_status() {
                p { class: "command-history-feedback", role: "status",
                    if success { "{t.history_copied}" } else { "{t.history_copy_failed}" }
                }
            }
        }
    }
}
