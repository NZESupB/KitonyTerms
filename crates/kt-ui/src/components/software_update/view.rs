//! 更新展示与服务操作分离，便于使用隔离快照核验所有界面状态。
use super::release_notes::ReleaseNotes;
use crate::{
    components::icons::Icon,
    i18n::{texts, AppText},
};
use dioxus::prelude::*;
use kt_config::AppLanguage;
use kt_update::{
    client::Available,
    metadata::{BuildInfo, Channel},
    Phase, Preferences, Snapshot,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UpdateAction {
    Check,
    Download,
    Cancel,
    Restart,
}

pub(super) fn primary_action(phase: Phase, available: bool) -> Option<UpdateAction> {
    match phase {
        Phase::Disabled | Phase::Checking | Phase::Downloading | Phase::Preparing => None,
        Phase::Ready => Some(UpdateAction::Restart),
        Phase::Available | Phase::Error if available => Some(UpdateAction::Download),
        _ => Some(UpdateAction::Check),
    }
}

pub(super) fn capacity(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}
pub(super) fn percentage(received: u64, total: u64) -> u64 {
    if total == 0 {
        0
    } else {
        ((received.min(total) as u128 * 100) / total as u128) as u64
    }
}
fn last_checked(seconds: Option<u64>) -> Option<String> {
    let seconds = i64::try_from(seconds?).ok()?;
    chrono::DateTime::from_timestamp(seconds, 0).map(|date| {
        date.with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    })
}
fn published(value: Option<&str>) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(value?)
        .ok()
        .map(|date| {
            date.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
}
fn release_url(available: Option<&Available>) -> String {
    available
        .map(|release| {
            let tag: String =
                url::form_urlencoded::byte_serialize(release.tag.as_bytes()).collect();
            format!("{}/tag/{tag}", kt_update::client::RELEASE_PAGE)
        })
        .unwrap_or_else(|| kt_update::client::RELEASE_PAGE.into())
}
fn channel_label(channel: Channel, t: &AppText) -> &'static str {
    match channel {
        Channel::Stable => t.update_stable,
        Channel::Alpha => "Alpha",
        Channel::Development => t.update_development_title,
    }
}
fn presentation(
    snapshot: &Snapshot,
    t: &AppText,
) -> (&'static str, &'static str, &'static str, &'static str) {
    match snapshot.phase {
        Phase::Disabled => (
            "neutral",
            "terminal",
            t.update_development_title,
            t.update_development,
        ),
        Phase::Idle if snapshot.last_check.is_some() => {
            ("success", "check", t.update_latest, t.update_latest_hint)
        }
        Phase::Idle => ("neutral", "sync", t.update_not_checked, t.update_hint),
        Phase::Checking => ("busy", "sync", t.update_checking, t.update_checking_hint),
        Phase::Cancelled => (
            "neutral",
            "sync",
            t.update_cancelled,
            t.update_cancelled_hint,
        ),
        Phase::Available => (
            "available",
            "download",
            t.update_available,
            t.update_available_hint,
        ),
        Phase::Downloading => (
            "busy",
            "download",
            t.update_downloading,
            t.update_download_hint,
        ),
        Phase::Ready => ("success", "check", t.update_ready, t.update_ready_hint),
        Phase::Preparing => ("busy", "sync", t.update_preparing, t.update_preparing_hint),
        Phase::Error => ("error", "close", t.update_failed_title, t.update_failed),
    }
}

#[component]
pub(super) fn UpdateView(
    language: AppLanguage,
    snapshot: Snapshot,
    current: BuildInfo,
    error: Option<String>,
    restarting: bool,
    on_action: EventHandler<UpdateAction>,
    on_preferences: EventHandler<Preferences>,
) -> Element {
    let t = texts(language).app;
    let phase = if restarting {
        Phase::Preparing
    } else {
        snapshot.phase
    };
    let display = Snapshot {
        phase,
        ..snapshot.clone()
    };
    let (tone, icon, title, hint) = presentation(&display, &t);
    let available = snapshot.available.as_ref();
    let target = available.map(|release| release.info.label());
    let current_label = current.label();
    let last = last_checked(snapshot.last_check).unwrap_or_else(|| t.update_never.into());
    let primary = primary_action(phase, available.is_some());
    let prefs_disabled = phase == Phase::Disabled || phase.busy();
    let preferences = snapshot.preferences;
    let release_href = release_url(available);
    rsx! {
        section { class: "software-update-section", aria_label: t.update_title,
            header { class: "update-page-heading",
                h3 { "{t.update_title}" }
                p { "{t.update_hint}" }
            }
            section { class: "update-card update-status-card is-{tone}",
                div { class: "update-status-heading",
                    div { class: if phase == Phase::Checking || phase == Phase::Preparing { "update-state-icon is-spinning" } else { "update-state-icon" }, aria_hidden: "true", Icon { name: icon } }
                    div { class: "update-status-copy",
                        h4 { role: "status", aria_live: "polite", aria_atomic: "true", "{title}" }
                        p { "{hint}" }
                    }
                }
                div { class: "update-version-line",
                    span { class: "update-channel", "{channel_label(current.channel, &t)}" }
                    span { class: "update-current-version", "{t.update_current} · {current_label}" }
                }
                if let Some(target) = target {
                    div { class: "update-target-line",
                        span { "{t.update_target}" }
                        strong { "{target}" }
                        if let Some(release) = available { span { class: "update-package-size", "{capacity(release.package.size)}" } }
                    }
                }
                if phase == Phase::Checking {
                    div { class: "update-indeterminate", role: "progressbar", aria_label: t.update_checking, span {} }
                }
                if phase == Phase::Downloading {
                    if let Some(release) = available {
                        DownloadProgress { received: snapshot.received, total: release.package.size, language }
                    }
                }
                if matches!(phase, Phase::Ready | Phase::Preparing) {
                    InstallSteps { preparing: phase == Phase::Preparing, language }
                }
                if let Some(error) = error {
                    UpdateError { message: error, language }
                }
                div { class: "update-card-footer",
                    span { class: "update-last-check", "{t.update_last_check} · {last}" }
                    div { class: "software-update-actions",
                        if matches!(phase, Phase::Checking | Phase::Downloading) {
                            button { class: "settings-button update-button", r#type: "button", onclick: move |_| on_action.call(UpdateAction::Cancel), "{texts(language).dialog.cancel}" }
                        }
                        if let Some(action) = primary {
                            button {
                                class: if phase == Phase::Idle && snapshot.last_check.is_some() { "settings-button update-button" } else { "settings-button primary update-button" },
                                r#type: "button",
                                onclick: move |_| on_action.call(action),
                                Icon { name: match action { UpdateAction::Download => "download", UpdateAction::Restart => "refresh", _ => "sync" } }
                                {match action {
                                    UpdateAction::Restart => t.update_restart,
                                    UpdateAction::Download if phase == Phase::Error => t.update_retry_download,
                                    UpdateAction::Download => t.update_download,
                                    _ if phase == Phase::Error => t.update_retry,
                                    _ if snapshot.last_check.is_some() || phase == Phase::Cancelled => t.update_recheck,
                                    _ => t.update_check,
                                }}
                            }
                        }
                    }
                }
            }
            if let Some(release) = available {
                section { class: "update-card update-notes-card", aria_label: t.update_notes,
                    div { class: "update-card-heading",
                        h4 { Icon { name: "list" } "{t.update_notes}" }
                        if let Some(date) = published(release.published_at.as_deref()) { span { "{t.update_published} {date}" } }
                    }
                    ReleaseNotes { key: "{release.release_id}-{release.info.build}-{release.info.attempt}", body: release.notes.clone(), truncated: release.notes_truncated, language }
                    a { class: "update-release-link", href: release_href.clone(), target: "_blank", rel: "noopener noreferrer", "{t.update_release_page}" span { aria_hidden: "true", "↗" } }
                }
            }
            section { class: "update-preferences", aria_label: t.update_preferences,
                h4 { "{t.update_preferences}" }
                div { class: "update-card update-preferences-card",
                    UpdateToggle { id: "update-auto-check", title: t.update_auto_check, hint: t.update_auto_check_hint, checked: preferences.auto_check, disabled: prefs_disabled,
                        on_change: move |checked| on_preferences.call(Preferences { auto_check: checked, ..preferences })
                    }
                    UpdateToggle { id: "update-auto-download", title: t.update_auto_download, hint: t.update_auto_download_hint, checked: preferences.auto_download, disabled: prefs_disabled,
                        on_change: move |checked| on_preferences.call(Preferences { auto_download: checked, ..preferences })
                    }
                }
                p { class: "update-local-hint", "{t.update_local_hint}" }
            }
            if available.is_none() {
                a { class: "update-release-link update-page-link", href: release_href, target: "_blank", rel: "noopener noreferrer", "{t.update_release_page}" span { aria_hidden: "true", "↗" } }
            }
        }
    }
}

#[component]
fn UpdateToggle(
    id: &'static str,
    title: &'static str,
    hint: &'static str,
    checked: bool,
    disabled: bool,
    on_change: EventHandler<bool>,
) -> Element {
    rsx! {
        label { class: "update-preference-row", r#for: id,
            span { class: "update-preference-copy", strong { "{title}" } span { id: "{id}-hint", "{hint}" } }
            input { id, class: "update-toggle", r#type: "checkbox", role: "switch", aria_checked: checked, aria_describedby: "{id}-hint", checked, disabled,
                onchange: move |event| on_change.call(event.checked())
            }
        }
    }
}

#[component]
fn DownloadProgress(received: u64, total: u64, language: AppLanguage) -> Element {
    let t = texts(language).app;
    let percent = percentage(received, total);
    rsx! {
        div { class: "update-download-progress",
            div { class: "update-progress-meta", span { "{capacity(received.min(total))} / {capacity(total)}" } strong { "{percent}%" } }
            progress { max: total.max(1), value: received.min(total), aria_label: t.update_downloading, aria_valuetext: "{percent}%" }
            if received >= total && total > 0 { p { role: "status", "{t.update_verifying}" } }
        }
    }
}

#[component]
pub(super) fn InstallSteps(preparing: bool, language: AppLanguage) -> Element {
    let t = texts(language).app;
    rsx! {
        div { class: "update-install-steps",
            div { class: "is-complete", span { aria_hidden: "true", Icon { name: "check" } } "{t.update_downloaded}" }
            div { class: if preparing { "is-active" } else { "" }, span { aria_hidden: "true", if preparing { span { class: "update-mini-spinner" } } else { "2" } } if preparing { "{t.update_preparing}" } else { "{t.update_install_next}" } }
        }
        if preparing { div { class: "update-indeterminate", role: "progressbar", aria_label: t.update_preparing, span {} } }
    }
}

#[component]
pub(super) fn UpdateError(message: String, language: AppLanguage) -> Element {
    let t = texts(language).app;
    rsx! {
        div { class: "software-update-error",
            p { role: "alert", "{t.update_failed_title}" }
            details { summary { "{t.update_error_details}" } p { "{message}" } }
        }
    }
}

#[component]
pub(super) fn RestartDialog(
    language: AppLanguage,
    target: String,
    preparing: bool,
    on_later: EventHandler<()>,
    on_restart: EventHandler<()>,
) -> Element {
    let t = texts(language).app;
    rsx! {
        div { class: "software-update-overlay",
            section { id: "software-update-dialog", class: "software-update-dialog", role: "dialog", aria_modal: "true", aria_labelledby: "update-dialog-title", aria_describedby: "update-dialog-description", tabindex: "-1", "data-preparing": if preparing { "true" } else { "false" },
                div { class: if preparing { "update-dialog-icon is-spinning" } else { "update-dialog-icon" }, aria_hidden: "true", Icon { name: "refresh" } }
                h3 { id: "update-dialog-title", role: "status", if preparing { "{t.update_preparing}" } else { "{t.update_confirm_title}" } }
                p { class: "update-dialog-target", "KitonyTerms {target}" }
                p { id: "update-dialog-description", if preparing { "{t.update_preparing_hint}" } else { "{t.update_confirm_hint}" } }
                if preparing {
                    InstallSteps { preparing: true, language }
                } else {
                    div { class: "update-restart-warning", Icon { name: "terminal" } p { "{t.update_restart_hint}" } }
                    div { class: "software-update-actions update-dialog-actions",
                        button { id: "update-restart-later", class: "settings-button update-button", r#type: "button", onclick: move |_| on_later.call(()), "{t.update_later}" }
                        button { class: "settings-button primary update-button", r#type: "button", onclick: move |_| on_restart.call(()), Icon { name: "refresh" } "{t.update_restart}" }
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
    fn actions_follow_service_phases() {
        for phase in [
            Phase::Disabled,
            Phase::Checking,
            Phase::Downloading,
            Phase::Preparing,
        ] {
            assert_eq!(primary_action(phase, true), None);
        }
        for phase in [Phase::Idle, Phase::Cancelled, Phase::Error] {
            assert_eq!(primary_action(phase, false), Some(UpdateAction::Check));
        }
        assert_eq!(
            primary_action(Phase::Available, true),
            Some(UpdateAction::Download)
        );
        assert_eq!(
            primary_action(Phase::Error, true),
            Some(UpdateAction::Download)
        );
        assert_eq!(
            primary_action(Phase::Ready, true),
            Some(UpdateAction::Restart)
        );
    }
    #[test]
    fn progress_and_dates_are_bounded() {
        assert_eq!(percentage(1, 0), 0);
        assert_eq!(percentage(25, 100), 25);
        assert_eq!(percentage(u64::MAX, 1), 100);
        assert_eq!(percentage(u64::MAX, u64::MAX), 100);
        assert_eq!(capacity(0), "0 B");
        assert_eq!(capacity(1024), "1.0 KiB");
        assert_eq!(capacity(1024 * 1024), "1.0 MiB");
        assert!(published(Some("not-a-date")).is_none());
        assert!(published(None).is_none());
        assert!(published(Some("2026-10-16T01:00:00Z")).is_some());
        assert!(last_checked(Some(u64::MAX)).is_none());
    }
}
