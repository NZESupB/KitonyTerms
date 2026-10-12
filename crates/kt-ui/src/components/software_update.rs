//! 更新界面及重启前收尾，后台工作由 kt-update 负责。
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
    confirm: Signal<bool>,
    restarting: Signal<bool>,
    safety: Rc<dyn Fn() -> RestartSafety>,
}
pub fn use_software_updates(safety: impl Fn() -> RestartSafety + 'static) -> Signal<bool> {
    let context = use_context::<UpdateContext>();
    let snapshot = use_signal(|| context.service.snapshot());
    let error = use_signal(|| None::<String>);
    let confirm = use_signal(|| false);
    let restarting = use_signal(|| false);
    let safety = use_hook(|| Rc::new(safety) as Rc<dyn Fn() -> RestartSafety>);
    let ui = UpdateUi {
        context: context.clone(),
        snapshot,
        error,
        confirm,
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
fn request_restart(mut ui: UpdateUi, language: AppLanguage) {
    ui.error.set(None);
    if (ui.safety)().blocked() {
        ui.error
            .set(Some(texts(language).app.update_blocked.into()));
    } else {
        ui.confirm.set(true);
    }
}
fn restart(mut ui: UpdateUi, language: AppLanguage) {
    if (ui.restarting)() {
        return;
    }
    if (ui.safety)().blocked() {
        ui.confirm.set(false);
        ui.error
            .set(Some(texts(language).app.update_blocked.into()));
        return;
    }
    ui.confirm.set(false);
    ui.restarting.set(true);
    ui.error.set(None);
    let window = dioxus::desktop::use_window();
    spawn(async move {
        let service = Arc::clone(&ui.context.service);
        let launch = ui.context.launch;
        let result = tokio::task::spawn_blocking(move || launch(&service)).await;
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

#[component]
pub fn SoftwareUpdateSection(language: AppLanguage) -> Element {
    let ui = use_context::<UpdateUi>();
    let snapshot = (ui.snapshot)();
    let t = texts(language).app;
    let disabled = snapshot.phase == Phase::Disabled;
    let busy = snapshot.phase.busy() || (ui.restarting)();
    let current = ui.context.service.current.label();
    let target = snapshot.available.as_ref().map(|a| a.info.label());
    let last = snapshot
        .last_check
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds as i64, 0))
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "—".into());
    let status = match snapshot.phase {
        Phase::Disabled => t.update_development,
        Phase::Idle if snapshot.last_check.is_some() => t.update_latest,
        Phase::Idle => t.update_not_checked,
        Phase::Checking => t.update_checking,
        Phase::Available => t.update_available,
        Phase::Downloading => t.update_downloading,
        Phase::Ready => t.update_ready,
        Phase::Preparing => t.update_preparing,
        Phase::Error => t.update_failed,
    };
    let error = (ui.error)().or(snapshot.message);
    rsx! {
        div { class: "software-update-section",
            h3 { "{current}" }
            p { "{t.update_hint}" }
            div { class: "settings-row",
                label { "{t.update_auto_check}" }
                input { r#type: "checkbox", checked: snapshot.preferences.auto_check, disabled: disabled || busy,
                    onchange: { let mut ui = ui.clone(); move |event: Event<FormData>| { let mut prefs = ui.context.service.snapshot().preferences; prefs.auto_check = event.checked(); if let Err(e) = ui.context.service.preferences(prefs) { ui.error.set(Some(e.to_string())); } } }
                }
            }
            div { class: "settings-row",
                label { "{t.update_auto_download}" }
                input { r#type: "checkbox", checked: snapshot.preferences.auto_download, disabled: disabled || busy,
                    onchange: { let mut ui = ui.clone(); move |event: Event<FormData>| { let mut prefs = ui.context.service.snapshot().preferences; prefs.auto_download = event.checked(); if let Err(e) = ui.context.service.preferences(prefs) { ui.error.set(Some(e.to_string())); } } }
                }
            }
            p { "{t.update_last_check}: {last}" }
            p { role: "status", "{status}" }
            if let Some(target) = target { p { "{t.update_target}: {target}" } }
            if snapshot.phase == Phase::Downloading {
                progress { max: snapshot.available.as_ref().map(|a| a.package.size).unwrap_or(1), value: snapshot.received }
                p { "{snapshot.received} / {snapshot.available.as_ref().map(|a| a.package.size).unwrap_or(0)} bytes" }
            }
            if let Some(error) = error { p { class: "software-update-error", role: "alert", "{error}" } }
            div { class: "software-update-actions",
                button { class: "secondary", disabled: disabled || busy || snapshot.phase == Phase::Ready,
                    onclick: { let mut ui = ui.clone(); move |_| { ui.error.set(None); if let Err(e) = ui.context.service.check() { ui.error.set(Some(e.to_string())); } } }, "{t.update_check}"
                }
                if snapshot.available.is_some() && !matches!(snapshot.phase, Phase::Ready | Phase::Preparing | Phase::Downloading) {
                    button { disabled: busy, onclick: { let mut ui = ui.clone(); move |_| { ui.error.set(None); if let Err(e) = ui.context.service.download() { ui.error.set(Some(e.to_string())); } } }, "{t.update_download}" }
                }
                if matches!(snapshot.phase, Phase::Checking | Phase::Downloading) {
                    button { class: "secondary", onclick: { let ui = ui.clone(); move |_| ui.context.service.cancel() }, {texts(language).dialog.cancel} }
                }
                if snapshot.phase == Phase::Ready {
                    button { disabled: busy, onclick: { let ui = ui.clone(); move |_| request_restart(ui.clone(), language) }, "{t.update_restart}" }
                }
            }
            a { href: kt_update::client::RELEASE_PAGE, target: "_blank", rel: "noopener noreferrer", "{t.update_release_page}" }
        }
    }
}
#[component]
pub fn SoftwareUpdateOverlay(language: AppLanguage) -> Element {
    let ui = use_context::<UpdateUi>();
    let mut dismissed = use_signal(|| false);
    let t = texts(language).app;
    if (ui.restarting)() {
        return rsx! { div { class: "software-update-overlay", role: "dialog", aria_modal: "true", aria_label: t.update_preparing, div { class: "software-update-dialog", p { role: "status", "{t.update_preparing}" } } } };
    }
    if (ui.confirm)() {
        return rsx! { div { class: "software-update-overlay", role: "dialog", aria_modal: "true", aria_label: t.update_restart,
            div { class: "software-update-dialog",
                h3 { "{t.update_restart}" } p { "{t.update_restart_hint}" }
                div { class: "software-update-actions",
                    button { onclick: { let ui = ui.clone(); move |_| restart(ui.clone(), language) }, "{t.update_restart}" }
                    button { onclick: { let mut ui = ui.clone(); move |_| ui.confirm.set(false) }, "{t.update_later}" }
                }
            }
        } };
    }
    if (ui.snapshot)().phase == Phase::Ready && !dismissed() {
        rsx! { div { class: "software-update-notice", role: "status",
            span { "{t.update_ready}" }
            button { onclick: { let ui = ui.clone(); move |_| request_restart(ui.clone(), language) }, "{t.update_restart}" }
            button { onclick: move |_| dismissed.set(true), "{t.update_later}" }
            if let Some(error) = (ui.error)() { p { role: "alert", "{error}" } }
        } }
    } else {
        rsx! {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
