//! 连接编辑对话框组件

use std::path::PathBuf;

use dioxus::prelude::*;
use kt_config::{
    normalize_group_name, AppLanguage, AuthMethod, ConnectParams, ProxyConfig, SessionProfile,
};

use crate::components::icons::Icon;
use crate::i18n::texts;

/// 供跳板下拉使用的已保存连接引用：展示名 + 目标地址 `[user@]host[:port]`。
#[derive(Clone, PartialEq)]
pub struct SavedConnectionRef {
    pub name: String,
    pub jump_spec: String,
}

/// 从已保存连接构造跳板下拉项，排除正在编辑的连接自身，避免自引用。
pub fn saved_connection_refs(
    profiles: &[SessionProfile],
    exclude_name: &str,
) -> Vec<SavedConnectionRef> {
    profiles
        .iter()
        .filter(|profile| profile.name != exclude_name)
        .map(|profile| {
            let p = &profile.params;
            let jump_spec = if p.port == 22 {
                format!("{}@{}", p.user, p.host)
            } else {
                format!("{}@{}:{}", p.user, p.host, p.port)
            };
            SavedConnectionRef {
                name: profile.name.clone(),
                jump_spec,
            }
        })
        .collect()
}

pub fn auth_methods_from_inputs(key_path: &str, use_agent: bool) -> Vec<AuthMethod> {
    let mut auth = Vec::new();
    let trimmed_key_path = key_path.trim();
    if !trimmed_key_path.is_empty() {
        auth.push(AuthMethod::PublicKey {
            key_path: PathBuf::from(trimmed_key_path),
        });
    }
    if use_agent {
        auth.push(AuthMethod::Agent);
    }
    auth.push(AuthMethod::Password);
    auth
}

pub fn first_public_key_path(auth: &[AuthMethod]) -> String {
    auth.iter()
        .find_map(|method| match method {
            AuthMethod::PublicKey { key_path } => Some(key_path.to_string_lossy().to_string()),
            AuthMethod::Password | AuthMethod::KeyboardInteractive | AuthMethod::Agent => None,
        })
        .unwrap_or_default()
}

/// 代理类型的稳定字符串标识（用于 UI 选择器与信号存储）。
pub fn proxy_kind_label(proxy: &ProxyConfig) -> &'static str {
    match proxy {
        ProxyConfig::Direct => "direct",
        ProxyConfig::System => "system",
        ProxyConfig::Socks5 { .. } => "socks5",
        ProxyConfig::Http { .. } => "http",
    }
}

/// 推断 UI 代理模式：TCP 代理(system/socks5/http)优先，其次跳转服务器(jump)，否则直连(direct)。
/// 用于把 `ConnectParams.proxy` 与 `proxy_jump` 两个独立字段归一为下拉框的单一选择。
pub fn proxy_mode(params: &ConnectParams) -> &'static str {
    match &params.proxy {
        ProxyConfig::Direct => {
            if params.proxy_jump.is_some() {
                "jump"
            } else {
                "direct"
            }
        }
        other => proxy_kind_label(other),
    }
}

/// 从代理配置中取出 host/port/username 文本（供编辑回填）。
pub fn proxy_fields(proxy: &ProxyConfig) -> (String, String, String) {
    match proxy {
        ProxyConfig::Socks5 {
            host,
            port,
            username,
        }
        | ProxyConfig::Http {
            host,
            port,
            username,
        } => (
            host.clone(),
            port.to_string(),
            username.clone().unwrap_or_default(),
        ),
        ProxyConfig::Direct | ProxyConfig::System => (String::new(), String::new(), String::new()),
    }
}

/// 根据 UI 输入构造 [`ProxyConfig`]。非法端口回退为 1080。
pub fn proxy_from_inputs(kind: &str, host: &str, port: &str, username: &str) -> ProxyConfig {
    let host = host.trim().to_string();
    let username = {
        let trimmed = username.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    };
    let port = port.trim().parse::<u16>().unwrap_or(1080);
    match kind {
        "system" => ProxyConfig::System,
        "socks5" if !host.is_empty() => ProxyConfig::Socks5 {
            host,
            port,
            username,
        },
        "http" if !host.is_empty() => ProxyConfig::Http {
            host,
            port,
            username,
        },
        _ => ProxyConfig::Direct,
    }
}

#[component]
#[allow(clippy::too_many_arguments)]
pub fn ConnectionDialog(
    show: Signal<bool>,
    mode: Signal<String>,
    name: Signal<String>,
    host: Signal<String>,
    port: Signal<String>,
    user: Signal<String>,
    group: Signal<String>,
    password: Signal<String>,
    key_path: Signal<String>,
    proxy_jump: Signal<String>,
    proxy_type: Signal<String>,
    proxy_host: Signal<String>,
    proxy_port: Signal<String>,
    proxy_username: Signal<String>,
    use_agent: Signal<bool>,
    forward_agent: Signal<bool>,
    groups: Vec<String>,
    /// 已保存连接名称，用于「使用已保存连接作为跳板」下拉。
    saved_connections: Vec<SavedConnectionRef>,
    language: AppLanguage,
    on_save: EventHandler<SessionProfile>,
) -> Element {
    // 选项卡状态需在 early return 之前初始化，避免 hooks 顺序随 show 抖动。
    let mut active_tab = use_signal(|| "session".to_string());

    if !show() {
        return rsx! {};
    }

    let is_edit = mode() == "edit";
    let t = texts(language).dialog;
    let title = if is_edit { t.edit_title } else { t.new_title };
    let proxy_mode_val = proxy_type();
    let proxy_needs_address = matches!(proxy_mode_val.as_str(), "socks5" | "http");
    let proxy_is_jump = proxy_mode_val == "jump";
    let on_session_tab = active_tab() == "session";

    rsx! {
        div {
            class: "settings-overlay",
            onclick: move |_| {
                show.set(false);
            },

            section {
                class: "settings-panel connection-dialog",
                onclick: move |evt| {
                    evt.stop_propagation();
                },

                div {
                    class: "settings-head",
                    h2 { "{title}" }
                    button {
                        class: "icon-button slim",
                        title: "{t.cancel}",
                        onclick: move |_| show.set(false),
                        Icon { name: "close" }
                    }
                }

                // 左侧竖向选项卡 + 右侧表单
                div {
                    class: "dialog-content",

                    div {
                        class: "dialog-tabs",
                        button {
                            class: if on_session_tab { "dialog-tab is-active" } else { "dialog-tab" },
                            onclick: move |_| active_tab.set("session".to_string()),
                            "{t.tab_session}"
                        }
                        button {
                            class: if on_session_tab { "dialog-tab" } else { "dialog-tab is-active" },
                            onclick: move |_| active_tab.set("proxy".to_string()),
                            "{t.tab_proxy}"
                        }
                    }

                    // 仅渲染活动页 body，避免 inline style display 切换造成 Dioxus 样式 diff 残留空白
                    if on_session_tab {
                        // 会话表单
                        div {
                            class: "dialog-body",

                            div {
                                class: "dialog-field",
                                span { "{t.name}" }
                                input {
                                    r#type: "text",
                                    value: "{name()}",
                                    oninput: move |evt| {
                                        name.set(evt.value().clone());
                                    },
                                    placeholder: "{t.name_placeholder}"
                                }
                            }

                            // 主机地址 + 端口 同行横排
                            div {
                                class: "dialog-field-row",
                                div {
                                    class: "dialog-field",
                                    style: "flex: 2;",
                                    span { "{t.host}" }
                                    input {
                                        r#type: "text",
                                        value: "{host()}",
                                        oninput: move |evt| {
                                            host.set(evt.value().clone());
                                        },
                                        placeholder: "{t.host_placeholder}"
                                    }
                                }
                                div {
                                    class: "dialog-field",
                                    style: "flex: 1;",
                                    span { "{t.port}" }
                                    input {
                                        r#type: "text",
                                        value: "{port()}",
                                        oninput: move |evt| {
                                            port.set(evt.value().clone());
                                        },
                                        placeholder: "22"
                                    }
                                }
                            }

                            div {
                                class: "dialog-field",
                                span { "{t.user}" }
                                input {
                                    r#type: "text",
                                    value: "{user()}",
                                    oninput: move |evt| {
                                        user.set(evt.value().clone());
                                    },
                                    placeholder: "root"
                                }
                            }

                            div {
                                class: "dialog-field",
                                span { "{t.group}" }
                                input {
                                    r#type: "text",
                                    list: "connection-groups",
                                    value: "{group()}",
                                    oninput: move |evt| {
                                        group.set(evt.value().clone());
                                    },
                                    placeholder: "{t.group_placeholder}"
                                }
                                datalist {
                                    id: "connection-groups",
                                    for group_name in groups.iter() {
                                        option { value: "{group_name}" }
                                    }
                                }
                            }

                            div {
                                class: "dialog-field",
                                span { "{t.password}" }
                                input {
                                    r#type: "password",
                                    value: "{password()}",
                                    oninput: move |evt| {
                                        password.set(evt.value().clone());
                                    },
                                    placeholder: "{t.password_placeholder}"
                                }
                                p { class: "dialog-hint", "{t.password_save_hint}" }
                            }

                            div {
                                class: "dialog-field",
                                span { "{t.private_key_path}" }
                                input {
                                    r#type: "text",
                                    value: "{key_path()}",
                                    oninput: move |evt| {
                                        key_path.set(evt.value().clone());
                                    },
                                    placeholder: "{t.private_key_path_placeholder}"
                                }
                            }

                            fieldset {
                                class: "dialog-fieldset",
                                legend { "{t.auth_options}" }
                                label {
                                    class: "dialog-check",
                                    input {
                                        r#type: "checkbox",
                                        checked: use_agent(),
                                        onchange: move |evt| {
                                            use_agent.set(evt.checked());
                                        }
                                    }
                                    "{t.use_agent}"
                                }
                                label {
                                    class: "dialog-check",
                                    input {
                                        r#type: "checkbox",
                                        checked: forward_agent(),
                                        onchange: move |evt| {
                                            forward_agent.set(evt.checked());
                                        }
                                    }
                                    "{t.forward_agent}"
                                }
                            }
                        }
                    } else {
                        // 代理表单（竖向）
                        div {
                            class: "dialog-body",

                            div {
                                class: "dialog-field",
                                span { "{t.proxy_type}" }
                                select {
                                    class: "dialog-select",
                                    value: "{proxy_mode_val}",
                                    onchange: move |evt| proxy_type.set(evt.value()),
                                    option { value: "direct", selected: proxy_mode_val == "direct", "{t.proxy_type_direct}" }
                                    option { value: "system", selected: proxy_mode_val == "system", "{t.proxy_type_system}" }
                                    option { value: "socks5", selected: proxy_mode_val == "socks5", "{t.proxy_type_socks5}" }
                                    option { value: "http", selected: proxy_mode_val == "http", "{t.proxy_type_http}" }
                                    option { value: "jump", selected: proxy_mode_val == "jump", "{t.proxy_type_jump}" }
                                }
                            }

                            // 跳转服务器：已保存连接下拉 + 手动输入
                            if proxy_is_jump {
                                div {
                                    class: "dialog-field",
                                    span { "{t.proxy_jump}" }
                                    select {
                                        class: "dialog-select",
                                        value: "{proxy_jump()}",
                                        onchange: move |evt| proxy_jump.set(evt.value()),
                                        option { value: "", selected: proxy_jump().is_empty(), "{t.proxy_jump_saved_manual}" }
                                        for conn in saved_connections.iter() {
                                            option {
                                                value: "{conn.jump_spec}",
                                                selected: proxy_jump() == conn.jump_spec,
                                                "{conn.name} ({conn.jump_spec})"
                                            }
                                        }
                                    }
                                    input {
                                        r#type: "text",
                                        value: "{proxy_jump()}",
                                        oninput: move |evt| {
                                            proxy_jump.set(evt.value().clone());
                                        },
                                        placeholder: "{t.proxy_jump_placeholder}"
                                    }
                                }
                            }

                            // SOCKS5 / HTTP 代理：主机、端口、用户名 各自全宽竖排
                            if proxy_needs_address {
                                div {
                                    class: "dialog-field",
                                    span { "{t.proxy_host_label}" }
                                    input {
                                        r#type: "text",
                                        value: "{proxy_host()}",
                                        oninput: move |evt| proxy_host.set(evt.value()),
                                        placeholder: "127.0.0.1"
                                    }
                                }
                                div {
                                    class: "dialog-field",
                                    span { "{t.proxy_port_label}" }
                                    input {
                                        r#type: "text",
                                        value: "{proxy_port()}",
                                        oninput: move |evt| proxy_port.set(evt.value()),
                                        placeholder: "1080"
                                    }
                                }
                                div {
                                    class: "dialog-field",
                                    span { "{t.proxy_username_label}" }
                                    input {
                                        r#type: "text",
                                        value: "{proxy_username()}",
                                        oninput: move |evt| proxy_username.set(evt.value()),
                                    }
                                }
                            }
                        }
                    }
                }

                div {
                    class: "dialog-actions",

                    button {
                        class: "dialog-btn",
                        onclick: move |_| {
                            show.set(false);
                        },
                        "{t.cancel}"
                    }

                    button {
                        class: "dialog-btn primary",
                        onclick: move |_| {
                            let name_val = name();
                            let host_val = host();
                            let port_str = port();
                            let user_val = user();
                            let group_val = group();
                            let key_path_val = key_path();
                            let use_agent_val = use_agent();
                            let forward_agent_val = forward_agent();

                            if name_val.is_empty() || host_val.is_empty() || user_val.is_empty() {
                                tracing::warn!("{}", t.required_warning);
                                active_tab.set("session".to_string());
                                return;
                            }

                            let port_val: u16 = port_str.parse().unwrap_or(22);
                            let auth = auth_methods_from_inputs(&key_path_val, use_agent_val);
                            // 代理类型与跳转服务器互斥：jump 存 proxy_jump+Direct，其余存 proxy+None
                            let (proxy_val, proxy_jump_str) = if proxy_type() == "jump" {
                                (ProxyConfig::Direct, proxy_jump())
                            } else {
                                (
                                    proxy_from_inputs(
                                        &proxy_type(),
                                        &proxy_host(),
                                        &proxy_port(),
                                        &proxy_username(),
                                    ),
                                    String::new(),
                                )
                            };

                            let profile = SessionProfile {
                                name: name_val.clone(),
                                group: {
                                    let trimmed = group_val.trim();
                                    if trimmed.is_empty() {
                                        None
                                    } else {
                                        Some(trimmed.to_string())
                                    }
                                },
                                params: ConnectParams {
                                    host: host_val.clone(),
                                    port: port_val,
                                    user: user_val.clone(),
                                    auth,
                                    vault_id: None,
                                    proxy_jump: {
                                        let trimmed = proxy_jump_str.trim();
                                        if trimmed.is_empty() {
                                            None
                                        } else {
                                            Some(trimmed.to_string())
                                        }
                                    },
                                    proxy: proxy_val,
                                    forward_agent: forward_agent_val,
                                },
                            };

                            on_save.call(profile);
                            show.set(false);
                        },
                        "{t.save}"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_methods_include_public_key_before_fallbacks() {
        let auth = auth_methods_from_inputs(" ~/.ssh/id_ed25519 ", true);

        assert_eq!(
            auth,
            vec![
                AuthMethod::PublicKey {
                    key_path: PathBuf::from("~/.ssh/id_ed25519")
                },
                AuthMethod::Agent,
                AuthMethod::Password,
            ]
        );
    }

    #[test]
    fn auth_methods_keep_password_fallback_without_key_or_agent() {
        assert_eq!(
            auth_methods_from_inputs("   ", false),
            vec![AuthMethod::Password]
        );
    }

    #[test]
    fn first_public_key_path_returns_existing_key_for_editing() {
        let auth = vec![
            AuthMethod::Agent,
            AuthMethod::PublicKey {
                key_path: PathBuf::from("/home/me/.ssh/id_rsa"),
            },
            AuthMethod::Password,
        ];

        assert_eq!(first_public_key_path(&auth), "/home/me/.ssh/id_rsa");
    }

    #[test]
    fn proxy_from_inputs_builds_expected_variants() {
        assert_eq!(proxy_from_inputs("direct", "", "", ""), ProxyConfig::Direct);
        assert_eq!(proxy_from_inputs("system", "", "", ""), ProxyConfig::System);
        assert_eq!(
            proxy_from_inputs("socks5", "127.0.0.1", "1080", "me"),
            ProxyConfig::Socks5 {
                host: "127.0.0.1".to_string(),
                port: 1080,
                username: Some("me".to_string()),
            }
        );
        assert_eq!(
            proxy_from_inputs("http", "proxy.local", "8080", ""),
            ProxyConfig::Http {
                host: "proxy.local".to_string(),
                port: 8080,
                username: None,
            }
        );
        // socks5 缺少 host 时回退为直连。
        assert_eq!(
            proxy_from_inputs("socks5", "  ", "1080", ""),
            ProxyConfig::Direct
        );
    }

    #[test]
    fn proxy_kind_label_and_fields_roundtrip() {
        let proxy = ProxyConfig::Socks5 {
            host: "h".to_string(),
            port: 9050,
            username: Some("u".to_string()),
        };
        assert_eq!(proxy_kind_label(&proxy), "socks5");
        assert_eq!(
            proxy_fields(&proxy),
            ("h".to_string(), "9050".to_string(), "u".to_string())
        );
        assert_eq!(proxy_kind_label(&ProxyConfig::System), "system");
    }

    #[test]
    fn proxy_mode_inferred_from_params() {
        // 直连 + 无跳板 → direct
        let mut params = ConnectParams::new("h", "u");
        assert_eq!(proxy_mode(&params), "direct");

        // 直连 + 跳板 → jump
        params.proxy_jump = Some("bastion".into());
        assert_eq!(proxy_mode(&params), "jump");

        // TCP 代理优先于跳板（socks5/http/system 不被跳板覆盖）
        params.proxy = ProxyConfig::Socks5 {
            host: "p".into(),
            port: 1080,
            username: None,
        };
        assert_eq!(proxy_mode(&params), "socks5");

        params.proxy = ProxyConfig::System;
        assert_eq!(proxy_mode(&params), "system");

        // 回到直连且无跳板 → direct
        params.proxy = ProxyConfig::Direct;
        params.proxy_jump = None;
        assert_eq!(proxy_mode(&params), "direct");
    }

    #[test]
    fn saved_connection_refs_exclude_self_and_format_jump_spec() {
        let profiles = vec![
            SessionProfile {
                name: "web".into(),
                group: None,
                params: ConnectParams::new("10.0.0.5", "deploy"),
            },
            SessionProfile {
                name: "db".into(),
                group: None,
                params: ConnectParams {
                    host: "db.local".into(),
                    port: 2222,
                    user: "admin".into(),
                    auth: vec![AuthMethod::Password],
                    vault_id: None,
                    proxy_jump: None,
                    proxy: ProxyConfig::Direct,
                    forward_agent: false,
                },
            },
        ];

        let refs = saved_connection_refs(&profiles, "web");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "db");
        // 非 22 端口需带端口。
        assert_eq!(refs[0].jump_spec, "admin@db.local:2222");

        let refs_default_port = saved_connection_refs(&profiles, "missing");
        assert_eq!(refs_default_port.len(), 2);
        // 22 端口省略端口。
        assert!(refs_default_port
            .iter()
            .any(|r| r.name == "web" && r.jump_spec == "deploy@10.0.0.5"));
    }
}

#[component]
pub fn GroupDialog(
    show: Signal<bool>,
    mode: Signal<String>,
    name: Signal<String>,
    language: AppLanguage,
    on_save: EventHandler<String>,
) -> Element {
    if !show() {
        return rsx! {};
    }

    let t = texts(language).app;
    let title = if mode() == "rename" {
        t.rename_group
    } else {
        t.new_group
    };

    rsx! {
        div {
            class: "settings-overlay",
            onclick: move |_| show.set(false),

            section {
                class: "settings-panel group-dialog",
                onclick: move |evt| evt.stop_propagation(),

                div {
                    class: "settings-head",
                    h2 { "{title}" }
                    button {
                        class: "icon-button slim",
                        title: "{t.close}",
                        onclick: move |_| show.set(false),
                        Icon { name: "close" }
                    }
                }

                div {
                    class: "group-form",
                    input {
                        r#type: "text",
                        value: "{name()}",
                        oninput: move |evt| name.set(evt.value().clone()),
                        placeholder: "{texts(language).dialog.group_placeholder}",
                    }
                }

                div {
                    class: "group-actions",
                    button {
                        onclick: move |_| show.set(false),
                        "{texts(language).dialog.cancel}"
                    }
                    button {
                        class: "primary",
                        onclick: move |_| {
                            let value = name();
                            if normalize_group_name(&value).is_some() {
                                on_save.call(value);
                                show.set(false);
                            }
                        },
                        "{texts(language).dialog.save}"
                    }
                }
            }
        }
    }
}

#[component]
pub fn SftpNameDialog(
    show: Signal<bool>,
    mode: Signal<String>,
    value: Signal<String>,
    language: AppLanguage,
    on_save: EventHandler<String>,
) -> Element {
    if !show() {
        return rsx! {};
    }

    let t = texts(language).sftp;
    let dialog_t = texts(language).dialog;
    let title = if mode() == "rename" {
        t.rename
    } else {
        t.new_folder
    };

    rsx! {
        div {
            class: "settings-overlay",
            onclick: move |_| show.set(false),

            section {
                class: "settings-panel group-dialog",
                onclick: move |evt| evt.stop_propagation(),

                div {
                    class: "settings-head",
                    h2 { "{title}" }
                    button {
                        class: "icon-button slim",
                        title: "{t.close}",
                        onclick: move |_| show.set(false),
                        Icon { name: "close" }
                    }
                }

                div {
                    class: "group-form",
                    input {
                        r#type: "text",
                        value: "{value()}",
                        oninput: move |evt| value.set(evt.value().clone()),
                        placeholder: "{t.name}",
                    }
                }

                div {
                    class: "group-actions",
                    button {
                        onclick: move |_| show.set(false),
                        "{dialog_t.cancel}"
                    }
                    button {
                        class: "primary",
                        onclick: move |_| {
                            let next = value();
                            if !next.trim().is_empty() {
                                on_save.call(next);
                                show.set(false);
                            }
                        },
                        "{dialog_t.save}"
                    }
                }
            }
        }
    }
}
