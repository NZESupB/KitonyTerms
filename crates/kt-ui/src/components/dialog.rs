//! 连接编辑对话框组件

use dioxus::prelude::*;
use kt_config::{AppLanguage, AuthMethod, ConnectParams, SessionProfile};

use crate::i18n::texts;

#[component]
pub fn ConnectionDialog(
    show: Signal<bool>,
    mode: Signal<String>,
    name: Signal<String>,
    host: Signal<String>,
    port: Signal<String>,
    user: Signal<String>,
    password: Signal<String>,
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

                    // 密码
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

                            if name_val.is_empty() || host_val.is_empty() || user_val.is_empty() {
                                tracing::warn!("{}", t.required_warning);
                                return;
                            }

                            let port_val: u16 = port_str.parse().unwrap_or(22);

                            // 创建 SessionProfile
                            let profile = SessionProfile {
                                name: name_val.clone(),
                                group: None,
                                params: ConnectParams {
                                    host: host_val.clone(),
                                    port: port_val,
                                    user: user_val.clone(),
                                    auth: vec![AuthMethod::Password],
                                    vault_id: None,
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
