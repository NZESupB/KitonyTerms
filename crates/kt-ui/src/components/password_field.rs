//! 保存密码的查看状态与编辑值隔离，系统认证结果只能作用于本次请求的目标。

use std::{cell::RefCell, rc::Rc};

use dioxus::prelude::*;
use kt_config::AppLanguage;
use zeroize::Zeroizing;

use crate::{
    components::icons::Icon,
    i18n::texts,
    os_auth::{self, AuthError, AuthMethod},
};

#[derive(Clone, PartialEq, Eq)]
struct RevealTarget {
    vault_id: Option<String>,
    connection: String,
    host: String,
    port: String,
    user: String,
}

#[derive(Default)]
struct RevealState {
    generation: u64,
    pending: bool,
    target: Option<RevealTarget>,
    secret: Option<Zeroizing<String>>,
    notice: Option<&'static str>,
}

impl RevealState {
    fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = false;
        self.target = None;
        self.secret = None;
        self.notice = None;
    }

    fn begin(&mut self, target: RevealTarget) -> u64 {
        self.clear();
        self.pending = true;
        self.target = Some(target);
        self.generation
    }

    fn accepts(&self, generation: u64, target: &RevealTarget) -> bool {
        self.generation == generation && self.target.as_ref() == Some(target)
    }
}

#[component]
#[allow(clippy::too_many_arguments)]
pub(super) fn PasswordField(
    password: Signal<String>,
    show: Signal<bool>,
    original_name: Signal<String>,
    host: Signal<String>,
    port: Signal<String>,
    user: Signal<String>,
    saved_vault_id: Option<String>,
    language: AppLanguage,
) -> Element {
    let t = texts(language).dialog;
    let mut visible = use_signal(|| false);
    let mut reveal = use_signal(RevealState::default);
    let target = RevealTarget {
        vault_id: saved_vault_id.clone(),
        connection: original_name(),
        host: host(),
        port: port(),
        user: user(),
    };
    let live_target = use_hook(|| Rc::new(RefCell::new(target.clone())));
    *live_target.borrow_mut() = target.clone();
    use_effect(use_reactive((&target,), move |_| {
        reveal.write().clear();
        visible.set(false);
    }));
    #[cfg(windows)]
    let owner = {
        use dioxus::desktop::tao::platform::windows::WindowExtWindows;
        dioxus::desktop::use_window().window.hwnd()
    };
    #[cfg(not(windows))]
    let owner = 0;

    let pending = reveal.read().pending;
    let secret = if reveal.read().target.as_ref() == Some(&target) {
        reveal.read().secret.clone()
    } else {
        None
    };
    let notice = reveal.read().notice;
    let has_saved = saved_vault_id.is_some();
    let revealed = secret.is_some();

    let begin_reveal = use_callback(move |method: AuthMethod| {
        let request_target = live_target.borrow().clone();
        let Some(vault_id) = request_target.vault_id.clone() else {
            return;
        };
        if reveal.peek().pending || !password().is_empty() {
            return;
        }
        let generation = reveal.write().begin(request_target.clone());
        let live_target = live_target.clone();
        spawn(async move {
            let result =
                os_auth::authenticate(t.password_auth_reason.to_string(), owner, method).await;
            if !show()
                || !password().is_empty()
                || *live_target.borrow() != request_target
                || !reveal.peek().accepts(generation, &request_target)
                || original_name() != request_target.connection
                || host() != request_target.host
                || port() != request_target.port
                || user() != request_target.user
            {
                return;
            }
            reveal.write().pending = false;
            match result {
                Ok(verified) => {
                    match crate::components::app::get_store().reveal_secret(&vault_id, verified) {
                        Ok(Some(secret)) => reveal.write().secret = Some(secret),
                        Ok(None) | Err(_) => reveal.write().notice = Some(t.password_missing),
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    if reveal.peek().accepts(generation, &request_target) {
                        reveal.write().clear();
                    }
                }
                Err(error) => {
                    reveal.write().notice = Some(match error {
                        AuthError::Cancelled => t.password_auth_cancelled,
                        AuthError::Unavailable => t.password_auth_unavailable,
                        AuthError::Failed => t.password_auth_failed,
                        AuthError::WrongCredentials => t.password_auth_wrong,
                        AuthError::AccountRestricted => t.password_auth_restricted,
                        AuthError::AccountLocked => t.password_auth_locked,
                        AuthError::Busy => t.password_auth_busy,
                    })
                }
            }
        });
    });

    let toggle_icon = if visible() { "eye-off" } else { "eye" };
    let reveal_icon = if revealed { "eye-off" } else { "shield" };
    rsx! {
        div { class: "dialog-field",
            span { "{t.password}" }
            div { class: "password-input-wrap",
                input {
                    r#type: if visible() { "text" } else { "password" },
                    value: "{password()}",
                    autocomplete: "new-password",
                    oninput: move |event| {
                        reveal.write().clear();
                        password.set(event.value());
                    },
                    placeholder: if has_saved { t.saved_password } else { t.password_placeholder },
                }
                button {
                    r#type: "button", class: "password-toggle",
                    title: if visible() { t.hide_password } else { t.show_password },
                    aria_label: if visible() { t.hide_password } else { t.show_password },
                    disabled: password().is_empty(),
                    onclick: move |_| visible.set(!visible()),
                    Icon { name: toggle_icon }
                }
            }
            if has_saved {
                button {
                    r#type: "button", class: "dialog-btn password-saved-reveal",
                    disabled: pending || !password().is_empty(),
                    onclick: move |_| {
                        if revealed { reveal.write().clear(); }
                        else { begin_reveal.call(AuthMethod::Auto); }
                    },
                    Icon { name: reveal_icon }
                    if pending { "{t.password_auth_pending}" }
                    else if revealed { "{t.hide_password}" }
                    else { "{t.reveal_saved_password}" }
                }
                if cfg!(windows) && !revealed {
                    button {
                        r#type: "button", class: "dialog-btn password-saved-reveal",
                        disabled: pending || !password().is_empty(),
                        onclick: move |_| begin_reveal.call(AuthMethod::AccountPassword),
                        "{t.use_account_password}"
                    }
                }
            }
            if let Some(secret) = secret {
                input {
                    class: "password-saved-reveal",
                    r#type: "text", readonly: true,
                    value: secret.as_str(),
                    aria_label: "{t.reveal_saved_password}",
                    autocomplete: "off", spellcheck: "false",
                }
            }
            if let Some(notice) = notice {
                p { class: "password-auth-status", role: "status", "{notice}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(name: &str) -> RevealTarget {
        RevealTarget {
            vault_id: Some(name.into()),
            connection: name.into(),
            host: "host".into(),
            port: "22".into(),
            user: "root".into(),
        }
    }

    #[test]
    fn late_authentication_cannot_reveal_changed_or_closed_target() {
        let mut state = RevealState::default();
        let first = target("first");
        let generation = state.begin(first.clone());
        assert!(state.accepts(generation, &first));
        assert!(!state.accepts(generation, &target("other")));
        state.clear();
        assert!(!state.accepts(generation, &first));
        let next = state.begin(first.clone());
        assert!(!state.accepts(generation, &first));
        assert!(state.accepts(next, &first));
    }

    #[test]
    fn hiding_or_editing_discards_plaintext_and_pending_state() {
        let mut state = RevealState::default();
        state.begin(target("first"));
        state.secret = Some(Zeroizing::new("test-value".into()));
        state.clear();
        assert!(state.secret.is_none());
        assert!(!state.pending);
        assert!(state.target.is_none());
    }
}
