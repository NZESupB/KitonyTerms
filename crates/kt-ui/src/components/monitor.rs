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
                "{t.system_monitor}"
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
                        label: "CPU".to_string(),
                        value: format!("{:.1}%", s.cpu_percent),
                        subvalue: format_cores(s.cpu_cores, t.core_unit),
                        percent: clamp_percent(s.cpu_percent),
                        state_class: "",
                        stacked_values: false,
                        trend: t.trend,
                    }
                    MetricCard {
                        icon: "memory",
                        tone: "amber",
                        label: t.memory.to_string(),
                        value: format_metric_percent(memory_percent(s.mem_used, s.mem_total)),
                        subvalue: format!("{} / {}", format_bytes(s.mem_used), format_bytes(s.mem_total)),
                        percent: memory_percent(s.mem_used, s.mem_total),
                        state_class: "",
                        stacked_values: false,
                        trend: t.trend,
                    }
                    MetricCard {
                        icon: "disk",
                        tone: "cyan",
                        label: t.disk.to_string(),
                        value: format_metric_percent(root_disk_percent(&s.disks)),
                        subvalue: format_root_disk_usage(&s.disks),
                        percent: root_disk_percent(&s.disks),
                        state_class: "",
                        stacked_values: false,
                        trend: t.trend,
                    }
                    MetricCard {
                        icon: "load",
                        tone: "green",
                        label: t.load.to_string(),
                        value: format!("{:.2}", s.load1),
                        subvalue: format_load_subvalue(s.uptime_secs),
                        percent: load_percent(s.load1, s.cpu_cores),
                        state_class: "",
                        stacked_values: false,
                        trend: t.trend,
                    }
                    MetricCard {
                        icon: "network",
                        tone: "purple",
                        label: format_network_title(t.network, s.latency_ms),
                        value: format!("↓ {}", format_rate(s.net_rx_rate)),
                        subvalue: format_network_subvalue(s.net_tx_rate),
                        percent: network_activity_percent(s.net_rx_rate, s.net_tx_rate),
                        state_class: latency_level(s.latency_ms).class_name(),
                        stacked_values: true,
                        trend: t.trend,
                    }
                }
            } else {
                div {
                    class: "{waiting_grid_class}",
                    MetricCard {
                        icon: "cpu",
                        tone: "blue",
                        label: "CPU".to_string(),
                        value: "--".to_string(),
                        subvalue: t.waiting.to_string(),
                        percent: 0.0,
                        state_class: "",
                        stacked_values: false,
                        trend: t.trend,
                    }
                    MetricCard {
                        icon: "memory",
                        tone: "amber",
                        label: t.memory.to_string(),
                        value: "--".to_string(),
                        subvalue: t.waiting.to_string(),
                        percent: 0.0,
                        state_class: "",
                        stacked_values: false,
                        trend: t.trend,
                    }
                    MetricCard {
                        icon: "disk",
                        tone: "cyan",
                        label: t.disk.to_string(),
                        value: "--".to_string(),
                        subvalue: t.waiting.to_string(),
                        percent: 0.0,
                        state_class: "",
                        stacked_values: false,
                        trend: t.trend,
                    }
                    MetricCard {
                        icon: "load",
                        tone: "green",
                        label: t.load.to_string(),
                        value: "--".to_string(),
                        subvalue: t.waiting.to_string(),
                        percent: 0.0,
                        state_class: "",
                        stacked_values: false,
                        trend: t.trend,
                    }
                    MetricCard {
                        icon: "network",
                        tone: "purple",
                        label: t.network.to_string(),
                        value: "--".to_string(),
                        subvalue: t.waiting.to_string(),
                        percent: 0.0,
                        state_class: "latency-unknown",
                        stacked_values: true,
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
    label: String,
    value: String,
    subvalue: String,
    percent: f32,
    state_class: &'static str,
    stacked_values: bool,
    trend: &'static str,
) -> Element {
    let trend_label = format!("{label} {trend}");
    let fill_style = metric_fill_style(percent);

    rsx! {
        div {
            class: "metric-card {tone} {state_class}",
            style: "{fill_style}",
            span {
                class: "metric-label",
                Icon { name: icon }
                "{label}"
            }
            div {
                class: "{metric_value_row_class(stacked_values)}",
                strong { "{value}" }
                small { "{subvalue}" }
            }
            div {
                class: "metric-meter",
                aria_label: "{trend_label}",
                div { class: "metric-fill {tone}" }
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

fn root_disk(disks: &[kt_core::monitor::DiskUsage]) -> Option<&kt_core::monitor::DiskUsage> {
    disks.iter().find(|disk| disk.mount == "/")
}

fn root_disk_percent(disks: &[kt_core::monitor::DiskUsage]) -> f32 {
    root_disk(disks)
        .map(|disk| memory_percent(disk.used, disk.total))
        .unwrap_or(0.0)
}

fn format_root_disk_usage(disks: &[kt_core::monitor::DiskUsage]) -> String {
    root_disk(disks)
        .map(|disk| format!("{} / {}", format_bytes(disk.used), format_bytes(disk.total)))
        .unwrap_or_else(|| "--".to_string())
}

fn metric_value_row_class(stacked_values: bool) -> &'static str {
    if stacked_values {
        "metric-value-row is-stacked"
    } else {
        "metric-value-row"
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

fn metric_fill_style(percent: f32) -> String {
    format!("--metric-fill-percent: {:.0}%;", clamp_percent(percent))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LatencyLevel {
    Normal,
    Warn,
    Danger,
    Unknown,
}

impl LatencyLevel {
    fn class_name(self) -> &'static str {
        match self {
            Self::Normal => "latency-normal",
            Self::Warn => "latency-warn",
            Self::Danger => "latency-danger",
            Self::Unknown => "latency-unknown",
        }
    }
}

fn latency_level(latency_ms: u64) -> LatencyLevel {
    match latency_ms {
        0 => LatencyLevel::Unknown,
        1..=149 => LatencyLevel::Normal,
        150..=299 => LatencyLevel::Warn,
        _ => LatencyLevel::Danger,
    }
}

fn format_network_title(network_label: &str, latency_ms: u64) -> String {
    if latency_ms == 0 {
        network_label.to_string()
    } else {
        format!("{network_label} {latency_ms}ms")
    }
}

fn format_load_subvalue(uptime_secs: u64) -> String {
    format_uptime(uptime_secs)
}

fn format_network_subvalue(tx_rate: u64) -> String {
    format!("↑ {}", format_rate(tx_rate))
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
    fn root_disk_metric_prefers_root_mount_and_degrades_when_missing() {
        let disks = vec![
            kt_core::monitor::DiskUsage {
                mount: "/data".to_string(),
                used: 9,
                total: 10,
            },
            kt_core::monitor::DiskUsage {
                mount: "/".to_string(),
                used: 40 * 1024 * 1024,
                total: 100 * 1024 * 1024,
            },
        ];

        assert_eq!(root_disk_percent(&disks), 40.0);
        assert_eq!(format_root_disk_usage(&disks), "40.0 MB / 100.0 MB");
        assert_eq!(root_disk_percent(&[]), 0.0);
        assert_eq!(format_root_disk_usage(&[]), "--");
    }

    #[test]
    fn network_values_use_stacked_layout_class() {
        assert_eq!(metric_value_row_class(true), "metric-value-row is-stacked");
        assert_eq!(metric_value_row_class(false), "metric-value-row");
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
    fn network_title_contains_latency_directly() {
        assert_eq!(format_load_subvalue(25 * 3600), "1d 1h");
        assert_eq!(format_network_title("网络", 100), "网络 100ms");
        assert_eq!(format_network_title("Network", 0), "Network");
        assert_eq!(format_network_subvalue(2 * 1024), "↑ 2.0 KB/s");
    }

    #[test]
    fn latency_level_maps_to_display_classes() {
        assert_eq!(latency_level(0), LatencyLevel::Unknown);
        assert_eq!(latency_level(80), LatencyLevel::Normal);
        assert_eq!(latency_level(150), LatencyLevel::Warn);
        assert_eq!(latency_level(300), LatencyLevel::Danger);
        assert_eq!(latency_level(300).class_name(), "latency-danger");
    }

    #[test]
    fn metric_fill_style_uses_card_css_variable() {
        assert_eq!(metric_fill_style(42.4), "--metric-fill-percent: 42%;");
        assert_eq!(metric_fill_style(150.0), "--metric-fill-percent: 100%;");
        assert_eq!(metric_fill_style(f32::NAN), "--metric-fill-percent: 0%;");
    }

    #[test]
    fn format_cores_uses_real_sample_count() {
        assert_eq!(format_cores(1, "核心"), "1 核心");
        assert_eq!(format_cores(0, "核心"), "--");
    }
}
