//! 资源监控面板组件。

use dioxus::prelude::*;
use kt_config::AppLanguage;
use kt_core::monitor::MonitorStats;
use kt_core::{SessionId, ToCore};

use crate::components::icons::Icon;
use crate::i18n::texts;

#[component]
pub fn MonitorPanel(session_id: SessionId, language: AppLanguage) -> Element {
    let state = crate::components::app::get_state().clone();
    let state_for_start = state.clone();
    let t = texts(language).monitor;

    let mut stats = use_signal(|| None::<MonitorStats>);

    use_effect(move || {
        if let Ok(app_state) = state_for_start.lock() {
            app_state
                .manager
                .send(ToCore::StartMonitor { id: session_id });
        }
    });

    use_effect(move || {
        let state = state.clone();
        spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                if let Ok(app_state) = state.lock() {
                    if let Some(sess) = app_state.sessions.get(&session_id) {
                        stats.set(sess.monitor.clone());
                    }
                }
            }
        });
    });

    rsx! {
        div {
            class: "monitor-panel",

            div {
                class: "monitor-title",
                Icon { name: "chevron-down" }
                "{t.system_monitor}"
                button { class: "icon-button slim", title: "{t.close}", Icon { name: "close" } }
            }

            if let Some(ref s) = stats() {
                div {
                    class: "monitor-grid",

                    MetricCard {
                        icon: "cpu",
                        tone: "blue",
                        label: "CPU",
                        value: format!("{:.1}%", s.cpu_percent),
                        subvalue: t.cpu_cores.to_string(),
                        trend: t.trend,
                    }
                    MetricCard {
                        icon: "memory",
                        tone: "amber",
                        label: t.memory,
                        value: if s.mem_total > 0 {
                            format!("{:.0}%", (s.mem_used as f64 / s.mem_total as f64) * 100.0)
                        } else {
                            "--".to_string()
                        },
                        subvalue: format!("{} / {}", format_bytes(s.mem_used), format_bytes(s.mem_total)),
                        trend: t.trend,
                    }
                    MetricCard {
                        icon: "load",
                        tone: "green",
                        label: t.load,
                        value: format!("{:.2}", s.load1),
                        subvalue: format!("uptime {}", format_uptime(s.uptime_secs)),
                        trend: t.trend,
                    }
                    MetricCard {
                        icon: "network",
                        tone: "purple",
                        label: t.network,
                        value: format!("↓ {}", format_rate(s.net_rx_rate)),
                        subvalue: format!("↑ {}", format_rate(s.net_tx_rate)),
                        trend: t.trend,
                    }
                }
            } else {
                div {
                    class: "monitor-grid",
                    MetricCard {
                        icon: "cpu",
                        tone: "blue",
                        label: "CPU",
                        value: "--".to_string(),
                        subvalue: t.waiting.to_string(),
                        trend: t.trend,
                    }
                    MetricCard {
                        icon: "memory",
                        tone: "amber",
                        label: t.memory,
                        value: "--".to_string(),
                        subvalue: t.waiting.to_string(),
                        trend: t.trend,
                    }
                    MetricCard {
                        icon: "load",
                        tone: "green",
                        label: t.load,
                        value: "--".to_string(),
                        subvalue: t.waiting.to_string(),
                        trend: t.trend,
                    }
                    MetricCard {
                        icon: "network",
                        tone: "purple",
                        label: t.network,
                        value: "--".to_string(),
                        subvalue: t.waiting.to_string(),
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
    trend: &'static str,
) -> Element {
    let trend_label = format!("{label} {trend}");

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
            div { class: "sparkline {tone}", aria_label: "{trend_label}" }
        }
    }
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
}
