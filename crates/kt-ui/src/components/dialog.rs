//! 连接编辑对话框组件

use std::path::PathBuf;

use dioxus::prelude::*;
use kt_config::{normalize_group_name, AppLanguage, AuthMethod, ConnectParams, SessionProfile};

use crate::components::icons::Icon;
use crate::i18n::texts;

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

#[component]
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
    use_agent: Signal<bool>,
    forward_agent: Signal<bool>,
    groups: Vec<String>,
    language: AppLanguage,
    on_save: EventHandler<SessionProfile>,
) -> Element {
    if !show() {
        return rsx! {};
    }

    let is_edit = mode() == "edit";
    let t = texts(language).dialog;
    let title = if is_edit { t.edit_title } else { t.new_title };

    rsx! {
        // 模态背景遮罩
        div {
            style: "position: fixed; top: 0; left: 0; right: 0; bottom: 0; background: rgba(0,0,0,0.5); display: flex; align-items: center; justify-content: center; z-index: 1000;",
            onclick: move |_| {
                show.set(false);
            },

            // 对话框内容
            div {
                style: "background: white; border-radius: 8px; padding: 24px; width: 400px; box-shadow: 0 4px 6px rgba(0,0,0,0.1);",
                onclick: move |evt| {
                    evt.stop_propagation();
                },

                // 标题
                h2 {
                    style: "margin: 0 0 20px 0; font-size: 18px; font-weight: 600; color: #1f2937;",
                    "{title}"
                }

                // 表单
                div {
                    style: "display: flex; flex-direction: column; gap: 16px;",

                    // 连接名称
                    div {
                        label {
                            style: "display: block; margin-bottom: 4px; font-size: 14px; color: #374151;",
                            "{t.name}"
                        }
                        input {
                            style: "width: 100%; padding: 8px; border: 1px solid #d1d5db; border-radius: 4px; font-size: 14px;",
                            r#type: "text",
                            value: "{name()}",
                            oninput: move |evt| {
                                name.set(evt.value().clone());
                            },
                            placeholder: "{t.name_placeholder}"
                        }
                    }

                    // 主机地址
                    div {
                        label {
                            style: "display: block; margin-bottom: 4px; font-size: 14px; color: #374151;",
                            "{t.host}"
                        }
                        input {
                            style: "width: 100%; padding: 8px; border: 1px solid #d1d5db; border-radius: 4px; font-size: 14px;",
                            r#type: "text",
                            value: "{host()}",
                            oninput: move |evt| {
                                host.set(evt.value().clone());
                            },
                            placeholder: "{t.host_placeholder}"
                        }
                    }

                    // 端口和用户名（横向排列）
                    div {
                        style: "display: flex; gap: 12px;",
                        div {
                            style: "flex: 1;",
                            label {
                                style: "display: block; margin-bottom: 4px; font-size: 14px; color: #374151;",
                                "{t.port}"
                            }
                            input {
                                style: "width: 100%; padding: 8px; border: 1px solid #d1d5db; border-radius: 4px; font-size: 14px;",
                                r#type: "text",
                                value: "{port()}",
                                oninput: move |evt| {
                                    port.set(evt.value().clone());
                                },
                                placeholder: "22"
                            }
                        }
                        div {
                            style: "flex: 2;",
                            label {
                                style: "display: block; margin-bottom: 4px; font-size: 14px; color: #374151;",
                                "{t.user}"
                            }
                            input {
                                style: "width: 100%; padding: 8px; border: 1px solid #d1d5db; border-radius: 4px; font-size: 14px;",
                                r#type: "text",
                                value: "{user()}",
                                oninput: move |evt| {
                                    user.set(evt.value().clone());
                                },
                                placeholder: "root"
                            }
                        }
                    }

                    // 分组
                    div {
                        label {
                            style: "display: block; margin-bottom: 4px; font-size: 14px; color: #374151;",
                            "{t.group}"
                        }
                        input {
                            style: "width: 100%; padding: 8px; border: 1px solid #d1d5db; border-radius: 4px; font-size: 14px;",
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

                    // 密码会在保存连接时自动写入本机 vault。
                    div {
                        label {
                            style: "display: block; margin-bottom: 4px; font-size: 14px; color: #374151;",
                            "{t.password}"
                        }
                        input {
                            style: "width: 100%; padding: 8px; border: 1px solid #d1d5db; border-radius: 4px; font-size: 14px;",
                            r#type: "password",
                            value: "{password()}",
                            oninput: move |evt| {
                                password.set(evt.value().clone());
                            },
                            placeholder: "{t.password_placeholder}"
                        }
                        p {
                            style: "margin-top: 4px; font-size: 12px; color: #6b7280; line-height: 1.4;",
                            "{t.password_save_hint}"
                        }
                    }

                    div {
                        label {
                            style: "display: block; margin-bottom: 4px; font-size: 14px; color: #374151;",
                            "{t.private_key_path}"
                        }
                        input {
                            style: "width: 100%; padding: 8px; border: 1px solid #d1d5db; border-radius: 4px; font-size: 14px;",
                            r#type: "text",
                            value: "{key_path()}",
                            oninput: move |evt| {
                                key_path.set(evt.value().clone());
                            },
                            placeholder: "{t.private_key_path_placeholder}"
                        }
                    }

                    div {
                        label {
                            style: "display: block; margin-bottom: 4px; font-size: 14px; color: #374151;",
                            "{t.proxy_jump}"
                        }
                        input {
                            style: "width: 100%; padding: 8px; border: 1px solid #d1d5db; border-radius: 4px; font-size: 14px;",
                            r#type: "text",
                            value: "{proxy_jump()}",
                            oninput: move |evt| {
                                proxy_jump.set(evt.value().clone());
                            },
                            placeholder: "{t.proxy_jump_placeholder}"
                        }
                    }

                    fieldset {
                        style: "border: 1px solid #e5e7eb; border-radius: 6px; padding: 10px 12px; margin: 0;",
                        legend {
                            style: "padding: 0 4px; font-size: 13px; color: #4b5563;",
                            "{t.auth_options}"
                        }
                        label {
                            style: "display: flex; align-items: center; gap: 8px; color: #374151; font-size: 14px; margin-bottom: 8px;",
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
                            style: "display: flex; align-items: center; gap: 8px; color: #374151; font-size: 14px;",
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

                // 按钮组
                div {
                    style: "margin-top: 24px; display: flex; justify-content: flex-end; gap: 8px;",

                    button {
                        style: "padding: 8px 16px; background: #e5e7eb; color: #374151; border: none; border-radius: 4px; cursor: pointer; font-size: 14px;",
                        onclick: move |_| {
                            show.set(false);
                        },
                        "{t.cancel}"
                    }

                    button {
                        style: "padding: 8px 16px; background: #2b7de9; color: white; border: none; border-radius: 4px; cursor: pointer; font-size: 14px;",
                        onclick: move |_| {
                            // 验证输入
                            let name_val = name();
                            let host_val = host();
                            let port_str = port();
                            let user_val = user();
                            let group_val = group();
                            let key_path_val = key_path();
                            let proxy_jump_val = proxy_jump();
                            let use_agent_val = use_agent();
                            let forward_agent_val = forward_agent();

                            if name_val.is_empty() || host_val.is_empty() || user_val.is_empty() {
                                tracing::warn!("{}", t.required_warning);
                                return;
                            }

                            let port_val: u16 = port_str.parse().unwrap_or(22);
                            let auth = auth_methods_from_inputs(&key_path_val, use_agent_val);

                            // 创建 SessionProfile
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
                                        let trimmed = proxy_jump_val.trim();
                                        if trimmed.is_empty() {
                                            None
                                        } else {
                                            Some(trimmed.to_string())
                                        }
                                    },
                                    forward_agent: forward_agent_val,
                                },
                            };

                            // 调用保存回调
                            on_save.call(profile);

                            // 关闭对话框
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
