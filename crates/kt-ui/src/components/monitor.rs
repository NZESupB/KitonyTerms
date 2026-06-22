//! 资源监控面板组件

use dioxus::prelude::*;
use kt_core::{SessionId, ToCore};
use kt_core::monitor::MonitorStats;

#[component]
pub fn MonitorPanel(session_id: SessionId) -> Element {
    let state = crate::components::app::get_state().clone();
    let state_for_start = state.clone();

    // 监控状态（从全局 state 读取，定时刷新）
    let mut stats = use_signal(|| None::<MonitorStats>);

    // 启动监控（只执行一次）
    use_effect(move || {
        if let Ok(app_state) = state_for_start.lock() {
            app_state.manager.send(ToCore::StartMonitor { id: session_id });
        }
    });

    // 定时从全局 state 拉取监控数据
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
            style: "display: flex; flex-direction: column; height: 100%; background: #ffffff; padding: 16px; overflow-y: auto;",

            if let Some(ref s) = stats() {
                // CPU 使用率
                div {
                    style: "margin-bottom: 20px;",
                    h3 {
                        style: "margin: 0 0 8px 0; font-size: 16px; color: #1f2937;",
                        "🖥️ CPU 使用率"
                    }
                    div {
                        style: "background: #f3f4f6; border-radius: 8px; padding: 12px;",
                        div {
                            style: "font-size: 24px; font-weight: 600; color: #2b7de9;",
                            "{s.cpu_percent:.1}%"
                        }
                        // 进度条
                        div {
                            style: "margin-top: 8px; height: 8px; background: #e5e7eb; border-radius: 4px; overflow: hidden;",
                            div {
                                style: "height: 100%; background: linear-gradient(90deg, #10b981, #2b7de9); width: {s.cpu_percent}%;",
                            }
                        }
                    }
                }

                // 内存使用
                div {
                    style: "margin-bottom: 20px;",
                    h3 {
                        style: "margin: 0 0 8px 0; font-size: 16px; color: #1f2937;",
                        "💾 内存使用"
                    }
                    div {
                        style: "background: #f3f4f6; border-radius: 8px; padding: 12px;",
                        div {
                            style: "font-size: 18px; color: #374151;",
                            "{format_bytes(s.mem_used)} / {format_bytes(s.mem_total)}"
                        }
                        div {
                            style: "font-size: 14px; color: #6b7280; margin-top: 4px;",
                            "{((s.mem_used as f64 / s.mem_total as f64) * 100.0):.1}% 已使用"
                        }
                        // 进度条
                        div {
                            style: "margin-top: 8px; height: 8px; background: #e5e7eb; border-radius: 4px; overflow: hidden;",
                            div {
                                style: "height: 100%; background: linear-gradient(90deg, #f59e0b, #ef4444); width: {(s.mem_used as f64 / s.mem_total as f64) * 100.0}%;",
                            }
                        }
                    }
                }

                // 网络流量
                if s.net_rx_rate > 0 || s.net_tx_rate > 0 {
                    div {
                        style: "margin-bottom: 20px;",
                        h3 {
                            style: "margin: 0 0 8px 0; font-size: 16px; color: #1f2937;",
                            "🌐 网络流量"
                        }
                        div {
                            style: "background: #f3f4f6; border-radius: 8px; padding: 12px; display: flex; gap: 16px;",
                            div {
                                style: "flex: 1;",
                                div {
                                    style: "font-size: 12px; color: #6b7280; margin-bottom: 4px;",
                                    "↓ 下载"
                                }
                                div {
                                    style: "font-size: 16px; font-weight: 600; color: #10b981;",
                                    "{format_rate(s.net_rx_rate)}"
                                }
                            }
                            div {
                                style: "flex: 1;",
                                div {
                                    style: "font-size: 12px; color: #6b7280; margin-bottom: 4px;",
                                    "↑ 上传"
                                }
                                div {
                                    style: "font-size: 16px; font-weight: 600; color: #f59e0b;",
                                    "{format_rate(s.net_tx_rate)}"
                                }
                            }
                        }
                    }
                }

                // 磁盘使用
                if !s.disks.is_empty() {
                    div {
                        style: "margin-bottom: 20px;",
                        h3 {
                            style: "margin: 0 0 8px 0; font-size: 16px; color: #1f2937;",
                            "💿 磁盘使用"
                        }
                        div {
                            style: "background: #f3f4f6; border-radius: 8px; padding: 12px; display: flex; flex-direction: column; gap: 12px;",
                            for disk in &s.disks {
                                div {
                                    key: "{disk.mount}",
                                    div {
                                        style: "font-size: 14px; color: #374151; margin-bottom: 4px;",
                                        "{disk.mount}"
                                    }
                                    div {
                                        style: "font-size: 12px; color: #6b7280; margin-bottom: 4px;",
                                        "{format_bytes(disk.used)} / {format_bytes(disk.total)} ({((disk.used as f64 / disk.total as f64) * 100.0):.1}%)"
                                    }
                                    div {
                                        style: "height: 6px; background: #e5e7eb; border-radius: 3px; overflow: hidden;",
                                        div {
                                            style: "height: 100%; background: #6366f1; width: {(disk.used as f64 / disk.total as f64) * 100.0}%;",
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // 进程列表
                if !s.processes.is_empty() {
                    div {
                        h3 {
                            style: "margin: 0 0 8px 0; font-size: 16px; color: #1f2937;",
                            "⚙️ 进程（CPU 排序）"
                        }
                        div {
                            style: "background: #f3f4f6; border-radius: 8px; overflow: hidden;",
                            for (idx, proc) in s.processes.iter().enumerate() {
                                div {
                                    key: "{idx}",
                                    style: "padding: 8px 12px; border-bottom: 1px solid #e5e7eb; display: flex; justify-content: space-between; align-items: center;",
                                    div {
                                        style: "font-size: 13px; color: #374151; font-family: monospace;",
                                        "{proc.name}"
                                    }
                                    div {
                                        style: "display: flex; gap: 12px; font-size: 12px;",
                                        span {
                                            style: "color: #10b981;",
                                            "CPU: {proc.cpu:.1}%"
                                        }
                                        span {
                                            style: "color: #f59e0b;",
                                            "MEM: {proc.mem:.1}%"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                div {
                    style: "padding: 40px; text-align: center; color: #6b7280;",
                    "正在获取资源监控数据..."
                }
            }
        }
    }
}

/// 格式化字节数
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// 格式化速率（字节/秒）
fn format_rate(bps: u64) -> String {
    format!("{}/s", format_bytes(bps))
}
