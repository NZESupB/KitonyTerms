//! 更新界面及重启前收尾，后台工作由 kt-update 负责。
mod release_notes;
mod view;

use self::view::{RestartNotice, UpdateAction, UpdateError, UpdateView};
use crate::components::icons::Icon;
use crate::{components::app::get_state, i18n::texts};
use dioxus::prelude::*;
use kt_config::AppLanguage;
use kt_update::{Phase, Snapshot, UpdateContext};
use std::{rc::Rc, sync::Arc, time::Duration};

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct RestartSafety {
    pub editing: bool,
    pub transferring: bool,
    pub syncing: bool,
    pub dialog: bool,
}
impl RestartSafety {
    pub fn blocked(self) -> bool {
        self.editing || self.transferring || self.syncing || self.dialog
    }
}
#[derive(Clone)]
struct UpdateUi {
    context: UpdateContext,
    snapshot: Signal<Snapshot>,
    error: Signal<Option<String>>,
    restarting: Signal<bool>,
    safety: Rc<dyn Fn() -> RestartSafety>,
}
pub fn use_software_updates(safety: impl Fn() -> RestartSafety + 'static) -> Signal<bool> {
    let context = use_context::<UpdateContext>();
    let snapshot = use_signal(|| context.service.snapshot());
    let error = use_signal(|| None::<String>);
    let restarting = use_signal(|| false);
    let safety = use_hook(|| Rc::new(safety) as Rc<dyn Fn() -> RestartSafety>);
    let ui = UpdateUi {
        context: context.clone(),
        snapshot,
        error,
        restarting,
        safety,
    };
    use_context_provider(|| ui);
    let service = context.service;
    let mut sink = snapshot;
    let mut error_sink = error;
    use_future(move || {
        let service = Arc::clone(&service);
        async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let confirmation = Arc::clone(&service);
            match tokio::task::spawn_blocking(move || confirmation.confirm_startup()).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => error_sink.set(Some(format!("{e:#}"))),
                Err(e) => error_sink.set(Some(e.to_string())),
            }
            loop {
                let next = service.snapshot();
                if *sink.peek() != next {
                    sink.set(next);
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    });
    restarting
}
fn restart_permitted(restarting: bool, phase: Phase, safety: RestartSafety) -> Result<bool, ()> {
    if restarting || phase != Phase::Ready {
        return Ok(false);
    }
    if safety.blocked() {
        return Err(());
    }
    Ok(true)
}

fn restart(mut ui: UpdateUi, language: AppLanguage) {
    match restart_permitted(
        (ui.restarting)(),
        ui.context.service.snapshot().phase,
        (ui.safety)(),
    ) {
        Ok(true) => {}
        Ok(false) => return,
        Err(()) => {
            ui.error
                .set(Some(texts(language).app.update_blocked.into()));
            return;
        }
    }
    ui.restarting.set(true);
    ui.error.set(None);
    let elevate = ui.context.service.requires_elevation();
    let window = dioxus::desktop::use_window();
    spawn(async move {
        let service = Arc::clone(&ui.context.service);
        let launch = ui.context.launch;
        let result = tokio::task::spawn_blocking(move || launch(&service, elevate)).await;
        let error = match result {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(format!("{e:#}")),
            Err(e) => Some(e.to_string()),
        };
        if let Some(error) = error {
            ui.context.service.preparation_failed(error.clone());
            ui.error.set(Some(error));
            ui.restarting.set(false);
            return;
        }
        // 准备期间仍可能收到先前排队的事件，握手后再检查一次。
        if (ui.safety)().blocked() {
            let result = ui.context.service.abort_restart();
            let message = result
                .err()
                .map(|e| e.to_string())
                .unwrap_or_else(|| texts(language).app.update_blocked.into());
            ui.context.service.preparation_failed(message.clone());
            ui.error.set(Some(message));
            ui.restarting.set(false);
            return;
        }
        let (sender, ids) = {
            let mut state = get_state().lock().unwrap();
            let ids = state.sessions.keys().copied().collect::<Vec<_>>();
            for id in &ids {
                state.cancel_operations(*id);
            }
            (state.manager.raw_sender(), ids)
        };
        for id in ids {
            if let Err(e) = sender
                .send(kt_core::session::ToCore::Disconnect { id })
                .await
            {
                let message = e.to_string();
                if let Err(abort) = ui.context.service.abort_restart() {
                    ui.error.set(Some(abort.to_string()));
                } else {
                    ui.error.set(Some(message.clone()));
                }
                ui.context.service.preparation_failed(message);
                ui.restarting.set(false);
                return;
            }
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while get_state()
            .lock()
            .unwrap()
            .sessions
            .values()
            .any(|s| s.connected)
        {
            if tokio::time::Instant::now() >= deadline {
                let message = texts(language).app.update_disconnect_timeout.to_string();
                if let Err(e) = ui.context.service.abort_restart() {
                    ui.error.set(Some(e.to_string()));
                } else {
                    ui.error.set(Some(message.clone()));
                }
                ui.context.service.preparation_failed(message);
                ui.restarting.set(false);
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if let Err(e) = ui.context.service.commit_restart() {
            let message = e.to_string();
            if let Err(abort) = ui.context.service.abort_restart() {
                ui.error.set(Some(abort.to_string()));
            } else {
                ui.error.set(Some(message.clone()));
            }
            ui.context.service.preparation_failed(message);
            ui.restarting.set(false);
            return;
        }
        window.close();
    });
}

fn act(mut ui: UpdateUi, action: UpdateAction, language: AppLanguage) {
    ui.error.set(None);
    let result = match action {
        UpdateAction::Check => ui.context.service.check(),
        UpdateAction::Download => ui.context.service.download(),
        UpdateAction::Cancel => {
            ui.context.service.cancel();
            Ok(())
        }
        UpdateAction::Restart => {
            restart(ui.clone(), language);
            Ok(())
        }
    };
    if let Err(error) = result {
        ui.error.set(Some(error.to_string()));
    }
    ui.snapshot.set(ui.context.service.snapshot());
}

#[component]
pub fn SoftwareUpdateSection(language: AppLanguage) -> Element {
    let ui = use_context::<UpdateUi>();
    let snapshot = (ui.snapshot)();
    let error = (ui.error)().or_else(|| snapshot.message.clone());
    rsx! {
        UpdateView {
            language,
            snapshot,
            current: ui.context.service.current.clone(),
            error,
            restarting: (ui.restarting)(),
            on_action: { let ui = ui.clone(); move |action| act(ui.clone(), action, language) },
            on_preferences: { let mut ui = ui.clone(); move |preferences| {
                ui.error.set(None);
                if let Err(error) = ui.context.service.preferences(preferences) {
                    ui.error.set(Some(error.to_string()));
                }
                ui.snapshot.set(ui.context.service.snapshot());
            } },
            on_proxy: { let mut ui = ui.clone(); move |proxy| {
                ui.error.set(None);
                let preferences = kt_update::Preferences {
                    proxy,
                    ..(ui.snapshot)().preferences
                };
                if let Err(error) = ui.context.service.preferences(preferences) {
                    ui.error.set(Some(error.to_string()));
                }
                ui.snapshot.set(ui.context.service.snapshot());
            } },
        }
    }
}

#[component]
pub fn SoftwareUpdateOverlay(
    language: AppLanguage,
    theme: String,
    accent: String,
    accent_rgb: String,
    ui_font: Option<String>,
    ui_scale: f32,
) -> Element {
    let ui = use_context::<UpdateUi>();
    let mut dismissed = use_signal(|| None::<kt_update::metadata::BuildInfo>);
    let restarting = ui.restarting;
    let snapshot = (ui.snapshot)();
    let target = snapshot
        .available
        .as_ref()
        .map(|release| release.info.clone());
    let label = target.as_ref().map(|info| info.label()).unwrap_or_default();
    let t = texts(language).app;
    let notice_icon = if restarting() { "refresh" } else { "check" };
    let notice_title = if restarting() {
        t.update_preparing
    } else {
        t.update_notice_title
    };
    let notice = matches!(snapshot.phase, Phase::Ready | Phase::Preparing)
        && target.is_some()
        && (dismissed() != target || restarting());
    let ink = crate::components::theme::accent_foreground(&accent);
    let layer_style = crate::components::theme::interface_root_style(
        &accent,
        &accent_rgb,
        ink,
        ui_font.as_deref(),
        ui_scale,
    );
    let theme_class = crate::components::main_shell::theme_class(&theme);
    rsx! {
        div {
            class: "software-update-layer {theme_class}",
            style: "{layer_style}",
            if notice {
                aside { class: "software-update-notice", aria_label: t.update_notice_title,
                    div { class: if restarting() { "update-notice-icon is-spinning" } else { "update-notice-icon" }, aria_hidden: "true", Icon { name: notice_icon } }
                    div { class: "update-notice-copy", role: "status", strong { "{notice_title}" } span { "KitonyTerms {label}" } }
                    div { class: "software-update-actions",
                        button { class: "settings-button update-button", r#type: "button", disabled: restarting(), onclick: move |_| dismissed.set(target.clone()), "{t.update_later}" }
                        button { class: "settings-button primary update-button", r#type: "button", disabled: restarting(), onclick: { let ui = ui.clone(); move |_| restart(ui.clone(), language) }, "{t.update_restart}" }
                    }
                    if restarting() { p { class: "update-notice-hint", role: "status", "{t.update_preparing_hint}" } }
                    else { RestartNotice { language, elevated: snapshot.requires_elevation } }
                    if let Some(error) = (ui.error)().or(snapshot.message) { UpdateError { message: error, language } }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notification_uses_the_resolved_window_theme_including_system_changes() {
        use crate::components::{main_shell::theme_class, theme::concrete_theme_name};
        for (preference, light, expected) in [
            ("default-light", false, "theme-light"),
            ("light", false, "theme-light"),
            ("default-dark", true, "theme-dark"),
            ("dark", true, "theme-dark"),
            ("system", true, "theme-light"),
            ("system", false, "theme-dark"),
            ("system", true, "theme-light"),
        ] {
            assert_eq!(
                theme_class(concrete_theme_name(preference, light)),
                expected
            );
        }
    }

    #[test]
    fn restart_starts_directly_once_and_can_retry_after_preparation_failure() {
        let safe = RestartSafety::default();
        assert_eq!(restart_permitted(false, Phase::Ready, safe), Ok(true));
        assert_eq!(restart_permitted(true, Phase::Ready, safe), Ok(false));
        assert_eq!(restart_permitted(false, Phase::Preparing, safe), Ok(false));
        assert_eq!(restart_permitted(false, Phase::Available, safe), Ok(false));
        assert_eq!(
            restart_permitted(
                false,
                Phase::Ready,
                RestartSafety {
                    editing: true,
                    ..safe
                }
            ),
            Err(())
        );
        assert_eq!(restart_permitted(false, Phase::Ready, safe), Ok(true));
    }

    #[test]
    fn restart_protects_each_kind_of_pending_work() {
        assert!(!RestartSafety::default().blocked());
        for safety in [
            RestartSafety {
                editing: true,
                ..Default::default()
            },
            RestartSafety {
                transferring: true,
                ..Default::default()
            },
            RestartSafety {
                syncing: true,
                ..Default::default()
            },
            RestartSafety {
                dialog: true,
                ..Default::default()
            },
        ] {
            assert!(safety.blocked());
        }
    }
}
