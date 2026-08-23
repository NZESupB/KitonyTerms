//! 设置面板：左侧分类导航 + 右侧内容区。
//!
//! 早先所有设置项挤在一个纵向长列表里，找一项要滚很久。这里按「通用 / 终端 /
//! 编辑器 / 同步」分类，手机上退化为「分类列表 → 二级页面」的两级导航，因为手机
//! 宽度放不下并排的侧栏。

use dioxus::prelude::*;
use kt_config::{
    normalize_theme_name, AppLanguage, AppSettings, EditorEntry, DEFAULT_DARK_THEME,
    DEFAULT_LIGHT_THEME,
};

use crate::components::external_edit::{detect_editors, env_editor_command};
use crate::components::icons::Icon;
use crate::components::qr::QrCodeView;
use crate::i18n::{texts, AppText};

#[derive(Clone, PartialEq, Eq)]
pub enum SyncAction {
    WebDavUpload {
        url: String,
        username: String,
        password: String,
    },
    WebDavDownload {
        url: String,
        username: String,
        password: String,
    },
    StartLanShare,
    StopLanShare,
    ImportLanShare {
        url: String,
        pairing_code: String,
    },
    /// 请求调用摄像头扫描配对二维码。
    ScanPairingCode,
    CopyText(String),
}

/// 正在进行的局域网分享，用于展示二维码与配对码。
#[derive(Clone, PartialEq, Eq)]
pub struct ActiveShare {
    pub url: String,
    pub pairing_code: String,
    /// 二维码负载（地址 + 配对码），由 kt-sync 统一编码。
    pub payload: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    General,
    Terminal,
    Editor,
    Sync,
}

impl SettingsSection {
    fn label(self, t: &AppText) -> &'static str {
        match self {
            Self::General => t.settings_nav_general,
            Self::Terminal => t.settings_nav_terminal,
            Self::Editor => t.settings_nav_editor,
            Self::Sync => t.settings_nav_sync,
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::General => "settings",
            Self::Terminal => "terminal",
            Self::Editor => "edit",
            Self::Sync => "sync",
        }
    }
}

/// 可见的分类。手机端没有右键菜单，「打开方式」自定义编辑器列表在移动端不可达，
/// 因此整个编辑器分类只在桌面显示。
fn visible_sections(is_phone: bool) -> Vec<SettingsSection> {
    if is_phone {
        vec![
            SettingsSection::General,
            SettingsSection::Terminal,
            SettingsSection::Sync,
        ]
    } else {
        vec![
            SettingsSection::General,
            SettingsSection::Terminal,
            SettingsSection::Editor,
            SettingsSection::Sync,
        ]
    }
}

/// 把配对码按 4 位分组显示，手抄和核对都更省力。
pub fn group_pairing_code(code: &str) -> String {
    code.chars()
        .collect::<Vec<_>>()
        .chunks(4)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

#[component]
pub fn SettingsPanel(
    show: Signal<bool>,
    language: AppLanguage,
    settings: AppSettings,
    is_phone: bool,
    on_language_change: EventHandler<AppLanguage>,
    on_theme_change: EventHandler<String>,
    on_settings_change: EventHandler<AppSettings>,
    sync_busy: bool,
    sync_status: Option<String>,
    active_share: Option<ActiveShare>,
    scan_supported: bool,
    /// 扫码结果，取用后清空。
    scan_result: Signal<Option<(String, String)>>,
    on_sync_action: EventHandler<SyncAction>,
) -> Element {
    // hooks 必须在 early return 之前初始化，避免随 show 抖动改变 hooks 顺序。
    let detected_editors = use_signal(detect_editors);
    let env_editor = use_signal(env_editor_command);
    let new_editor_command = use_signal(String::new);
    let sync_url = use_signal(String::new);
    let sync_username = use_signal(String::new);
    let sync_password = use_signal(String::new);
    let share_url = use_signal(String::new);
    let share_code = use_signal(String::new);
    let mut section = use_signal(|| SettingsSection::General);
    // 手机端是两级导航：None 表示停留在分类列表页。
    let mut phone_section = use_signal(|| None::<SettingsSection>);

    // 扫码成功后把地址与配对码填进输入框，并跳到同步分类让用户确认后导入。
    let mut share_url_sink = share_url;
    let mut share_code_sink = share_code;
    let mut scan_result_sink = scan_result;
    use_effect(move || {
        if let Some((url, code)) = scan_result_sink.take() {
            share_url_sink.set(url);
            share_code_sink.set(code);
            section.set(SettingsSection::Sync);
            phone_section.set(Some(SettingsSection::Sync));
        }
    });

    if !show() {
        return rsx! {};
    }

    let t = texts(language).app;
    let sections = visible_sections(is_phone);
    // 分类可见性随设备变化，选中项失效时回落到通用。
    let active_section = if sections.contains(&section()) {
        section()
    } else {
        SettingsSection::General
    };
    let phone_active = phone_section().filter(|current| sections.contains(current));

    let overlay_class = if is_phone {
        "settings-overlay app-settings-overlay is-mobile"
    } else {
        "settings-overlay app-settings-overlay is-desktop"
    };

    let body = SettingsBodyProps {
        // 实际分类由下面两处渲染分别覆盖。
        section: active_section,
        language,
        settings: settings.clone(),
        is_phone,
        on_language_change,
        on_theme_change,
        on_settings_change,
        sync_busy,
        sync_status: sync_status.clone(),
        active_share: active_share.clone(),
        scan_supported,
        on_sync_action,
        detected_editors,
        env_editor,
        new_editor_command,
        sync_url,
        sync_username,
        sync_password,
        share_url,
        share_code,
    };

    rsx! {
        div {
            class: overlay_class,
            onclick: move |_| show.set(false),

            section {
                class: "settings-panel app-settings-panel",
                onclick: move |evt| evt.stop_propagation(),

                div {
                    class: "settings-head",
                    // 手机端二级页面左上角是返回，回到分类列表。
                    if is_phone && phone_active.is_some() {
                        button {
                            class: "icon-button slim",
                            title: "{t.settings_back}",
                            onclick: move |_| phone_section.set(None),
                            Icon { name: "chevron-left" }
                        }
                    }
                    h2 {
                        match (is_phone, phone_active) {
                            (true, Some(current)) => current.label(&t),
                            _ => t.settings,
                        }
                    }
                    button {
                        class: "icon-button slim",
                        title: "{t.close}",
                        onclick: move |_| show.set(false),
                        Icon { name: "close" }
                    }
                }

                if is_phone {
                    match phone_active {
                        None => rsx! {
                            nav {
                                class: "settings-nav-list",
                                for entry in sections.iter().copied() {
                                    button {
                                        key: "{entry.icon()}",
                                        class: "settings-nav-list-item",
                                        onclick: move |_| phone_section.set(Some(entry)),
                                        Icon { name: entry.icon() }
                                        span { "{entry.label(&t)}" }
                                        Icon { name: "chevron-right" }
                                    }
                                }
                            }
                        },
                        Some(current) => rsx! {
                            div {
                                class: "settings-body",
                                SettingsBody { section: current, ..body }
                            }
                        },
                    }
                } else {
                    div {
                        class: "settings-layout",
                        nav {
                            class: "settings-nav",
                            for entry in sections.iter().copied() {
                                button {
                                    key: "{entry.icon()}",
                                    class: if entry == active_section {
                                        "settings-nav-item is-active"
                                    } else {
                                        "settings-nav-item"
                                    },
                                    onclick: move |_| section.set(entry),
                                    Icon { name: entry.icon() }
                                    span { "{entry.label(&t)}" }
                                }
                            }
                        }
                        div {
                            class: "settings-body",
                            SettingsBody { section: active_section, ..body }
                        }
                    }
                }
            }
        }
    }
}

/// 内容区。分类之间共享一批 hook 状态（输入框内容），所以由父组件创建后传入。
#[derive(Props, Clone, PartialEq)]
struct SettingsBodyProps {
    #[props(default = SettingsSection::General)]
    section: SettingsSection,
    language: AppLanguage,
    settings: AppSettings,
    is_phone: bool,
    on_language_change: EventHandler<AppLanguage>,
    on_theme_change: EventHandler<String>,
    on_settings_change: EventHandler<AppSettings>,
    sync_busy: bool,
    sync_status: Option<String>,
    active_share: Option<ActiveShare>,
    scan_supported: bool,
    on_sync_action: EventHandler<SyncAction>,
    detected_editors: Signal<Vec<EditorEntry>>,
    env_editor: Signal<Option<String>>,
    new_editor_command: Signal<String>,
    sync_url: Signal<String>,
    sync_username: Signal<String>,
    sync_password: Signal<String>,
    share_url: Signal<String>,
    share_code: Signal<String>,
}

#[component]
fn SettingsBody(props: SettingsBodyProps) -> Element {
    match props.section {
        SettingsSection::General => rsx! { GeneralSection { ..props } },
        SettingsSection::Terminal => rsx! { TerminalSection { ..props } },
        SettingsSection::Editor => rsx! { EditorSection { ..props } },
        SettingsSection::Sync => rsx! { SyncSection { ..props } },
    }
}

#[component]
fn GeneralSection(props: SettingsBodyProps) -> Element {
    let t = texts(props.language).app;
    let language = props.language;
    let on_language_change = props.on_language_change;
    let on_theme_change = props.on_theme_change;
    let selected_theme = normalize_theme_name(&props.settings.theme);

    rsx! {
        div {
            class: "settings-row",
            div {
                class: "settings-row-copy",
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
                class: "settings-row-copy",
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
    }
}

#[component]
fn TerminalSection(props: SettingsBodyProps) -> Element {
    let t = texts(props.language).app;
    let settings = props.settings.clone();
    let on_settings_change = props.on_settings_change;
    let line_numbers_settings = settings.clone();
    let timestamps_settings = settings.clone();

    rsx! {
        div {
            class: "settings-row",
            div {
                class: "settings-row-copy",
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
                        onchange: move |evt: Event<FormData>| {
                            let mut next = line_numbers_settings.clone();
                            next.show_line_numbers = evt.checked();
                            on_settings_change.call(next);
                        },
                    }
                    span { "{t.show_line_numbers}" }
                }
                label {
                    class: "settings-toggle",
                    input {
                        r#type: "checkbox",
                        checked: settings.show_timestamps,
                        onchange: move |evt: Event<FormData>| {
                            let mut next = timestamps_settings.clone();
                            next.show_timestamps = evt.checked();
                            on_settings_change.call(next);
                        },
                    }
                    span { "{t.show_timestamps}" }
                }
            }
        }
    }
}

#[component]
fn EditorSection(props: SettingsBodyProps) -> Element {
    let t = texts(props.language).app;
    let settings = props.settings.clone();
    let on_settings_change = props.on_settings_change;
    let mut new_editor_command = props.new_editor_command;

    let default_editor_value = settings.default_editor.clone().unwrap_or_default();
    let detected_list = (props.detected_editors)();
    let env_editor_cmd = (props.env_editor)();
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

    let default_settings = settings.clone();

    rsx! {
        div {
            class: "settings-row settings-row-stacked",
            div {
                class: "settings-row-copy",
                strong { "{t.default_editor}" }
                p { "{t.default_editor_hint}" }
            }
            select {
                class: "settings-text-input",
                value: "{default_editor_value}",
                onchange: move |evt: Event<FormData>| {
                    let mut next = default_settings.clone();
                    let trimmed = evt.value().trim().to_string();
                    next.default_editor = if trimmed.is_empty() { None } else { Some(trimmed) };
                    on_settings_change.call(next);
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

        div {
            class: "settings-row settings-row-stacked",
            div {
                class: "settings-row-copy",
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
                            class: "settings-button primary",
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

#[component]
fn SyncSection(props: SettingsBodyProps) -> Element {
    let t = texts(props.language).app;
    let sync_busy = props.sync_busy;
    let on_sync_action = props.on_sync_action;
    let mut sync_url = props.sync_url;
    let mut sync_username = props.sync_username;
    let mut sync_password = props.sync_password;
    let mut share_url = props.share_url;
    let mut share_code = props.share_code;
    let active_share = props.active_share.clone();

    rsx! {
        div {
            class: "settings-row settings-row-stacked",
            div {
                class: "settings-row-copy",
                strong { "{t.sync_webdav_title}" }
                p { "{t.sync_hint}" }
            }
            div {
                class: "settings-field-grid",
                input {
                    class: "settings-text-input",
                    r#type: "url",
                    placeholder: "https://dav.example.com/kitonyterms.json",
                    value: "{sync_url}",
                    oninput: move |evt| sync_url.set(evt.value()),
                }
                input {
                    class: "settings-text-input",
                    r#type: "text",
                    placeholder: "{t.sync_username}",
                    value: "{sync_username}",
                    oninput: move |evt| sync_username.set(evt.value()),
                }
                input {
                    class: "settings-text-input",
                    r#type: "password",
                    placeholder: "{t.sync_password}",
                    value: "{sync_password}",
                    oninput: move |evt| sync_password.set(evt.value()),
                }
            }
            div {
                class: "settings-actions",
                button {
                    class: "settings-button primary",
                    disabled: sync_busy,
                    onclick: move |_| {
                        on_sync_action.call(SyncAction::WebDavUpload {
                            url: sync_url(),
                            username: sync_username(),
                            password: sync_password(),
                        });
                        sync_password.set(String::new());
                    },
                    "{t.sync_upload}"
                }
                button {
                    class: "settings-button",
                    disabled: sync_busy,
                    onclick: move |_| {
                        on_sync_action.call(SyncAction::WebDavDownload {
                            url: sync_url(),
                            username: sync_username(),
                            password: sync_password(),
                        });
                        sync_password.set(String::new());
                    },
                    "{t.sync_download}"
                }
            }
        }

        div {
            class: "settings-row settings-row-stacked",
            div {
                class: "settings-row-copy",
                strong { "{t.sync_lan_title}" }
                p { "{t.sync_lan_hint}" }
            }

            // 分享端：二维码 + 高熵配对秘密，二维码优先，手输按四位分组。
            if let Some(share) = active_share {
                div {
                    class: "settings-share-card",
                    QrCodeView { data: share.payload.clone(), label: t.sync_qr_label.to_string() }
                    p { class: "settings-share-hint", "{t.sync_scan_hint}" }
                    div {
                        class: "settings-share-field",
                        span { class: "settings-share-label", "{t.sync_lan_url}" }
                        code { class: "settings-share-value", "{share.url}" }
                        button {
                            class: "settings-button slim",
                            onclick: {
                                let url = share.url.clone();
                                move |_| on_sync_action.call(SyncAction::CopyText(url.clone()))
                            },
                            "{t.sync_copy}"
                        }
                    }
                    div {
                        class: "settings-share-field",
                        span { class: "settings-share-label", "{t.sync_pairing_code}" }
                        code {
                            class: "settings-share-value is-code",
                            "{group_pairing_code(&share.pairing_code)}"
                        }
                        button {
                            class: "settings-button slim",
                            onclick: {
                                let code = share.pairing_code.clone();
                                move |_| on_sync_action.call(SyncAction::CopyText(code.clone()))
                            },
                            "{t.sync_copy}"
                        }
                    }
                    div {
                        class: "settings-actions",
                        button {
                            class: "settings-button danger",
                            onclick: move |_| on_sync_action.call(SyncAction::StopLanShare),
                            "{t.sync_stop_share}"
                        }
                    }
                }
            } else {
                div {
                    class: "settings-actions",
                    button {
                        class: "settings-button primary",
                        disabled: sync_busy,
                        onclick: move |_| on_sync_action.call(SyncAction::StartLanShare),
                        "{t.sync_share}"
                    }
                }
            }

            // 接收端：扫码或手动输入。
            div {
                class: "settings-field-grid",
                input {
                    class: "settings-text-input",
                    r#type: "url",
                    placeholder: "http://192.168.1.20:12345/v2/config",
                    value: "{share_url}",
                    oninput: move |evt| share_url.set(evt.value()),
                }
                input {
                    class: "settings-text-input is-code",
                    r#type: "text",
                    placeholder: "{t.sync_pairing_code}",
                    autocomplete: "off",
                    autocapitalize: "characters",
                    spellcheck: false,
                    maxlength: 32,
                    value: "{share_code}",
                    oninput: move |evt| share_code.set(evt.value()),
                }
            }
            div {
                class: "settings-actions",
                if props.scan_supported {
                    button {
                        class: "settings-button",
                        disabled: sync_busy,
                        onclick: move |_| on_sync_action.call(SyncAction::ScanPairingCode),
                        Icon { name: "scan" }
                        "{t.sync_scan}"
                    }
                }
                button {
                    class: "settings-button primary",
                    disabled: sync_busy,
                    onclick: move |_| {
                        on_sync_action.call(SyncAction::ImportLanShare {
                            url: share_url(),
                            pairing_code: share_code(),
                        });
                        share_code.set(String::new());
                    },
                    "{t.sync_import}"
                }
            }

            if let Some(status) = props.sync_status.as_deref() {
                p { class: "settings-sync-status", "{status}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_code_is_grouped_in_blocks_of_four() {
        assert_eq!(
            group_pairing_code("0123456789ABCDEFGHJKMNPQRS"),
            "0123 4567 89AB CDEF GHJK MNPQ RS"
        );
        // 不足一组时不补空格。
        assert_eq!(group_pairing_code("AB"), "AB");
        assert_eq!(group_pairing_code(""), "");
    }

    #[test]
    fn settings_layout_css_does_not_leak_into_shared_dialog_panels() {
        let css = include_str!("../assets/app.css");
        let shared_panel = css
            .rsplit_once("\n.settings-panel {")
            .and_then(|(_, rest)| rest.split_once('}'))
            .map(|(body, _)| body)
            .expect("缺少通用 settings-panel 样式");
        let app_panel = css
            .rsplit_once("\n.app-settings-panel {")
            .and_then(|(_, rest)| rest.split_once('}'))
            .map(|(body, _)| body)
            .expect("缺少应用设置面板样式");

        assert!(!shared_panel.contains("display: flex"));
        assert!(!shared_panel.contains("overflow: hidden"));
        assert!(shared_panel.contains("overflow-y: auto"));
        assert!(app_panel.contains("display: flex"));
        assert!(app_panel.contains("overflow: hidden"));
        for selector in [
            ".settings-panel.group-dialog",
            ".settings-panel.external-edit-dialog",
            ".settings-panel.connection-dialog",
        ] {
            assert!(css.contains(selector), "弹窗宽度缺少专用作用域: {selector}");
        }
    }

    #[test]
    fn phones_hide_the_editor_section() {
        // 手机没有右键菜单，「打开方式」自定义编辑器无处可用。
        assert!(!visible_sections(true).contains(&SettingsSection::Editor));
        assert!(visible_sections(false).contains(&SettingsSection::Editor));
    }

    #[test]
    fn every_platform_keeps_general_terminal_and_sync() {
        for is_phone in [true, false] {
            let sections = visible_sections(is_phone);
            assert!(sections.contains(&SettingsSection::General));
            assert!(sections.contains(&SettingsSection::Terminal));
            assert!(sections.contains(&SettingsSection::Sync));
        }
    }
}
