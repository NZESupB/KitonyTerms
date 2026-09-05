//! 远程运维中心的公共视图外壳。
//!
//! 这里不向宿主交互终端注入只读查询命令：指标数据仍由 core 中独立的 Monitor 通道采集。
//! 服务、进程、网络和 Docker 查询通过类型化 Operations 协议进入同一抽屉；容器终端则使用
//! 独立 SSH PTY 与独立的终端状态。

use std::{
    cell::Cell,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

use dioxus::prelude::*;
use kt_config::{AppLanguage, AppSettings};
use kt_core::monitor::MonitorStats;
use kt_core::{OperationsDomain, OperationsResult, PtySize, SessionId};

use crate::components::icons::Icon;
use crate::components::main_shell::SplitMode;
use crate::components::metrics_format::{format_bytes, format_rate, format_uptime, percent};
use crate::components::terminal::{SnapshotWrapper, Terminal};
use crate::i18n::{
    operations_cpu_summary, operations_error_message, operations_result_count_message,
    operations_updated_message, texts,
};
use crate::state::{AppState, ContainerTerminalState, OperationsViewState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OperationsTool {
    Docker,
    Services,
    Processes,
    Network,
    Metrics,
}

impl OperationsTool {
    const ALL: [Self; 5] = [
        Self::Docker,
        Self::Services,
        Self::Processes,
        Self::Network,
        Self::Metrics,
    ];

    fn icon(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Services => "services",
            Self::Processes => "processes",
            Self::Network => "network",
            Self::Metrics => "monitor",
        }
    }

    fn label(self, language: AppLanguage) -> &'static str {
        let t = texts(language).operations;
        match self {
            Self::Docker => t.tool_docker,
            Self::Services => t.tool_services,
            Self::Processes => t.tool_processes,
            Self::Network => t.tool_network,
            Self::Metrics => t.tool_metrics,
        }
    }

    fn domain(self) -> Option<OperationsDomain> {
        match self {
            Self::Docker => Some(OperationsDomain::Docker),
            Self::Services => Some(OperationsDomain::Services),
            Self::Processes => Some(OperationsDomain::Processes),
            Self::Network => Some(OperationsDomain::Network),
            Self::Metrics => None,
        }
    }

    fn refresh_interval(self) -> Duration {
        match self {
            Self::Processes | Self::Network => Duration::from_secs(3),
            Self::Docker | Self::Services => Duration::from_secs(10),
            Self::Metrics => Duration::from_secs(2),
        }
    }
}

fn request_refresh(
    state: &Arc<Mutex<AppState>>,
    session_id: Option<SessionId>,
    tool: OperationsTool,
) {
    let (Some(session_id), Some(domain)) = (session_id, tool.domain()) else {
        return;
    };
    if let Ok(mut app_state) = state.lock() {
        let already_loading = app_state
            .sessions
            .get(&session_id)
            .and_then(|session| session.operations.get(&domain))
            .is_some_and(|view| view.loading);
        if !already_loading {
            let _ = app_state.refresh_operations(session_id, domain);
        }
    }
}

#[component]
pub fn OperationsPanel(
    session_id: Option<SessionId>,
    connected: bool,
    language: AppLanguage,
    mobile: bool,
    settings: Signal<AppSettings>,
) -> Element {
    let state = crate::components::app::get_state().clone();
    let mut tool = use_signal(|| OperationsTool::Metrics);
    let mut open = use_signal(|| mobile);
    let mut stats = use_signal(|| None::<MonitorStats>);
    let mut loading = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut operations = use_signal(|| None::<OperationsViewState>);
    let mut container_terminal = use_signal(|| None::<ContainerTerminalState>);
    let container_split_mode = use_signal(|| None::<SplitMode>);
    let poll_generation = use_hook(|| Rc::new(Cell::new(0_u64)));

    let state_for_effect = state.clone();
    use_effect(use_reactive((&session_id,), move |(session_id,)| {
        let generation = poll_generation.get().wrapping_add(1);
        poll_generation.set(generation);
        let state = state_for_effect.clone();
        let poll_generation = poll_generation.clone();
        spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                if poll_generation.get() != generation {
                    break;
                }
                let active_tool = tool();
                let panel_visible = mobile || open();
                if let (Some(id), Ok(mut app_state)) = (session_id, state.lock()) {
                    if panel_visible {
                        let (next_stats, next_loading, next_error) = app_state
                            .sessions
                            .get(&id)
                            .map(|session| {
                                (
                                    session.monitor.clone(),
                                    session.monitor_loading,
                                    session.monitor_error.clone(),
                                )
                            })
                            .unwrap_or((None, false, None));
                        if stats.peek().as_ref() != next_stats.as_ref() {
                            stats.set(next_stats);
                        }
                        if *loading.peek() != next_loading {
                            loading.set(next_loading);
                        }
                        if error.peek().as_ref() != next_error.as_ref() {
                            error.set(next_error);
                        }

                        if let Some(domain) = active_tool.domain() {
                            let should_refresh = should_refresh_operations(
                                app_state
                                    .sessions
                                    .get(&id)
                                    .and_then(|session| session.operations.get(&domain)),
                                active_tool.refresh_interval(),
                            );
                            if should_refresh {
                                let _ = app_state.refresh_operations(id, domain);
                            }
                        }
                        let next_operations = active_tool.domain().and_then(|domain| {
                            app_state
                                .sessions
                                .get(&id)
                                .and_then(|session| session.operations.get(&domain))
                                .cloned()
                        });
                        if operations.peek().as_ref() != next_operations.as_ref() {
                            operations.set(next_operations);
                        }
                        let next_container = app_state
                            .sessions
                            .get(&id)
                            .and_then(|session| session.container_terminal.clone());
                        if container_terminal.peek().as_ref() != next_container.as_ref() {
                            container_terminal.set(next_container);
                        }
                    }
                } else {
                    if panel_visible {
                        if stats.peek().is_some() {
                            stats.set(None);
                        }
                        if *loading.peek() {
                            loading.set(false);
                        }
                        if error.peek().is_some() {
                            error.set(None);
                        }
                        if operations.peek().is_some() {
                            operations.set(None);
                        }
                        if container_terminal.peek().is_some() {
                            container_terminal.set(None);
                        }
                    }
                }
            }
        });
    }));

    let rail_class = if mobile {
        "operations-mobile-tools"
    } else {
        "operations-rail"
    };
    let drawer_class = if mobile {
        "operations-mobile-content"
    } else if open() {
        "operations-drawer is-open"
    } else {
        "operations-drawer"
    };
    let can_use = session_id.is_some() && connected;
    let active_tool = tool();
    let refresh_state = state.clone();

    rsx! {
        section { class: if mobile { "operations-mobile" } else { "operations-dock" },
            div { class: "{rail_class}",
                for item in OperationsTool::ALL {
                    button {
                        class: if tool() == item { "operations-tool-button is-active tooltip-trigger" } else { "operations-tool-button tooltip-trigger" },
                        "data-tooltip": "{item.label(language)}",
                        aria_label: "{item.label(language)}",
                        disabled: !can_use,
                        onclick: {
                            let action_state = state.clone();
                            move |_| {
                                if !mobile && open() && tool() == item {
                                    open.set(false);
                                } else {
                                    tool.set(item);
                                    open.set(true);
                                    request_refresh(&action_state, session_id, item);
                                }
                            }
                        },
                        Icon { name: item.icon() }
                    }
                }
            }

            if mobile || open() {
                aside { class: "{drawer_class}",
                    header { class: "operations-drawer-header",
                        div {
                            Icon { name: tool().icon() }
                            h2 { "{tool().label(language)}" }
                        }
                        div { class: "operations-header-actions",
                            if active_tool.domain().is_some() {
                                button {
                                    class: "operations-refresh tooltip-trigger",
                                    "data-tooltip": "{texts(language).operations.refresh}",
                                    aria_label: "{texts(language).operations.refresh}",
                                    disabled: !can_use || operations().as_ref().is_some_and(|view| view.loading),
                                    onclick: move |_| request_refresh(&refresh_state, session_id, active_tool),
                                    Icon { name: "refresh" }
                                }
                            }
                            if !mobile {
                                button {
                                    class: "operations-close tooltip-trigger",
                                    "data-tooltip": "{texts(language).operations.close}",
                                    aria_label: "{texts(language).operations.close}",
                                    onclick: move |_| open.set(false),
                                    Icon { name: "close" }
                                }
                            }
                        }
                    }
                    div { class: "operations-drawer-body",
                        if !can_use {
                            div { class: "operations-state",
                                Icon { name: "terminal" }
                                p { "{texts(language).phone.monitor_need_session}" }
                            }
                        } else if tool() == OperationsTool::Metrics {
                            MetricsDetail {
                                stats: stats(),
                                loading: loading(),
                                error: error(),
                                language,
                            }
                        } else {
                            if active_tool == OperationsTool::Docker {
                                if let Some(terminal) = container_terminal() {
                                    ContainerTerminalPanel {
                                        session_id: session_id.unwrap_or(SessionId(0)),
                                        terminal,
                                        settings,
                                        language,
                                        split_mode: container_split_mode,
                                    }
                                }
                            }
                            OperationsDetail {
                                tool: active_tool,
                                view: operations(),
                                session_id,
                                language,
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn OperationsDetail(
    tool: OperationsTool,
    view: Option<OperationsViewState>,
    session_id: Option<SessionId>,
    language: AppLanguage,
) -> Element {
    let t = texts(language).operations;
    let state = crate::components::app::get_state().clone();
    let loading = view.as_ref().is_some_and(|value| value.loading);
    let error = view
        .as_ref()
        .and_then(|value| value.error.as_ref())
        .map(|value| operations_error_message(language, value.kind));
    let updated = view
        .as_ref()
        .and_then(|value| value.updated_at)
        .map(|updated| operations_updated_message(language, updated.elapsed().as_secs()))
        .unwrap_or_else(|| t.no_successful_snapshot.to_string());

    rsx! {
        if let Some(error) = error {
            div { class: "operations-error", "{error}" }
        }

        if let Some(result) = view.as_ref().and_then(|value| value.result.as_ref()) {
            div { class: "operations-result-meta",
                span { "{operations_result_count_message(language, result_count(result))}" }
                span { "{updated}" }
                if loading { span { "{t.refreshing}" } }
            }
            match result {
                OperationsResult::Services(rows) => rsx! {
                    div { class: "operations-list operations-resource-list",
                        for service in rows.iter() {
                            div { class: "operations-resource-row",
                                div { class: "operations-resource-primary",
                                    strong { "{service.name}" }
                                    small { "{service.description}" }
                                }
                                div { class: "operations-resource-meta",
                                    span { class: "operations-status", "{service.active_state}" }
                                    span { "{service.sub_state}" }
                                    span { "{service.load_state}" }
                                }
                            }
                        }
                    }
                },
                OperationsResult::Processes(rows) => rsx! {
                    div { class: "operations-list operations-resource-list",
                        for process in rows.iter() {
                            div { class: "operations-resource-row",
                                div { class: "operations-resource-primary",
                                    strong { "{process.command}" }
                                    small { "{t.process_id} {process.pid} · {t.parent_process_id} {process.ppid} · {t.user_id} {process.uid} · {process.elapsed}" }
                                }
                                div { class: "operations-resource-meta",
                                    span { "{t.cpu} {process.cpu_percent:.1}%" }
                                    span { "{t.memory} {process.memory_percent:.1}%" }
                                }
                            }
                        }
                    }
                },
                OperationsResult::NetworkConnections(rows) => rsx! {
                    div { class: "operations-list operations-resource-list",
                        for connection in rows.iter() {
                            div { class: "operations-resource-row",
                                div { class: "operations-resource-primary",
                                    strong { "{connection.protocol} · {connection.state}" }
                                    small { "{connection.local} -> {connection.peer}" }
                                    if let Some(owner) = &connection.owner {
                                        small { "{owner}" }
                                    } else {
                                        small { "{t.process_owner_unavailable}" }
                                    }
                                }
                            }
                        }
                    }
                },
                OperationsResult::DockerContainers(rows) => rsx! {
                    div { class: "operations-list operations-resource-list",
                        for container in rows.iter() {
                            div { class: "operations-resource-row",
                                div { class: "operations-resource-primary",
                                    strong { "{container.name}" }
                                    small { "{container.image.clone().unwrap_or_else(|| t.unknown_image.to_string())}" }
                                    small { "{container.id}" }
                                }
                                if let Some(session_id) = session_id {
                                    {
                                        let state = state.clone();
                                        let container_id = container.id.clone();
                                        rsx! {
                                            button {
                                                class: "operations-container-open tooltip-trigger",
                                                "data-tooltip": "{t.container_open}",
                                                aria_label: "{t.container_open}",
                                                onclick: move |_| {
                                                    if let Ok(mut app_state) = state.lock() {
                                                        let _ = app_state.open_container_terminal(
                                                            session_id,
                                                            container_id.clone(),
                                                            PtySize::default(),
                                                        );
                                                    }
                                                },
                                                Icon { name: "terminal" }
                                            }
                                        }
                                    }
                                }
                                if let Some(status) = &container.status {
                                    div { class: "operations-resource-meta", span { class: "operations-status", "{status}" } }
                                }
                            }
                        }
                    }
                },
            }
        } else if loading {
            div { class: "operations-state",
                Icon { name: tool.icon() }
                p { "{t.loading_title}" }
                small { "{t.loading_hint}" }
            }
        } else {
            div { class: "operations-state",
                Icon { name: tool.icon() }
                p { "{t.empty_title}" }
                small { "{t.empty_hint}" }
            }
        }
    }
}

fn result_count(result: &OperationsResult) -> usize {
    match result {
        OperationsResult::Services(rows) => rows.len(),
        OperationsResult::Processes(rows) => rows.len(),
        OperationsResult::NetworkConnections(rows) => rows.len(),
        OperationsResult::DockerContainers(rows) => rows.len(),
    }
}

#[component]
fn ContainerTerminalPanel(
    session_id: SessionId,
    terminal: ContainerTerminalState,
    settings: Signal<AppSettings>,
    language: AppLanguage,
    split_mode: Signal<Option<SplitMode>>,
) -> Element {
    let t = texts(language).operations;
    let exec_id = terminal.exec_id;
    let terminal_id = format!("container-{}", exec_id.0);
    let snapshot = terminal.snapshot.clone();
    let settings_value = settings();
    let close_state = crate::components::app::get_state().clone();

    rsx! {
        section { class: "operations-container-terminal",
            header { class: "operations-container-terminal-header",
                div {
                    Icon { name: "terminal" }
                    strong { "{terminal.container_id}" }
                }
                button {
                    class: "operations-close tooltip-trigger",
                    "data-tooltip": "{t.container_close}",
                    aria_label: "{t.container_close}",
                    onclick: move |_| {
                        if let Ok(mut app_state) = close_state.lock() {
                            let _ = app_state.close_container_terminal(session_id, exec_id);
                        }
                    },
                    Icon { name: "close" }
                }
            }
            if terminal.error.is_some() {
                div { class: "operations-error", "{t.container_error}" }
            }
            if terminal.closed {
                div { class: "operations-state",
                    Icon { name: "terminal" }
                    p { "{t.container_closed}" }
                }
            } else if let Some(snapshot) = snapshot {
                div { class: "operations-container-terminal-surface",
                    Terminal {
                        snapshot: SnapshotWrapper(snapshot),
                        session_id,
                        pane_id: terminal_id,
                        trigger_highlights: settings_value.trigger_highlights,
                        show_line_numbers: settings_value.show_line_numbers,
                        show_timestamps: settings_value.show_timestamps,
                        language,
                        split_mode,
                        exec_id: Some(exec_id),
                        allow_split: false,
                    }
                }
            } else if terminal.loading {
                div { class: "operations-state",
                    Icon { name: "terminal" }
                    p { "{t.container_starting}" }
                    small { "{t.container_terminal_hint}" }
                }
            } else {
                div { class: "operations-state",
                    Icon { name: "terminal" }
                    p { "{t.container_input_unavailable}" }
                }
            }
        }
    }
}

#[component]
fn MetricsDetail(
    stats: Option<MonitorStats>,
    loading: bool,
    error: Option<String>,
    language: AppLanguage,
) -> Element {
    let t = texts(language).operations;
    let mut show_virtual = use_signal(|| false);
    if let Some(stats) = stats {
        let cpu_summary = operations_cpu_summary(language, stats.cpu_percent, stats.cpu_cores);
        rsx! {
            if error.is_some() {
                div { class: "operations-error", "{t.metrics_error}" }
            }
            div { class: "operations-section",
                h3 { "{t.system}" }
                div { class: "operations-kv-grid",
                    MetricValue { label: t.host, value: display_or_dash(&stats.system.hostname) }
                    MetricValue { label: t.distribution, value: stats.system.distro.clone().unwrap_or_else(|| "--".to_string()) }
                    MetricValue { label: t.architecture, value: stats.system.architecture.clone().unwrap_or_else(|| "--".to_string()) }
                    MetricValue { label: t.uptime, value: format_uptime(stats.uptime_secs) }
                }
            }
            div { class: "operations-section",
                h3 { "{t.cpu}" }
                p { class: "operations-summary", "{cpu_summary}" }
                div { class: "operations-core-grid",
                    for core in stats.cpu_per_core {
                        span { class: "operations-core", "CPU{core.id} {core.percent}%" }
                    }
                }
            }
            div { class: "operations-section",
                h3 { "{t.load}" }
                div { class: "operations-kv-grid",
                    MetricValue { label: t.one_minute, value: format!("{:.2}", stats.load1) }
                    MetricValue { label: t.uptime, value: format_uptime(stats.uptime_secs) }
                }
            }
            div { class: "operations-section",
                h3 { "{t.memory}" }
                div { class: "operations-kv-grid",
                    MetricValue { label: t.ram, value: format!("{} / {}", format_bytes(stats.mem_used), format_bytes(stats.mem_total)) }
                    MetricValue { label: t.swap, value: format!("{} / {}", format_bytes(stats.swap_used), format_bytes(stats.swap_total)) }
                }
            }
            div { class: "operations-section",
                div { class: "operations-section-title",
                    h3 { "{t.network}" }
                    label { class: "operations-check",
                        input {
                            r#type: "checkbox",
                            checked: show_virtual(),
                            onchange: move |event| show_virtual.set(event.checked()),
                        }
                        "{t.show_virtual_interfaces}"
                    }
                }
                div { class: "operations-list",
                    for interface in stats.interfaces.into_iter().filter(|interface| show_virtual() || !interface.is_virtual) {
                        div { class: "operations-list-row",
                            strong { "{interface.name}" }
                            span { "↓ {format_rate(interface.rx_rate)}" }
                            span { "↑ {format_rate(interface.tx_rate)}" }
                        }
                    }
                }
            }
            div { class: "operations-section",
                h3 { "{t.disk}" }
                div { class: "operations-list",
                    for disk in sorted_disks(stats.disks) {
                        div { class: "operations-list-row",
                            strong { "{disk.mount}" }
                            span { "{format_bytes(disk.used)} / {format_bytes(disk.total)}" }
                            span { "{percent(disk.used, disk.total):.0}%" }
                        }
                    }
                }
            }
        }
    } else if loading {
        rsx! {
            div { class: "operations-state", p { "{t.metrics_loading}" } }
        }
    } else {
        rsx! {
            if error.is_some() {
                div { class: "operations-error", "{t.metrics_error}" }
            }
            div { class: "operations-state", p { "{t.metrics_snapshot_unavailable}" } }
        }
    }
}

#[component]
fn MetricValue(label: &'static str, value: String) -> Element {
    rsx! { div { class: "operations-kv", small { "{label}" } strong { "{value}" } } }
}

fn sorted_disks(mut disks: Vec<kt_core::monitor::DiskUsage>) -> Vec<kt_core::monitor::DiskUsage> {
    disks.sort_by(|left, right| {
        let left_root = left.mount == "/";
        let right_root = right.mount == "/";
        right_root
            .cmp(&left_root)
            .then_with(|| left.mount.cmp(&right.mount))
    });
    disks
}

fn display_or_dash(value: &str) -> String {
    if value.is_empty() {
        "--".to_string()
    } else {
        value.to_string()
    }
}

fn should_refresh_operations(view: Option<&OperationsViewState>, interval: Duration) -> bool {
    match view {
        None => true,
        Some(view) if view.loading || view.error.is_some() => false,
        Some(view) => view
            .updated_at
            .is_none_or(|updated| updated.elapsed() >= interval),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kt_core::OperationsError;
    use std::time::Instant;

    fn view() -> OperationsViewState {
        OperationsViewState {
            request_id: Some(kt_core::OperationId(1)),
            loading: false,
            error: None,
            result: None,
            requested_at: None,
            updated_at: Some(Instant::now()),
        }
    }

    #[test]
    fn automatic_refresh_only_runs_for_missing_or_stale_successful_views() {
        let interval = Duration::from_secs(10);
        assert!(should_refresh_operations(None, interval));
        assert!(!should_refresh_operations(Some(&view()), interval));

        let mut loading = view();
        loading.loading = true;
        assert!(!should_refresh_operations(Some(&loading), interval));

        let mut failed = view();
        failed.error = Some(OperationsError::new(
            kt_core::OperationsErrorKind::Timeout,
            "timeout",
        ));
        failed.updated_at = None;
        assert!(!should_refresh_operations(Some(&failed), interval));

        let mut stale = view();
        stale.updated_at = Some(Instant::now() - Duration::from_secs(11));
        assert!(should_refresh_operations(Some(&stale), interval));
    }
}
