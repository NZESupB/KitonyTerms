//! 工作区级展示组件。

use dioxus::prelude::*;
use kt_config::{
    normalize_theme_name, AppLanguage, AppSettings, EditorEntry, DEFAULT_DARK_THEME,
    DEFAULT_LIGHT_THEME,
};

use crate::components::app_logic::SessionConnectionStatus;
use crate::components::external_edit::{detect_editors, env_editor_command};
use crate::components::icons::{AppLogo, Icon};
use crate::i18n::texts;

#[component]
pub fn SettingsPanel(
    show: Signal<bool>,
    language: AppLanguage,
    settings: AppSettings,
    on_language_change: EventHandler<AppLanguage>,
    on_theme_change: EventHandler<String>,
    on_settings_change: EventHandler<AppSettings>,
) -> Element {
    // hooks 必须在 early return 之前初始化，避免随 show 抖动改变 hooks 顺序。
    let detected_editors = use_signal(detect_editors);
    let env_editor = use_signal(env_editor_command);
    let mut new_editor_command = use_signal(String::new);

    if !show() {
        return rsx! {};
    }

    let t = texts(language).app;
    let theme = settings.theme.clone();
    let selected_theme = normalize_theme_name(&theme);

    let default_editor_value = settings.default_editor.clone().unwrap_or_default();
    let detected_list = detected_editors();
    let env_editor_cmd = env_editor();
    // 既有默认编辑器命令未匹配任何选项时，额外保留为「自定义」项，避免静默丢失。
    let default_is_custom = !default_editor_value.is_empty()
        && env_editor_cmd.as_deref() != Some(default_editor_value.as_str())
        && !detected_list
            .iter()
            .any(|e| e.command == default_editor_value);
    // 添加下拉可选：尚未加入自定义列表的探测编辑器。
    let available: Vec<EditorEntry> = detected_list
        .iter()
        .filter(|e| !settings.editors.iter().any(|se| se.command == e.command))
        .cloned()
        .collect();

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

                // 终端显示：行号 / 时间戳
                div {
                    class: "settings-row",
                    div {
                        strong { "{t.terminal_display}" }
                        p { "{t.terminal_display_hint}" }
                    }
                    div {
                        class: "settings-toggle-group",
                        label {
                            class: "settings-toggle",
                            input {
                                r#type: "checkbox",
                                checked: settings.show_line_numbers,
                                onchange: {
                                    let settings = settings.clone();
                                    move |evt: Event<FormData>| {
                                        let mut next = settings.clone();
                                        next.show_line_numbers = evt.checked();
                                        on_settings_change.call(next);
                                    }
                                }
                            }
                            span { "{t.show_line_numbers}" }
                        }
                        label {
                            class: "settings-toggle",
                            input {
                                r#type: "checkbox",
                                checked: settings.show_timestamps,
                                onchange: {
                                    let settings = settings.clone();
                                    move |evt: Event<FormData>| {
                                        let mut next = settings.clone();
                                        next.show_timestamps = evt.checked();
                                        on_settings_change.call(next);
                                    }
                                }
                            }
                            span { "{t.show_timestamps}" }
                        }
                    }
                }

                // 默认编辑器
                div {
                    class: "settings-row settings-row-stacked",
                    div {
                        strong { "{t.default_editor}" }
                        p { "{t.default_editor_hint}" }
                    }
                    select {
                        class: "settings-text-input",
                        value: "{default_editor_value}",
                        onchange: {
                            let settings = settings.clone();
                            move |evt: Event<FormData>| {
                                let mut next = settings.clone();
                                let trimmed = evt.value().trim().to_string();
                                next.default_editor = if trimmed.is_empty() { None } else { Some(trimmed) };
                                on_settings_change.call(next);
                            }
                        },
                        option { value: "", selected: default_editor_value.is_empty(), "{t.editor_system_default}" }
                        if let Some(cmd) = env_editor_cmd.as_deref() {
                            option { value: "{cmd}", selected: default_editor_value == cmd, "{t.editor_env_var}" }
                        }
                        for editor in detected_list.iter() {
                            option {
                                value: "{editor.command}",
                                selected: default_editor_value == editor.command,
                                "{editor.name}"
                            }
                        }
                        if default_is_custom {
                            option {
                                value: "{default_editor_value}",
                                selected: true,
                                "{t.editor_custom}: {default_editor_value}"
                            }
                        }
                    }
                }

                // 自定义编辑器列表（打开方式）
                div {
                    class: "settings-row settings-row-stacked",
                    div {
                        strong { "{t.editors_title}" }
                        p { "{t.editors_hint}" }
                    }
                    div {
                        class: "settings-editor-list",
                        for (index, editor) in settings.editors.iter().enumerate() {
                            div {
                                key: "editor-{index}",
                                class: "settings-editor-item",
                                span { class: "settings-editor-name", "{editor.name}" }
                                code { class: "settings-editor-command", "{editor.command}" }
                                button {
                                    class: "icon-button slim danger",
                                    title: "{t.remove}",
                                    onclick: {
                                        let settings = settings.clone();
                                        move |_| {
                                            let mut next = settings.clone();
                                            if index < next.editors.len() {
                                                next.editors.remove(index);
                                                on_settings_change.call(next);
                                            }
                                        }
                                    },
                                    Icon { name: "trash" }
                                }
                            }
                        }
                        div {
                            class: "settings-editor-add",
                            if available.is_empty() {
                                span { class: "settings-editor-empty", "{t.editor_none_detected}" }
                            } else {
                                select {
                                    class: "settings-text-input",
                                    value: "{new_editor_command()}",
                                    onchange: move |evt| new_editor_command.set(evt.value()),
                                    option { value: "", selected: new_editor_command().is_empty(), "{t.editor_select_prompt}" }
                                    for editor in available.iter() {
                                        option {
                                            value: "{editor.command}",
                                            selected: new_editor_command() == editor.command,
                                            "{editor.name}"
                                        }
                                    }
                                }
                                button {
                                    class: "primary",
                                    onclick: {
                                        let settings = settings.clone();
                                        let available = available.clone();
                                        move |_| {
                                            let command = new_editor_command().trim().to_string();
                                            if command.is_empty() {
                                                return;
                                            }
                                            let name = available
                                                .iter()
                                                .find(|e| e.command == command)
                                                .map(|e| e.name.clone())
                                                .unwrap_or_else(|| command.clone());
                                            let mut next = settings.clone();
                                            if !next.editors.iter().any(|e| e.command == command) {
                                                next.editors.push(EditorEntry { name, command });
                                                on_settings_change.call(next);
                                            }
                                            new_editor_command.set(String::new());
                                        }
                                    },
                                    "{t.add_editor}"
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
