//! 资源监控面板组件。

use dioxus::prelude::*;
use kt_config::AppLanguage;
use kt_core::monitor::MonitorStats;
use kt_core::SessionId;

use crate::components::icons::Icon;
use crate::i18n::texts;

#[component]
pub fn MonitorPanel(session_id: SessionId, language: AppLanguage) -> Element {
    let state = crate::components::app::get_state().clone();
    let t = texts(language).monitor;

    let mut stats = use_signal(|| None::<MonitorStats>);
    let mut loading = use_signal(|| true);
    let mut error_message = use_signal(|| None::<String>);

    use_effect(move || {
        let state = state.clone();
        spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                if let Ok(app_state) = state.lock() {
                    if let Some(sess) = app_state.sessions.get(&session_id) {
                        stats.set(sess.monitor.clone());
                        loading.set(sess.monitor_loading);
                        error_message.set(sess.monitor_error.clone());
                    }
                }
            }
        });
    });

    let waiting_grid_class = if loading() {
        "monitor-grid is-loading"
    } else {
        "monitor-grid"
    };

    rsx! {
        div {
            class: "monitor-panel",

            div {
                class: "monitor-title",
                Icon { name: "chevron-down" }
                "{t.system_monitor}"
                button { class: "icon-button slim", title: "{t.close}", Icon { name: "close" } }
            }

            if let Some(error) = error_message() {
                div {
                    class: "monitor-state-message error",
                    Icon { name: "monitor" }
                    "{t.error_prefix}: {error}"
                }
            } else if let Some(ref s) = stats() {
                div {
                    class: "monitor-grid",

                    MetricCard {
                        icon: "cpu",
                        tone: "blue",
                        label: "CPU",
                        value: format!("{:.1}%", s.cpu_percent),
                        subvalue: format_cores(s.cpu_cores, t.core_unit),
                        percent: clamp_percent(s.cpu_percent),
                        trend: t.trend,
                    }
                    MetricCard {
                        icon: "memory",
                        tone: "amber",
                        label: t.memory,
                        value: format_metric_percent(memory_percent(s.mem_used, s.mem_total)),
                        subvalue: format!("{} / {}", format_bytes(s.mem_used), format_bytes(s.mem_total)),
                        percent: memory_percent(s.mem_used, s.mem_total),
                        trend: t.trend,
                    }
                    MetricCard {
                        icon: "load",
                        tone: "green",
                        label: t.load,
                        value: format!("{:.2}", s.load1),
                        subvalue: format_load_subvalue(s.uptime_secs),
                        percent: load_percent(s.load1, s.cpu_cores),
                        trend: t.trend,
                    }
                    MetricCard {
                        icon: "network",
                        tone: "purple",
                        label: t.network,
                        value: format!("↓ {}", format_rate(s.net_rx_rate)),
                        subvalue: format_network_subvalue(s.net_tx_rate, s.latency_ms, t.latency),
                        percent: network_activity_percent(s.net_rx_rate, s.net_tx_rate),
                        trend: t.trend,
                    }
                }
            } else {
                div {
                    class: "{waiting_grid_class}",
                    MetricCard {
                        icon: "cpu",
                        tone: "blue",
                        label: "CPU",
                        value: "--".to_string(),
                        subvalue: t.waiting.to_string(),
                        percent: 0.0,
                        trend: t.trend,
                    }
                    MetricCard {
                        icon: "memory",
                        tone: "amber",
                        label: t.memory,
                        value: "--".to_string(),
                        subvalue: t.waiting.to_string(),
                        percent: 0.0,
                        trend: t.trend,
                    }
                    MetricCard {
                        icon: "load",
                        tone: "green",
                        label: t.load,
                        value: "--".to_string(),
                        subvalue: t.waiting.to_string(),
                        percent: 0.0,
                        trend: t.trend,
                    }
                    MetricCard {
                        icon: "network",
                        tone: "purple",
                        label: t.network,
                        value: "--".to_string(),
                        subvalue: t.waiting.to_string(),
                        percent: 0.0,
                        trend: t.trend,
                    }
                }
            }
        }
    }
}

#[component]
fn MetricCard(
    icon: &'static str,
    tone: &'static str,
    label: &'static str,
    value: String,
    subvalue: String,
    percent: f32,
    trend: &'static str,
) -> Element {
    let trend_label = format!("{label} {trend}");
    let fill_style = format!("width: {:.0}%;", clamp_percent(percent));

    rsx! {
        div {
            class: "metric-card {tone}",
            span {
                class: "metric-label",
                Icon { name: icon }
                "{label}"
            }
            div {
                class: "metric-value-row",
                strong { "{value}" }
                small { "{subvalue}" }
            }
            div {
                class: "metric-meter",
                aria_label: "{trend_label}",
                div { class: "metric-fill {tone}", style: "{fill_style}" }
            }
        }
    }
}

fn memory_percent(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        clamp_percent((used as f32 / total as f32) * 100.0)
    }
}

fn load_percent(load1: f32, cpu_cores: u32) -> f32 {
    if cpu_cores == 0 {
        0.0
    } else {
        clamp_percent((load1 / cpu_cores as f32) * 100.0)
    }
}

fn network_activity_percent(rx_rate: u64, tx_rate: u64) -> f32 {
    let total = rx_rate.saturating_add(tx_rate);
    if total == 0 {
        return 0.0;
    }

    const LOW: u64 = 64 * 1024;
    const MID: u64 = 1024 * 1024;
    const HIGH: u64 = 10 * 1024 * 1024;
    const CAP: u64 = 100 * 1024 * 1024;

    if total <= LOW {
        interpolate_activity(total, 0, LOW, 0.0, 25.0)
    } else if total <= MID {
        interpolate_activity(total, LOW, MID, 25.0, 25.0)
    } else if total <= HIGH {
        interpolate_activity(total, MID, HIGH, 50.0, 25.0)
    } else if total <= CAP {
        interpolate_activity(total, HIGH, CAP, 75.0, 25.0)
    } else {
        100.0
    }
}

fn interpolate_activity(value: u64, start: u64, end: u64, base: f32, span: f32) -> f32 {
    if end <= start {
        return clamp_percent(base);
    }
    let ratio = (value.saturating_sub(start)) as f32 / (end - start) as f32;
    clamp_percent(base + ratio * span)
}

fn clamp_percent(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 100.0)
    } else {
        0.0
    }
}

fn format_metric_percent(percent: f32) -> String {
    format!("{:.0}%", clamp_percent(percent))
}

fn format_cores(cores: u32, unit: &str) -> String {
    if cores == 0 {
        "--".to_string()
    } else {
        format!("{cores} {unit}")
    }
}

fn format_latency(latency_ms: u64, label: &str) -> String {
    format!("{label} {latency_ms}ms")
}

fn format_load_subvalue(uptime_secs: u64) -> String {
    format_uptime(uptime_secs)
}

fn format_network_subvalue(tx_rate: u64, latency_ms: u64, latency_label: &str) -> String {
    format!(
        "↑ {} · {}",
        format_rate(tx_rate),
        format_latency(latency_ms, latency_label)
    )
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn format_rate(bps: u64) -> String {
    format!("{}/s", format_bytes(bps))
}

fn format_uptime(seconds: u64) -> String {
    let hours = seconds / 3600;
    if hours >= 24 {
        format!("{}d {}h", hours / 24, hours % 24)
    } else {
        format!("{}h", hours)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_uptime_uses_days_after_24_hours() {
        assert_eq!(format_uptime(3600), "1h");
        assert_eq!(format_uptime(25 * 3600), "1d 1h");
    }

    #[test]
    fn format_bytes_keeps_kilobyte_unit_below_one_megabyte() {
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(2 * 1024 * 1024), "2.0 MB");
    }

    #[test]
    fn monitor_percentages_are_clamped() {
        assert_eq!(memory_percent(50, 100), 50.0);
        assert_eq!(memory_percent(150, 100), 100.0);
        assert_eq!(memory_percent(1, 0), 0.0);
        assert_eq!(load_percent(0.5, 1), 50.0);
        assert_eq!(load_percent(3.0, 1), 100.0);
    }

    #[test]
    fn network_activity_uses_throughput_as_meter_percent() {
        assert_eq!(network_activity_percent(0, 0), 0.0);

        let light = network_activity_percent(32 * 1024, 0);
        assert!(light > 0.0 && light < 25.0, "{light}");

        let moderate = network_activity_percent(512 * 1024, 512 * 1024);
        assert!((moderate - 50.0).abs() < 0.01, "{moderate}");

        assert_eq!(network_activity_percent(100 * 1024 * 1024, 0), 100.0);
        assert_eq!(network_activity_percent(u64::MAX, u64::MAX), 100.0);
    }

    #[test]
    fn latency_is_formatted_with_network_subvalue_not_load_subvalue() {
        assert_eq!(format_load_subvalue(25 * 3600), "1d 1h");
        assert_eq!(
            format_network_subvalue(2 * 1024, 37, "延迟"),
            "↑ 2.0 KB/s · 延迟 37ms"
        );
    }

    #[test]
    fn format_cores_uses_real_sample_count() {
        assert_eq!(format_cores(1, "核心"), "1 核心");
        assert_eq!(format_cores(0, "核心"), "--");
    }
}
