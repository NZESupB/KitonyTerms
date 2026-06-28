//! 安全与认证相关对话框。

use dioxus::prelude::*;
use kt_config::AppLanguage;
use kt_core::{AuthChallenge, AuthPrompt};

use crate::components::icons::Icon;
use crate::i18n::texts;
use crate::store::PendingHostKey;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingSecretSave {
    pub vault_id: String,
    pub password: String,
}

#[component]
pub fn AuthChallengeDialog(
    session_title: String,
    challenge: AuthChallenge,
    language: AppLanguage,
    on_submit: EventHandler<Vec<String>>,
    on_cancel: EventHandler<()>,
) -> Element {
    let dialog_t = texts(language).dialog;
    let (title, body, prompts) = match &challenge {
        AuthChallenge::Password { user, host, port } => (
            dialog_t.auth_password_title,
            format!("{}: {user}@{host}:{port}", dialog_t.auth_password_body),
            vec![AuthPrompt {
                text: dialog_t.password.to_string(),
                echo: false,
            }],
        ),
        AuthChallenge::KeyPassphrase { key_path } => (
            dialog_t.auth_passphrase_title,
            format!("{}: {key_path}", dialog_t.auth_passphrase_body),
            vec![AuthPrompt {
                text: dialog_t.auth_passphrase_label.to_string(),
                echo: false,
            }],
        ),
        AuthChallenge::KeyboardInteractive {
            name,
            instructions,
            prompts,
        } => {
            let body = if instructions.is_empty() {
                format!("{}: {name}", dialog_t.auth_keyboard_body)
            } else {
                format!("{}: {name}\n{instructions}", dialog_t.auth_keyboard_body)
            };
            (dialog_t.auth_keyboard_title, body, prompts.clone())
        }
    };
    let mut answers = use_signal(|| vec![String::new(); prompts.len()]);
    let answer_values = answers();
    let prompt_rows: Vec<(usize, AuthPrompt, String)> = prompts
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, prompt)| {
            let value = answer_values.get(index).cloned().unwrap_or_default();
            (index, prompt, value)
        })
        .collect();

    rsx! {
        div {
            class: "settings-overlay",

            section {
                class: "settings-panel external-edit-dialog",
                onclick: move |evt| evt.stop_propagation(),

                div {
                    class: "settings-head",
                    h2 { "{title}" }
                    button {
                        class: "icon-button slim",
                        title: "{dialog_t.cancel}",
                        onclick: move |_| on_cancel.call(()),
                        Icon { name: "close" }
                    }
                }

                div {
                    class: "external-edit-dialog-body",
                    Icon { name: "shield" }
                    div {
                        strong { "{session_title}" }
                        for line in body.lines() {
                            p { "{line}" }
                        }
                        if prompt_rows.is_empty() {
                            p { "{dialog_t.auth_no_prompt}" }
                        }
                        for (index, prompt, value) in prompt_rows {
                            label {
                                class: "dialog-field",
                                span { "{prompt.text}" }
                                if prompt.echo {
                                    input {
                                        r#type: "text",
                                        value: "{value}",
                                        placeholder: "{dialog_t.auth_prompt_placeholder}",
                                        oninput: move |evt| {
                                            let mut next = answers.peek().clone();
                                            if let Some(answer) = next.get_mut(index) {
                                                *answer = evt.value().clone();
                                            }
                                            answers.set(next);
                                        },
                                    }
                                } else {
                                    input {
                                        r#type: "password",
                                        value: "{value}",
                                        placeholder: "{dialog_t.auth_prompt_placeholder}",
                                        oninput: move |evt| {
                                            let mut next = answers.peek().clone();
                                            if let Some(answer) = next.get_mut(index) {
                                                *answer = evt.value().clone();
                                            }
                                            answers.set(next);
                                        },
                                    }
                                }
                            }
                        }
                    }
                }

                div {
                    class: "external-edit-dialog-actions",
                    button {
                        class: "primary",
                        onclick: move |_| on_submit.call(answers()),
                        "{dialog_t.auth_submit}"
                    }
                    button {
                        onclick: move |_| on_cancel.call(()),
                        "{dialog_t.cancel}"
                    }
                }
            }
        }
    }
}

#[component]
pub fn HostKeyConfirmDialog(
    prompt: PendingHostKey,
    language: AppLanguage,
    error: Option<String>,
    on_trust: EventHandler<PendingHostKey>,
    on_allow_once: EventHandler<PendingHostKey>,
    on_cancel: EventHandler<()>,
) -> Element {
    let dialog_t = texts(language).dialog;
    let title = if prompt.is_changed() {
        dialog_t.host_key_changed_title
    } else {
        dialog_t.host_key_unknown_title
    };
    let body = if prompt.is_changed() {
        dialog_t.host_key_changed_body
    } else {
        dialog_t.host_key_unknown_body
    };
    let host_label = format!("{}:{}", prompt.host, prompt.port);
    let allow_once_prompt = prompt.clone();
    let trust_prompt = prompt.clone();

    rsx! {
        div {
            class: "settings-overlay",

            section {
                class: "settings-panel external-edit-dialog",
                onclick: move |evt| evt.stop_propagation(),

                div {
                    class: "settings-head",
                    h2 { "{title}" }
                    button {
                        class: "icon-button slim",
                        title: "{dialog_t.cancel}",
                        onclick: move |_| on_cancel.call(()),
                        Icon { name: "close" }
                    }
                }

                div {
                    class: "external-edit-dialog-body",
                    Icon { name: "shield" }
                    div {
                        strong { "{host_label}" }
                        p { "{body}" }
                        p { "{dialog_t.host_key_reconnect_hint}" }
                        if let Some(expected) = prompt.expected.as_ref() {
                            p {
                                strong { "{dialog_t.host_key_expected}: " }
                                code { "{expected}" }
                            }
                        }
                        p {
                            strong { "{dialog_t.host_key_received}: " }
                            code { "{prompt.fingerprint}" }
                        }
                        if let Some(error) = error.as_ref() {
                            p { class: "dialog-error", "{error}" }
                        }
                    }
                }

                div {
                    class: "external-edit-dialog-actions",
                    button {
                        onclick: move |_| on_allow_once.call(allow_once_prompt.clone()),
                        "{dialog_t.host_key_allow_once}"
                    }
                    button {
                        class: "primary",
                        onclick: move |_| on_trust.call(trust_prompt.clone()),
                        "{dialog_t.host_key_trust}"
                    }
                    button {
                        onclick: move |_| on_cancel.call(()),
                        "{dialog_t.cancel}"
                    }
                }
            }
        }
    }
}

#[component]
pub fn VaultUnlockDialog(
    pending: PendingSecretSave,
    language: AppLanguage,
    master_password: Signal<String>,
    error: Option<String>,
    on_unlock: EventHandler<String>,
    on_cancel: EventHandler<()>,
) -> Element {
    let dialog_t = texts(language).dialog;

    rsx! {
        div {
            class: "settings-overlay",

            section {
                class: "settings-panel external-edit-dialog",
                onclick: move |evt| evt.stop_propagation(),

                div {
                    class: "settings-head",
                    h2 { "{dialog_t.vault_unlock_title}" }
                    button {
                        class: "icon-button slim",
                        title: "{dialog_t.cancel}",
                        onclick: move |_| on_cancel.call(()),
                        Icon { name: "close" }
                    }
                }

                div {
                    class: "external-edit-dialog-body",
                    Icon { name: "shield" }
                    div {
                        strong { "{pending.vault_id}" }
                        p { "{dialog_t.vault_unlock_body}" }
                        label {
                            class: "dialog-field",
                            span { "{dialog_t.vault_master_password}" }
                            input {
                                r#type: "password",
                                value: "{master_password()}",
                                placeholder: "{dialog_t.vault_master_password_placeholder}",
                                oninput: move |evt| master_password.set(evt.value().clone()),
                            }
                        }
                        if let Some(error) = error.as_ref() {
                            p { class: "dialog-error", "{error}" }
                        }
                    }
                }

                div {
                    class: "external-edit-dialog-actions",
                    button {
                        class: "primary",
                        onclick: move |_| on_unlock.call(master_password()),
                        "{dialog_t.vault_unlock}"
                    }
                    button {
                        onclick: move |_| on_cancel.call(()),
                        "{dialog_t.cancel}"
                    }
                }
            }
        }
    }
}
