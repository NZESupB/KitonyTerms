//! 更新展示与服务操作分离，便于使用隔离快照核验所有界面状态。
use dioxus::prelude::*;
use kt_config::AppLanguage;

use super::release_notes::ReleaseNotes;
use crate::{
    components::icons::Icon,
    components::metrics_format::{format_mb, format_mb_rate},
    i18n::{texts, AppText},
};
use kt_update::{
    client::Available,
    metadata::{BuildInfo, Channel},
    proxy::UpdateProxy,
    Phase, Preferences, Snapshot,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UpdateAction {
    Check,
    Download,
    Cancel,
    Restart,
    /// 便携副本位于受保护目录：重启前需要用户确认管理员授权。
    ElevatedRestart,
}

pub(super) fn primary_action(
    phase: Phase,
    available: bool,
    requires_elevation: bool,
) -> Option<UpdateAction> {
    match phase {
        Phase::Disabled | Phase::Checking | Phase::Downloading | Phase::Preparing => None,
        Phase::Ready if requires_elevation => Some(UpdateAction::ElevatedRestart),
        Phase::Ready => Some(UpdateAction::Restart),
        Phase::Available | Phase::Error if available => Some(UpdateAction::Download),
        _ => Some(UpdateAction::Check),
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
    on_proxy: EventHandler<UpdateProxy>,
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
    let primary = primary_action(phase, available.is_some(), snapshot.requires_elevation);
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
                        if let Some(release) = available { span { class: "update-package-size", "{format_mb(release.package.size)}" } }
                    }
                }
                if phase == Phase::Checking {
                    div { class: "update-indeterminate", role: "progressbar", aria_label: t.update_checking, span {} }
                }
                if phase == Phase::Downloading {
                    if let Some(release) = available {
                        DownloadProgress { received: snapshot.received, total: release.package.size, speed: snapshot.speed, language }
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
                                Icon { name: match action { UpdateAction::Download => "download", UpdateAction::Restart | UpdateAction::ElevatedRestart => "refresh", _ => "sync" } }
                                {match action {
                                    UpdateAction::ElevatedRestart => t.update_restart_elevated,
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
                        on_change: { let preferences = preferences.clone(); move |checked| on_preferences.call(Preferences { auto_check: checked, ..preferences.clone() }) }
                    }
                    UpdateToggle { id: "update-auto-download", title: t.update_auto_download, hint: t.update_auto_download_hint, checked: preferences.auto_download, disabled: prefs_disabled,
                        on_change: { let preferences = preferences.clone(); move |checked| on_preferences.call(Preferences { auto_download: checked, ..preferences.clone() }) }
                    }
                }
                p { class: "update-local-hint", "{t.update_local_hint}" }
            }
            section { class: "update-preferences", aria_label: t.update_network,
                h4 { "{t.update_network}" }
                UpdateProxyCard { language, proxy: preferences.proxy.clone(), disabled: prefs_disabled, on_apply: on_proxy }
                p { class: "update-local-hint", "{t.update_network_hint}" }
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

/// 代理模式标识，与 [`UpdateProxy`] 的变体一一对应。
fn proxy_mode(proxy: &UpdateProxy) -> &'static str {
    match proxy {
        UpdateProxy::System => "system",
        UpdateProxy::Direct => "direct",
        UpdateProxy::Http { .. } => "http",
        UpdateProxy::Https { .. } => "https",
        UpdateProxy::Socks5 { .. } => "socks5",
    }
}

/// 取出 host/port/username 文本，供表单回填。
fn proxy_fields(proxy: &UpdateProxy) -> (String, String, String) {
    match proxy {
        UpdateProxy::Http {
            host,
            port,
            username,
        }
        | UpdateProxy::Https {
            host,
            port,
            username,
        }
        | UpdateProxy::Socks5 {
            host,
            port,
            username,
        } => (
            host.clone(),
            port.to_string(),
            username.clone().unwrap_or_default(),
        ),
        UpdateProxy::System | UpdateProxy::Direct => (String::new(), String::new(), String::new()),
    }
}

/// 把表单草稿还原为代理配置；非法输入返回可直接展示的原因，不提交给服务。
fn proxy_draft(mode: &str, host: &str, port: &str, username: &str) -> Result<UpdateProxy, String> {
    let username = username.trim();
    let username = (!username.is_empty()).then(|| username.to_string());
    let proxy = match mode {
        "system" => UpdateProxy::System,
        "direct" => UpdateProxy::Direct,
        kind => {
            let host = host.trim().to_string();
            let port = port.trim().parse::<u16>().map_err(|_| {
                "代理端口必须是 1-65535 的整数 / Proxy port must be 1-65535".to_string()
            })?;
            match kind {
                "http" => UpdateProxy::Http {
                    host,
                    port,
                    username,
                },
                "https" => UpdateProxy::Https {
                    host,
                    port,
                    username,
                },
                _ => UpdateProxy::Socks5 {
                    host,
                    port,
                    username,
                },
            }
        }
    };
    proxy.validate().map_err(|error| format!("{error:#}"))?;
    Ok(proxy)
}

/// 更新通道代理表单：草稿在本地编辑，点「应用代理」才写入并生效。
#[component]
fn UpdateProxyCard(
    language: AppLanguage,
    proxy: UpdateProxy,
    disabled: bool,
    on_apply: EventHandler<UpdateProxy>,
) -> Element {
    let t = texts(language).app;
    let labels = texts(language).dialog;
    let mut mode = use_signal(|| proxy_mode(&proxy).to_string());
    let mut host = use_signal(String::new);
    let mut port = use_signal(String::new);
    let mut username = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    // 已保存的代理变化时重新播种草稿（含启动时非法配置回退），界面始终反映生效值。
    use_effect(use_reactive((&proxy,), move |(proxy,)| {
        let (saved_host, saved_port, saved_username) = proxy_fields(&proxy);
        mode.set(proxy_mode(&proxy).to_string());
        host.set(saved_host);
        port.set(saved_port);
        username.set(saved_username);
        error.set(None);
    }));

    let needs_address = !matches!(mode().as_str(), "system" | "direct");
    let dirty = proxy_draft(&mode(), &host(), &port(), &username())
        .map(|draft| draft != proxy)
        .unwrap_or(true);
    rsx! {
        div { class: "update-card update-proxy-card",
            label { class: "update-proxy-field", r#for: "update-proxy-mode",
                span { "{t.update_proxy_mode}" }
                select {
                    id: "update-proxy-mode",
                    class: "settings-text-input",
                    value: "{mode()}",
                    disabled,
                    onchange: move |event: Event<FormData>| {
                        mode.set(event.value());
                        error.set(None);
                    },
                    option { value: "system", selected: mode() == "system", "{t.update_proxy_system}" }
                    option { value: "direct", selected: mode() == "direct", "{t.update_proxy_direct}" }
                    option { value: "http", selected: mode() == "http", "{t.update_proxy_http}" }
                    option { value: "https", selected: mode() == "https", "{t.update_proxy_https}" }
                    option { value: "socks5", selected: mode() == "socks5", "{t.update_proxy_socks5}" }
                }
            }
            if needs_address {
                div { class: "update-proxy-row",
                    label { class: "update-proxy-field", r#for: "update-proxy-host",
                        span { "{labels.proxy_host_label}" }
                        input {
                            id: "update-proxy-host",
                            class: "settings-text-input",
                            r#type: "text",
                            value: "{host()}",
                            disabled,
                            placeholder: "127.0.0.1",
                            oninput: move |event: Event<FormData>| {
                                host.set(event.value());
                                error.set(None);
                            },
                        }
                    }
                    label { class: "update-proxy-field", r#for: "update-proxy-port",
                        span { "{labels.proxy_port_label}" }
                        input {
                            id: "update-proxy-port",
                            class: "settings-text-input",
                            r#type: "text",
                            value: "{port()}",
                            disabled,
                            placeholder: "8080",
                            oninput: move |event: Event<FormData>| {
                                port.set(event.value());
                                error.set(None);
                            },
                        }
                    }
                }
                label { class: "update-proxy-field", r#for: "update-proxy-username",
                    span { "{labels.proxy_username_label}" }
                    input {
                        id: "update-proxy-username",
                        class: "settings-text-input",
                        r#type: "text",
                        value: "{username()}",
                        disabled,
                        oninput: move |event: Event<FormData>| {
                            username.set(event.value());
                            error.set(None);
                        },
                    }
                }
            }
            p { class: "update-local-hint", "{t.update_proxy_hint}" }
            if let Some(message) = error() {
                p { class: "update-proxy-error", role: "alert", "{message}" }
            }
            div { class: "update-proxy-actions",
                button {
                    class: "settings-button update-button",
                    r#type: "button",
                    disabled: disabled || !dirty,
                    onclick: move |_| {
                        match proxy_draft(&mode(), &host(), &port(), &username()) {
                            Ok(proxy) => {
                                error.set(None);
                                on_apply.call(proxy);
                            }
                            Err(message) => error.set(Some(message)),
                        }
                    },
                    "{t.update_proxy_apply}"
                }
            }
        }
    }
}

#[component]
fn DownloadProgress(received: u64, total: u64, speed: u64, language: AppLanguage) -> Element {
    let t = texts(language).app;
    let percent = percentage(received, total);
    rsx! {
        div { class: "update-download-progress",
            div { class: "update-progress-meta",
                span { "{format_mb(received.min(total))} / {format_mb(total)}" }
                // 速率是采样平滑值；首个采样窗口前为 0，此时不显示以免误导。
                if speed > 0 {
                    span { class: "update-download-speed", title: t.update_speed, "{format_mb_rate(speed)}" }
                }
                strong { "{percent}%" }
            }
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
    /// 替换需要管理员授权：确认前就告知会弹出系统授权对话框。
    elevated: bool,
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
                p { id: "update-dialog-description", if preparing { "{t.update_preparing_hint}" } else if elevated { "{t.update_elevated_hint}" } else { "{t.update_confirm_hint}" } }
                if elevated && !preparing {
                    div { class: "update-restart-warning update-elevation-warning", Icon { name: "shield" } p { "{t.update_elevated_notice}" } }
                }
                if preparing {
                    InstallSteps { preparing: true, language }
                } else {
                    div { class: "update-restart-warning", Icon { name: "terminal" } p { "{t.update_restart_hint}" } }
                    div { class: "software-update-actions update-dialog-actions",
                        button { id: "update-restart-later", class: "settings-button update-button", r#type: "button", onclick: move |_| on_later.call(()), "{t.update_later}" }
                        button { class: "settings-button primary update-button", r#type: "button", onclick: move |_| on_restart.call(()), Icon { name: "refresh" } if elevated { "{t.update_restart_elevated}" } else { "{t.update_restart}" } }
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
            assert_eq!(primary_action(phase, true, false), None);
            assert_eq!(primary_action(phase, true, true), None);
        }
        for phase in [Phase::Idle, Phase::Cancelled, Phase::Error] {
            assert_eq!(
                primary_action(phase, false, false),
                Some(UpdateAction::Check)
            );
        }
        assert_eq!(
            primary_action(Phase::Available, true, false),
            Some(UpdateAction::Download)
        );
        assert_eq!(
            primary_action(Phase::Error, true, false),
            Some(UpdateAction::Download)
        );
        assert_eq!(
            primary_action(Phase::Ready, true, false),
            Some(UpdateAction::Restart)
        );
        // 受保护目录里的便携副本：就绪状态给出提权重启，而不是普通重启。
        assert_eq!(
            primary_action(Phase::Ready, true, true),
            Some(UpdateAction::ElevatedRestart)
        );
    }
    #[test]
    fn progress_and_dates_are_bounded() {
        assert_eq!(percentage(1, 0), 0);
        assert_eq!(percentage(25, 100), 25);
        assert_eq!(percentage(u64::MAX, 1), 100);
        assert_eq!(percentage(u64::MAX, u64::MAX), 100);
        assert!(published(Some("not-a-date")).is_none());
        assert!(published(None).is_none());
        assert!(published(Some("2026-10-16T01:00:00Z")).is_some());
        assert!(last_checked(Some(u64::MAX)).is_none());
    }

    #[test]
    fn proxy_forms_roundtrip_and_reject_invalid_drafts() {
        for proxy in [
            UpdateProxy::System,
            UpdateProxy::Direct,
            UpdateProxy::Http {
                host: "127.0.0.1".into(),
                port: 8080,
                username: Some("me".into()),
            },
            UpdateProxy::Https {
                host: "proxy.local".into(),
                port: 8443,
                username: None,
            },
            UpdateProxy::Socks5 {
                host: "127.0.0.1".into(),
                port: 1080,
                username: None,
            },
        ] {
            let (host, port, username) = proxy_fields(&proxy);
            assert_eq!(
                proxy_draft(proxy_mode(&proxy), &host, &port, &username).unwrap(),
                proxy
            );
        }

        // 端口必须是整数；主机校验交给 kt-update，界面只负责展示原因。
        assert!(proxy_draft("http", "127.0.0.1", "", "").is_err());
        assert!(proxy_draft("http", "127.0.0.1", "abc", "").is_err());
        assert!(proxy_draft("http", "", "8080", "").is_err());
        assert!(proxy_draft("socks5", "127.0.0.1", "0", "").is_err());
        // 用户名留空按未填写处理，与保存值一致。
        assert_eq!(
            proxy_draft("http", "127.0.0.1", "8080", "   ").unwrap(),
            UpdateProxy::Http {
                host: "127.0.0.1".into(),
                port: 8080,
                username: None,
            }
        );
    }
}
