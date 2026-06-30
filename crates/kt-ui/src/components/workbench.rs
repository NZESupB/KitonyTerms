//! 工作区级展示组件。

use dioxus::prelude::*;
use kt_config::{
    normalize_theme_name, AppLanguage, SshProxy, DEFAULT_DARK_THEME, DEFAULT_LIGHT_THEME,
};

use crate::components::dialog::{proxy_from_inputs, proxy_host, proxy_kind, proxy_port};
use crate::components::icons::{AppLogo, Icon};
use crate::i18n::texts;

#[component]
pub fn SettingsPanel(
    show: Signal<bool>,
    language: AppLanguage,
    theme: String,
    default_text_editor: Option<String>,
    default_ssh_proxy: SshProxy,
    terminal_show_timestamps: bool,
    terminal_show_line_numbers: bool,
    on_language_change: EventHandler<AppLanguage>,
    on_theme_change: EventHandler<String>,
    on_default_text_editor_change: EventHandler<Option<String>>,
    on_default_ssh_proxy_change: EventHandler<SshProxy>,
    on_terminal_timestamps_change: EventHandler<bool>,
    on_terminal_line_numbers_change: EventHandler<bool>,
) -> Element {
    if !show() {
        return rsx! {};
    }

    let t = texts(language).app;
    let selected_theme = normalize_theme_name(&theme);
    let proxy_kind_value = proxy_kind(&default_ssh_proxy).to_string();
    let proxy_host_value = proxy_host(&default_ssh_proxy);
    let proxy_port_value = proxy_port(&default_ssh_proxy);

    rsx! {
        div {
            class: "settings-overlay",
            onclick: move |_| show.set(false),

            section {
                class: "settings-panel",
                onclick: move |evt| evt.stop_propagation(),

                div {
                    class: "settings-head",
                    h2 { "{t.settings}" }
                    button {
                        class: "icon-button slim",
                        title: "{t.close}",
                        onclick: move |_| show.set(false),
                        Icon { name: "close" }
                    }
                }

                div {
                    class: "settings-row",
                    div {
                        strong { "{t.language}" }
                        p { "{t.language_hint}" }
                    }

                    div {
                        class: "segmented-control",
                        button {
                            class: if language == AppLanguage::Chinese { "is-selected" } else { "" },
                            onclick: move |_| on_language_change.call(AppLanguage::Chinese),
                            "{t.chinese}"
                        }
                        button {
                            class: if language == AppLanguage::English { "is-selected" } else { "" },
                            onclick: move |_| on_language_change.call(AppLanguage::English),
                            "{t.english}"
                        }
                    }
                }

                div {
                    class: "settings-row",
                    div {
                        strong { "{t.theme}" }
                        p { "{t.theme_hint}" }
                    }

                    div {
                        class: "segmented-control",
                        button {
                            class: if selected_theme == DEFAULT_DARK_THEME { "is-selected" } else { "" },
                            onclick: move |_| on_theme_change.call(DEFAULT_DARK_THEME.to_string()),
                            "{t.theme_dark}"
                        }
                        button {
                            class: if selected_theme == DEFAULT_LIGHT_THEME { "is-selected" } else { "" },
                            onclick: move |_| on_theme_change.call(DEFAULT_LIGHT_THEME.to_string()),
                            "{t.theme_light}"
                        }
                    }
                }

                div {
                    class: "settings-row settings-row-stacked",
                    div {
                        strong { "{t.default_text_editor}" }
                        p { "{t.default_text_editor_hint}" }
                    }

                    input {
                        class: "settings-input",
                        r#type: "text",
                        value: "{default_text_editor.clone().unwrap_or_default()}",
                        placeholder: "{t.default_text_editor_placeholder}",
                        oninput: move |evt| {
                            let value = evt.value();
                            let editor = if value.trim().is_empty() {
                                None
                            } else {
                                Some(value.trim().to_string())
                            };
                            on_default_text_editor_change.call(editor);
                        },
                    }
                }

                div {
                    class: "settings-row",
                    div {
                        strong { "{t.terminal_display}" }
                        p { "{t.show_timestamps} / {t.show_line_numbers}" }
                    }

                    div {
                        class: "settings-checks",
                        label {
                            input {
                                r#type: "checkbox",
                                checked: terminal_show_timestamps,
                                onchange: move |evt| on_terminal_timestamps_change.call(evt.checked()),
                            }
                            "{t.show_timestamps}"
                        }
                        label {
                            input {
                                r#type: "checkbox",
                                checked: terminal_show_line_numbers,
                                onchange: move |evt| on_terminal_line_numbers_change.call(evt.checked()),
                            }
                            "{t.show_line_numbers}"
                        }
                    }
                }

                div {
                    class: "settings-row settings-row-stacked",
                    div {
                        strong { "{t.default_ssh_proxy}" }
                        p { "{t.default_ssh_proxy_hint}" }
                    }

                    div {
                        class: "settings-proxy-editor",
                        div {
                            class: "segmented-control",
                            for (kind, label) in [
                                ("direct", t.proxy_direct),
                                ("system", t.proxy_system),
                                ("socks", t.proxy_socks),
                                ("http", t.proxy_http),
                            ] {
                                button {
                                    class: if proxy_kind_value == kind { "is-selected" } else { "" },
                                    onclick: {
                                        let host = proxy_host_value.clone();
                                        let port = proxy_port_value.clone();
                                        move |_| {
                                            on_default_ssh_proxy_change.call(proxy_from_inputs(kind, &host, &port));
                                        }
                                    },
                                    "{label}"
                                }
                            }
                        }

                        if matches!(proxy_kind_value.as_str(), "socks" | "http") {
                            div {
                                class: "settings-proxy-fields",
                                label {
                                    span { "{t.proxy_host}" }
                                    input {
                                        class: "settings-input",
                                        r#type: "text",
                                        value: "{proxy_host_value}",
                                        oninput: {
                                            let kind = proxy_kind_value.clone();
                                            let port = proxy_port_value.clone();
                                            move |evt| {
                                                on_default_ssh_proxy_change.call(proxy_from_inputs(&kind, &evt.value(), &port));
                                            }
                                        },
                                    }
                                }
                                label {
                                    span { "{t.proxy_port}" }
                                    input {
                                        class: "settings-input",
                                        r#type: "text",
                                        value: "{proxy_port_value}",
                                        oninput: {
                                            let kind = proxy_kind_value.clone();
                                            let host = proxy_host_value.clone();
                                            move |evt| {
                                                on_default_ssh_proxy_change.call(proxy_from_inputs(&kind, &host, &evt.value()));
                                            }
                                        },
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn TerminalPlaceholder(
    connected: bool,
    title: String,
    error: Option<String>,
    language: AppLanguage,
) -> Element {
    let t = texts(language).app;
    let state_line = if let Some(error) = error.as_deref() {
        error
    } else if connected {
        t.terminal_waiting
    } else {
        t.terminal_connecting
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
    let labels = ["CPU", t.memory, t.load, t.network];

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
