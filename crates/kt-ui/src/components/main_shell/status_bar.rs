//! 主工作台底部状态栏渲染。

use dioxus::prelude::*;
use kt_config::AppLanguage;

use crate::components::app_logic::ActiveMonitorView;
use crate::components::monitor::MonitorPanel;

pub(super) struct StatusBarArgs {
    pub(super) language: AppLanguage,
    pub(super) active_monitor: Option<ActiveMonitorView>,
}

pub(super) fn render_status_bar(args: StatusBarArgs) -> Element {
    let StatusBarArgs {
        language,
        active_monitor,
    } = args;
    rsx! {
        footer {
            class: "status-bar",
            if let Some(monitor) = active_monitor {
                div {
                    class: "status-monitor",
                    MonitorPanel {
                        key: "monitor-{monitor.session_id.0}",
                        session_id: monitor.session_id,
                        language,
                        compact: true,
                    }
                }
            }
        }
    }
}
