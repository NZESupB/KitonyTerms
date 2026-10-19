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
use kt_core::{
    DockerAction, DockerResource, OperationsDomain, OperationsRequest, OperationsResult,
    ProcessAction, PtySize, ServiceAction, SessionId,
};

use super::docker_usage::{usage_for_container, DockerUsage};
use crate::components::icons::Icon;
use crate::components::main_shell::SplitMode;
use crate::components::metrics_format::{format_bytes, format_rate, format_uptime, percent};
use crate::components::terminal::{SnapshotWrapper, Terminal};
use crate::i18n::{
    operations_cpu_summary, operations_error_message, operations_result_count_message,
    operations_updated_message, texts,
};
use crate::operations_state::{
    DetailTarget, DockerUsageCache, ListContext, OperationKey, OperationsList, OperationsPage,
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
    _docker_resource: DockerResource,
) {
    let (Some(session_id), Some(domain)) = (session_id, tool.domain()) else {
        return;
    };
    if let Ok(mut app_state) = state.lock() {
        app_state.probe_operations(session_id, false);
        if let Some(session) = app_state.sessions.get_mut(&session_id) {
            session
                .operations_ui
                .retry
                .retain(|key, _| key.domain() != domain);
        }
        let compose = app_state.sessions.get(&session_id).is_some_and(|session| {
            session.operations_ui.page(domain).parent()
                == OperationsList::Docker(DockerResource::Compose)
        });
        if probe_allows_refresh(
            app_state
                .sessions
                .get(&session_id)
                .and_then(|s| s.operations.get(&OperationKey::Capabilities)),
            domain,
            compose,
        ) {
            app_state.refresh_operation_page(session_id, domain);
        }
    }
}

fn request_operation(
    state: &Arc<Mutex<AppState>>,
    session_id: Option<SessionId>,
    request: OperationsRequest,
) {
    let Some(session_id) = session_id else {
        return;
    };
    if let Ok(mut app_state) = state.lock() {
        let key = OperationKey::for_request(&request);
        match &key {
            OperationKey::Detail(target) => app_state.navigate_operation(
                session_id,
                OperationsPage::Detail {
                    parent: target.parent(),
                    target: target.clone(),
                },
            ),
            OperationKey::List(list) => {
                app_state.navigate_operation(session_id, OperationsPage::List(*list))
            }
            _ => {}
        }
        if matches!(key, OperationKey::Detail(_) | OperationKey::List(_)) {
            for background in [
                OperationKey::DockerRuntime,
                OperationKey::DockerStorage,
                OperationKey::DockerImageSizes,
            ] {
                app_state.cancel_operation_key(session_id, &background);
            }
        }
        let _ = app_state.request_operation(session_id, request);
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
    let mut capabilities = use_signal(|| None::<OperationsViewState>);
    let mut snapshot_owner = use_signal(|| None::<SessionId>);
    let mut snapshot_tool = use_signal(|| OperationsTool::Metrics);
    let mut container_terminal = use_signal(|| None::<ContainerTerminalState>);
    let mut docker_resource = use_signal(|| DockerResource::Containers);
    let mut page = use_signal(|| None::<OperationsPage>);
    let mut list_context = use_signal(ListContext::default);
    let mut docker_cache = use_signal(DockerUsageCache::default);
    let mut docker_error = use_signal(|| None::<String>);
    let mut action = use_signal(|| None::<OperationsViewState>);
    let container_split_mode = use_signal(|| None::<SplitMode>);
    let poll_generation = use_hook(|| Rc::new(Cell::new(0_u64)));
    let previous_session = use_hook(|| Rc::new(Cell::new(None::<SessionId>)));
    let cleanup_session = previous_session.clone();
    let cleanup_state = state.clone();
    use_drop(move || {
        if let (Some(id), Ok(mut state)) = (cleanup_session.get(), cleanup_state.lock()) {
            state.cancel_operations(id);
        }
    });

    let state_for_effect = state.clone();
    use_effect(use_reactive((&session_id,), move |(session_id,)| {
        if let Some(previous) = previous_session.replace(session_id) {
            if Some(previous) != session_id {
                if let Ok(mut state) = state_for_effect.lock() {
                    state.cancel_operations(previous);
                }
            }
        }
        stats.set(None);
        snapshot_owner.set(None);
        loading.set(false);
        error.set(None);
        operations.set(None);
        capabilities.set(None);
        container_terminal.set(None);
        page.set(None);
        docker_cache.set(DockerUsageCache::default());
        action.set(None);
        docker_error.set(None);
        list_context.set(ListContext::default());
        if let (Some(id), Some(domain), Ok(mut state)) =
            (session_id, tool().domain(), state_for_effect.lock())
        {
            if let Some(session) = state.sessions.get_mut(&id) {
                session
                    .operations_ui
                    .retry
                    .retain(|key, _| key.domain() != domain);
            }
        }
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
                        app_state.probe_operations(id, false);
                        let next_capabilities = app_state
                            .sessions
                            .get(&id)
                            .and_then(|session| session.operations.get(&OperationKey::Capabilities))
                            .cloned();
                        if *capabilities.peek() != next_capabilities {
                            capabilities.set(next_capabilities.clone());
                        }
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
                            let current = app_state
                                .sessions
                                .get(&id)
                                .map(|s| s.operations_ui.page(domain));
                            let compose = current.as_ref().is_some_and(|page| {
                                page.parent() == OperationsList::Docker(DockerResource::Compose)
                            });
                            if probe_allows_refresh(next_capabilities.as_ref(), domain, compose) {
                                app_state.poll_operation_page(
                                    id,
                                    domain,
                                    active_tool.refresh_interval(),
                                );
                            }
                            if *page.peek() != current {
                                page.set(current.clone());
                            }
                            if let Some(session) = app_state.sessions.get(&id) {
                                let context = current
                                    .as_ref()
                                    .and_then(|p| session.operations_ui.contexts.get(&p.parent()))
                                    .cloned()
                                    .unwrap_or_default();
                                if *list_context.peek() != context {
                                    list_context.set(context);
                                }
                                if *docker_cache.peek() != session.operations_ui.docker {
                                    docker_cache.set(session.operations_ui.docker.clone());
                                }
                                let next_action = session
                                    .operations
                                    .get(&OperationKey::Action(domain))
                                    .cloned();
                                if *action.peek() != next_action {
                                    action.set(next_action);
                                }
                                let next_error = [
                                    OperationKey::DockerRuntime,
                                    OperationKey::DockerStorage,
                                    OperationKey::DockerImageSizes,
                                ]
                                .iter()
                                .find_map(|key| {
                                    session
                                        .operations
                                        .get(key)
                                        .and_then(|v| v.error.as_ref())
                                        .map(|e| {
                                            operations_error_message(language, e.kind).to_string()
                                        })
                                });
                                if *docker_error.peek() != next_error {
                                    docker_error.set(next_error);
                                }
                            }
                            if let Some(
                                OperationsPage::List(OperationsList::Docker(resource))
                                | OperationsPage::Detail {
                                    parent: OperationsList::Docker(resource),
                                    ..
                                },
                            ) = current.as_ref()
                            {
                                if *docker_resource.peek() != *resource {
                                    docker_resource.set(*resource);
                                }
                            }
                        }
                        let next_operations = active_tool.domain().and_then(|domain| {
                            app_state
                                .sessions
                                .get(&id)
                                .and_then(|session| {
                                    session
                                        .operations
                                        .get(&session.operations_ui.page(domain).key())
                                })
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
                        if *snapshot_owner.peek() != Some(id) {
                            snapshot_owner.set(Some(id));
                        }
                        if *snapshot_tool.peek() != active_tool {
                            snapshot_tool.set(active_tool);
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
    let tool_is_visible = mobile || open();
    let refresh_state = state.clone();
    let probe_state = state.clone();
    let cancel_state = state.clone();
    let close_state = state.clone();
    let same_session = snapshot_matches_session(snapshot_owner(), session_id);
    let (stats_view, error_view, container_view) = if same_session {
        (stats(), error(), container_terminal())
    } else {
        (None, None, None)
    };
    let capability_view = if same_session { capabilities() } else { None };
    let operation_view = if same_session && snapshot_tool() == active_tool {
        operations()
    } else {
        None
    };
    let probe_loading = capability_view.as_ref().is_some_and(|view| view.loading);
    let pending = probe_loading || operation_view.as_ref().is_some_and(|view| view.loading);

    rsx! {
        section { class: if mobile { "operations-mobile" } else { "operations-dock" },
            div { class: "{rail_class}",
                for item in OperationsTool::ALL {
                    button {
                        class: if tool_button_is_active(can_use, tool_is_visible, active_tool, item) {
                            "operations-tool-button is-active tooltip-trigger"
                        } else {
                            "operations-tool-button tooltip-trigger"
                        },
                        "data-tooltip": "{item.label(language)}",
                        aria_label: "{item.label(language)}",
                        disabled: !can_use,
                        onclick: {
                            let action_state = state.clone();
                            move |_| {
                                if !mobile && open() && tool() == item {
                                    open.set(false);
                                    if let (Some(id), Ok(mut state)) = (session_id, action_state.lock()) {
                                        state.cancel_operations(id);
                                    }
                                } else {
                                    if let (Some(id), Ok(mut state)) = (session_id, action_state.lock()) { state.cancel_operations(id); }
                                    tool.set(item);
                                    open.set(true);
                                    request_refresh(&action_state, session_id, item, docker_resource());
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
                            button {
                                class: "operations-refresh tooltip-trigger",
                                "data-tooltip": "{texts(language).operations.probe}",
                                aria_label: "{texts(language).operations.probe}",
                                disabled: !can_use || probe_loading,
                                onclick: move |_| {
                                    if let (Some(id), Ok(mut state)) = (session_id, probe_state.lock()) {
                                        state.probe_operations(id, true);
                                    }
                                },
                                Icon { name: "search" }
                            }
                            if pending {
                                button {
                                    class: "operations-close tooltip-trigger",
                                    "data-tooltip": "{texts(language).operations.cancel}",
                                    aria_label: "{texts(language).operations.cancel}",
                                    onclick: move |_| {
                                        if let (Some(id), Ok(mut state)) = (session_id, cancel_state.lock()) {
                                            state.cancel_operations(id);
                                            if let Some(session) = state.sessions.get_mut(&id) {
                                                for key in session.operations.keys() { session.operations_ui.retry.entry(key.clone()).or_default().failed(kt_core::OperationsErrorKind::Cancelled); }
                                            }
                                        }
                                    },
                                    Icon { name: "close" }
                                }
                            }
                            if active_tool.domain().is_some() {
                                button {
                                    class: "operations-refresh tooltip-trigger",
                                    "data-tooltip": "{texts(language).operations.refresh}",
                                    aria_label: "{texts(language).operations.refresh}",
                                    disabled: !can_use || pending,
                                    onclick: move |_| {
                                        if let (Some(id), Ok(mut state)) = (session_id, refresh_state.lock()) {
                                            if let Some(domain) = active_tool.domain() { state.refresh_operation_page(id, domain); }
                                        }
                                    },
                                    Icon { name: "refresh" }
                                }
                            }
                            if !mobile {
                                button {
                                    class: "operations-close tooltip-trigger",
                                    "data-tooltip": "{texts(language).operations.close}",
                                    aria_label: "{texts(language).operations.close}",
                                    onclick: move |_| {
                                        open.set(false);
                                        if let (Some(id), Ok(mut state)) = (session_id, close_state.lock()) {
                                            state.cancel_operations(id);
                                        }
                                    },
                                    Icon { name: "close" }
                                }
                            }
                        }
                    }
                    div { class: "operations-drawer-body",
                        CapabilitiesStatus { view: capability_view, language, tool: active_tool, compose: docker_resource() == DockerResource::Compose }
                        if !can_use {
                            div { class: "operations-state",
                                Icon { name: "terminal" }
                                p { "{texts(language).phone.monitor_need_session}" }
                            }
                        }
                        if tool() == OperationsTool::Metrics {
                            MetricsDetail {
                                stats: stats_view,
                                loading: same_session && loading(),
                                error: error_view,
                                language,
                            }
                        } else {
                            if active_tool == OperationsTool::Docker {
                                if let Some(terminal) = container_view {
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
                                view: operation_view,
                                session_id,
                                language,
                                docker_resource,
                                page: page().filter(|_| same_session && snapshot_tool() == active_tool),
                                context: list_context(),
                                docker_cache: docker_cache(),
                                docker_error: docker_error().filter(|_| same_session && snapshot_tool() == active_tool),
                                action: action().filter(|_| same_session && snapshot_tool() == active_tool),
                            }
                        }
                    }
                }
            }
        }
    }
}

fn snapshot_matches_session(owner: Option<SessionId>, active: Option<SessionId>) -> bool {
    active.is_some() && owner == active
}

fn probe_allows_refresh(
    view: Option<&OperationsViewState>,
    domain: OperationsDomain,
    compose: bool,
) -> bool {
    let Some(view) = view.filter(|view| !view.loading && view.error.is_none()) else {
        return false;
    };
    matches!(&view.result, Some(OperationsResult::Capabilities(caps)) if caps.availability(domain, compose) != kt_core::remote_ops::Availability::Unavailable && match domain {
        OperationsDomain::Processes => caps.ps,
        OperationsDomain::Network => caps.ss,
        _ => true,
    })
}

#[component]
fn CapabilitiesStatus(
    view: Option<OperationsViewState>,
    language: AppLanguage,
    tool: OperationsTool,
    compose: bool,
) -> Element {
    use kt_core::remote_ops::Availability;
    let t = texts(language).operations;
    let Some(view) = view else { return rsx! {} };
    let status = view.result.as_ref().and_then(|result| match result {
        OperationsResult::Capabilities(caps) => Some(caps),
        _ => None,
    });
    rsx! {
        div { class: "operations-capabilities",
            if view.loading { p { "{t.probing}" } }
            if let Some(error) = &view.error {
                p { class: "operations-error", "{operations_error_message(language, error.kind)}" }
            }
            if let Some(caps) = status {
                if let Some(domain) = tool.domain() {
                    match caps.availability(domain, compose) {
                        Availability::Unavailable => rsx! { p { class: "operations-error", "{t.unavailable}" } },
                        Availability::Degraded => rsx! { p { "{t.degraded}" } },
                        Availability::Available => rsx! {},
                    }
                }
                details {
                    summary { "Linux / systemd / Docker" }
                    dl {
                        for (name, supported) in [("Linux", caps.linux), ("systemd", caps.systemd), ("ps", caps.ps), ("/proc", caps.proc), ("ss", caps.ss), ("/proc/net", caps.proc_net), ("Docker daemon", caps.docker_daemon), ("Compose", caps.compose)] {
                            dt { "{name}" }
                            dd { if supported { "{t.available}" } else { "{t.unavailable}" } }
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
    mut docker_resource: Signal<DockerResource>,
    page: Option<OperationsPage>,
    context: ListContext,
    docker_cache: DockerUsageCache,
    docker_error: Option<String>,
    action: Option<OperationsViewState>,
) -> Element {
    let t = texts(language).operations;
    let mut filter = use_signal(String::new);
    let mut status_filter = use_signal(String::new);
    let mut process_sort = use_signal(|| ProcessSort::Cpu);
    let list = page
        .as_ref()
        .map(OperationsPage::parent)
        .unwrap_or_else(|| match tool {
            OperationsTool::Services => OperationsList::Services,
            OperationsTool::Processes => OperationsList::Processes,
            OperationsTool::Network => OperationsList::Network,
            _ => OperationsList::Docker(DockerResource::Containers),
        });
    let is_list = matches!(page, Some(OperationsPage::List(_)));
    use_effect(use_reactive(
        (&session_id, &list),
        move |(session_id, list)| {
            let saved = crate::components::app::get_state()
                .lock()
                .ok()
                .and_then(|state| {
                    session_id
                        .and_then(|id| state.sessions.get(&id))
                        .and_then(|s| s.operations_ui.contexts.get(&list))
                        .cloned()
                })
                .unwrap_or_default();
            filter.set(saved.filter);
            status_filter.set(saved.status_filter);
            process_sort.set(if saved.memory_sort {
                ProcessSort::Memory
            } else {
                ProcessSort::Cpu
            });
        },
    ));
    use_effect(use_reactive(
        (&session_id, &page),
        move |(session_id, page)| {
            let Some(page) = page else {
                return;
            };
            let list = page.parent();
            let restore_list = matches!(page, OperationsPage::List(_));
            let top = if restore_list {
                crate::components::app::get_state()
                    .lock()
                    .ok()
                    .and_then(|state| {
                        session_id
                            .and_then(|id| state.sessions.get(&id))
                            .and_then(|s| s.operations_ui.contexts.get(&list))
                            .map(|c| c.scroll_top)
                    })
                    .unwrap_or(0.0)
            } else {
                0.0
            };
            let script = format!(
                "const restoreTop = {top}; const restoreList = {restore_list};\n{}",
                include_str!("../assets/operations_viewport.js")
            );
            let mut eval = dioxus::document::eval(&script);
            spawn(async move {
                while let Ok(top) = eval.recv::<f64>().await {
                    if restore_list && top.is_finite() && top >= 0.0 {
                        update_list_context(session_id, list, |context| context.scroll_top = top);
                    }
                }
            });
        },
    ));
    use_drop(|| {
        dioxus::document::eval("window.__ktOperationsViewportCleanup?.();");
    });
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
    let result = view.as_ref().and_then(|value| value.result.clone());
    let failure_status = view
        .as_ref()
        .filter(|view| view.error.is_some())
        .map(|view| {
            if view.result.is_some() {
                ("degraded", t.degraded)
            } else {
                ("unavailable", t.unavailable)
            }
        });
    let process_rows = match result.as_ref() {
        Some(OperationsResult::Processes(rows)) => sorted_processes(rows, process_sort()),
        _ => Vec::new(),
    };

    rsx! {
        div { class: "operations-page-marker", hidden: true, "data-selected": context.selected.clone().unwrap_or_default() }
        if let Some(page) = page.as_ref() {
            OperationsBreadcrumb { page: page.clone(), session_id, language }
        }
        if let Some(action) = action {
            if action.loading { p { role: "status", "{t.refreshing}" } }
            if let Some(error) = action.error { p { class: "operations-error", role: "status", "{operations_error_message(language, error.kind)}" } }
            if let Some(OperationsResult::Action { message }) = action.result { p { class: "operations-action-result", role: "status", "{message}" } }
        }
        if tool == OperationsTool::Docker {
            if let Some(error) = docker_error { p { class: "operations-error", role: "status", "{error}" } }
        }
        if let Some((state, label)) = failure_status {
            p { class: "operations-error", "data-state": state, role: "status", "{label}" }
        }
        if let Some(error) = error {
            div { class: "operations-error", "{error}" }
        }

        if tool == OperationsTool::Docker {
            div { class: "operations-tabs",
                for resource in [DockerResource::Containers, DockerResource::Images, DockerResource::Volumes, DockerResource::Networks, DockerResource::Compose] {
                    button {
                        class: if docker_resource() == resource { "operations-tab is-active" } else { "operations-tab" },
                        onclick: move |_| {
                            docker_resource.set(resource);
                            request_operation(&crate::components::app::get_state().clone(), session_id, OperationsRequest::DockerList(resource));
                        },
                        match resource {
                            DockerResource::Containers => t.docker_containers,
                            DockerResource::Images => t.docker_images,
                            DockerResource::Volumes => t.docker_volumes,
                            DockerResource::Networks => t.docker_networks,
                            DockerResource::Compose => t.docker_compose,
                        }
                    }
                }
            }
        }

        if is_list {
            div { class: "operations-filter-bar",
                input {
                    class: "operations-filter-input",
                    r#type: "search",
                    placeholder: "{t.search}",
                    value: "{filter()}",
                    oninput: move |evt: Event<FormData>| { let value = evt.value(); filter.set(value.clone()); update_list_context(session_id, list, |context| context.filter = value); },
                }
                if tool == OperationsTool::Services {
                    input {
                        class: "operations-filter-input",
                        r#type: "search",
                        placeholder: "{t.status_filter}",
                        value: "{status_filter()}",
                        oninput: move |evt: Event<FormData>| { let value = evt.value(); status_filter.set(value.clone()); update_list_context(session_id, list, |context| context.status_filter = value); },
                    }
                }
                if tool == OperationsTool::Processes {
                    select {
                        class: "operations-filter-select",
                        value: if process_sort() == ProcessSort::Cpu { "cpu" } else { "memory" },
                        onchange: move |evt: Event<FormData>| {
                            process_sort.set(if evt.value() == "memory" { ProcessSort::Memory } else { ProcessSort::Cpu });
                            update_list_context(session_id, list, |context| context.memory_sort = evt.value() == "memory");
                        },
                        option { value: "cpu", "{t.sort_cpu}" }
                        option { value: "memory", "{t.sort_memory}" }
                    }
                }
            }
        }

        if let Some(result) = result {
            div { class: "operations-result-meta",
                span { "{operations_result_count_message(language, result_count(&result))}" }
                span { "{updated}" }
                if loading { span { "{t.refreshing}" } }
            }
            match &result {
                OperationsResult::Capabilities(_) => rsx! {},
                OperationsResult::Services(rows) => rsx! {
                    div { class: "operations-list operations-resource-list",
                        for service in rows.iter().filter(|row| {
                            let needle = filter().trim().to_ascii_lowercase();
                            let status = status_filter().trim().to_ascii_lowercase();
                            (needle.is_empty() || row.name.to_ascii_lowercase().contains(&needle) || row.description.to_ascii_lowercase().contains(&needle))
                                && (status.is_empty() || row.active_state.to_ascii_lowercase().contains(&status) || row.sub_state.to_ascii_lowercase().contains(&status))
                        }) {
                            ServiceRow { key: "{service.name}", service: service.clone(), session_id, language }
                        }
                    }
                },
                OperationsResult::Processes(_rows) => rsx! {
                    div { class: "operations-list operations-resource-list",
                        for process in process_rows.iter().filter(|row| {
                            let needle = filter().trim().to_ascii_lowercase();
                            needle.is_empty() || row.command.to_ascii_lowercase().contains(&needle) || row.pid.to_string() == needle
                        }) {
                            ProcessRow { key: "{process.pid}", process: process.clone(), session_id, language }
                        }
                    }
                },
                OperationsResult::NetworkConnections(rows) => rsx! {
                    div { class: "operations-list operations-resource-list",
                        for connection in rows.iter().filter(|connection| {
                            let needle = filter().trim().to_ascii_lowercase();
                            let status = status_filter().trim().to_ascii_lowercase();
                            let haystack = format!("{} {} {} {} {}", connection.protocol, connection.state, connection.local, connection.peer, connection.owner.as_deref().unwrap_or_default()).to_ascii_lowercase();
                            (needle.is_empty() || haystack.contains(&needle) || connection.owner.as_deref().unwrap_or_default().to_ascii_lowercase().contains(&needle))
                                && (status.is_empty() || connection.state.to_ascii_lowercase().contains(&status))
                        }) {
                            NetworkRow { connection: connection.clone(), language }
                        }
                    }
                },
                OperationsResult::DockerContainers(rows) => rsx! {
                    div { class: "operations-list operations-resource-list",
                        for container in rows.iter().filter(|row| { let needle = filter().trim().to_ascii_lowercase(); needle.is_empty() || row.name.to_ascii_lowercase().contains(&needle) || row.id.contains(&needle) || row.image.as_deref().unwrap_or_default().to_ascii_lowercase().contains(&needle) }) {
                            ContainerRow { key: "{container.id}", container: container.clone(), usage: usage_for_container(&docker_cache, container), session_id, language }
                        }
                    }
                },
                OperationsResult::DockerImages(rows) => rsx! {
                    div { class: "operations-list operations-resource-list",
                        for image in rows.iter().filter(|row| filter().trim().is_empty() || row.repository.to_ascii_lowercase().contains(&filter().trim().to_ascii_lowercase()) || row.id.contains(filter().trim())) {
                            DockerImageRow { key: "{image.id}", image: image.clone(), session_id, language }
                        }
                    }
                },
                OperationsResult::DockerVolumes(rows) => rsx! {
                    div { class: "operations-list operations-resource-list",
                        for volume in rows.iter().filter(|row| filter().trim().is_empty() || row.name.to_ascii_lowercase().contains(&filter().trim().to_ascii_lowercase())) {
                            DockerVolumeRow { key: "{volume.name}", volume: volume.clone(), session_id, language }
                        }
                    }
                },
                OperationsResult::DockerNetworks(rows) => rsx! {
                    div { class: "operations-list operations-resource-list",
                        for network in rows.iter().filter(|row| filter().trim().is_empty() || row.name.to_ascii_lowercase().contains(&filter().trim().to_ascii_lowercase())) {
                            DockerNetworkRow { key: "{network.id}", network: network.clone(), session_id, language }
                        }
                    }
                },
                OperationsResult::DockerCompose(rows) => rsx! {
                    div { class: "operations-list operations-resource-list",
                        for project in rows.iter().filter(|row| filter().trim().is_empty() || row.name.to_ascii_lowercase().contains(&filter().trim().to_ascii_lowercase())) {
                            DockerComposeRow { key: "{project.name}", project: project.clone(), session_id, language }
                        }
                    }
                },
                OperationsResult::Details { title, content } => rsx! {
                    div { class: "operations-details",
                        strong { "{title}" }
                        pre { "{content}" }
                    }
                },
                OperationsResult::DockerRuntime(_) => rsx! {
                    if let Some(OperationsPage::Detail { target: DetailTarget::DockerResources { id }, .. }) = page.as_ref() {
                        DockerUsage { value: super::docker_usage::usage_for_id(&docker_cache, id), language }
                    }
                    p { class: "operations-storage-hint", "{t.storage_hint}" }
                },
                OperationsResult::DockerStorage(_) | OperationsResult::DockerImageSizes(_) => rsx! {},
                OperationsResult::Action { message } => rsx! {
                    div { class: "operations-action-result", "{message}" }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProcessSort {
    Cpu,
    Memory,
}

fn sorted_processes(
    rows: &[kt_core::ProcessSummary],
    sort: ProcessSort,
) -> Vec<kt_core::ProcessSummary> {
    let mut rows = rows.to_vec();
    rows.sort_by(|left, right| {
        let ordering = match sort {
            ProcessSort::Cpu => right.cpu_percent.total_cmp(&left.cpu_percent),
            ProcessSort::Memory => right.memory_percent.total_cmp(&left.memory_percent),
        };
        ordering.then_with(|| left.pid.cmp(&right.pid))
    });
    rows
}

fn result_count(result: &OperationsResult) -> usize {
    match result {
        OperationsResult::Capabilities(_) => 0,
        OperationsResult::Services(rows) => rows.len(),
        OperationsResult::Processes(rows) => rows.len(),
        OperationsResult::NetworkConnections(rows) => rows.len(),
        OperationsResult::DockerContainers(rows) => rows.len(),
        OperationsResult::DockerImages(rows) => rows.len(),
        OperationsResult::DockerVolumes(rows) => rows.len(),
        OperationsResult::DockerNetworks(rows) => rows.len(),
        OperationsResult::DockerCompose(rows) => rows.len(),
        OperationsResult::DockerRuntime(rows) => rows.len(),
        OperationsResult::DockerStorage(rows) => rows.len(),
        OperationsResult::DockerImageSizes(rows) => rows.len(),
        OperationsResult::Details { content, .. } => usize::from(!content.is_empty()),
        OperationsResult::Action { .. } => 1,
    }
}

fn submit_operation(session_id: Option<SessionId>, request: OperationsRequest) {
    request_operation(
        &crate::components::app::get_state().clone(),
        session_id,
        request,
    );
}

fn update_list_context(
    session_id: Option<SessionId>,
    list: OperationsList,
    update: impl FnOnce(&mut ListContext),
) {
    if let Some(id) = session_id {
        if let Ok(mut state) = crate::components::app::get_state().lock() {
            if let Some(session) = state.sessions.get_mut(&id) {
                update(session.operations_ui.contexts.entry(list).or_default());
            }
        }
    }
}

fn list_label(list: OperationsList, language: AppLanguage) -> &'static str {
    let t = texts(language).operations;
    match list {
        OperationsList::Docker(DockerResource::Containers) => t.docker_containers,
        OperationsList::Docker(DockerResource::Images) => t.docker_images,
        OperationsList::Docker(DockerResource::Volumes) => t.docker_volumes,
        OperationsList::Docker(DockerResource::Networks) => t.docker_networks,
        OperationsList::Docker(DockerResource::Compose) => t.docker_compose,
        OperationsList::Services => t.tool_services,
        OperationsList::Processes => t.tool_processes,
        OperationsList::Network => t.tool_network,
    }
}

#[component]
fn OperationsBreadcrumb(
    page: OperationsPage,
    session_id: Option<SessionId>,
    language: AppLanguage,
) -> Element {
    let t = texts(language).operations;
    let parent = page.parent();
    let label = list_label(parent, language);
    let OperationsPage::Detail { target, .. } = page else {
        return rsx! {};
    };
    let title = match &target {
        DetailTarget::DockerLogs { .. } | DetailTarget::ServiceLogs { .. } => t.logs,
        DetailTarget::DockerNetwork { .. } => t.network,
        DetailTarget::DockerResources { .. } => t.resources,
        _ => t.details,
    };
    rsx! {
        nav { class: "operations-breadcrumb", aria_label: "{t.back}",
            button { r#type: "button", class: "operations-back", title: "{t.back}", aria_label: "{t.back}",
                onclick: move |_| { if let Some(id) = session_id { if let Ok(mut state) = crate::components::app::get_state().lock() { state.navigate_operation(id, OperationsPage::List(parent)); } } },
                Icon { name: "back" }
                span { "{label}" }
            }
            span { class: "operations-breadcrumb-target", title: target.identity(), "{target.identity()} / {title}" }
        }
    }
}

#[component]
fn ServiceRow(
    service: kt_core::ServiceSummary,
    session_id: Option<SessionId>,
    language: AppLanguage,
) -> Element {
    let t = texts(language).operations;
    let name = service.name.clone();
    let mut confirm_stop = use_signal(|| false);
    rsx! {
        div { class: "operations-resource-row",
            "data-resource-id": service.name.clone(),
            div { class: "operations-resource-primary", strong { "{service.name}" }, small { "{service.description}" } }
            div { class: "operations-resource-meta",
                span { class: "operations-status", "{service.active_state}" }
                span { "{service.sub_state}" }
                span { "{service.load_state}" }
                if session_id.is_some() {
                    div { class: "operations-row-actions",
                        button { class: "operations-action-button", title: "{t.details}", onclick: { let name = name.clone(); move |_| submit_operation(session_id, OperationsRequest::ServiceDetails { name: name.clone() }) }, Icon { name: "list" } }
                        button { class: "operations-action-button", title: "{t.logs}", onclick: { let name = name.clone(); move |_| submit_operation(session_id, OperationsRequest::ServiceLogs { name: name.clone(), lines: 200 }) }, Icon { name: "file" } }
                        button { class: "operations-action-button", title: "{t.start}", onclick: { let name = name.clone(); move |_| submit_operation(session_id, OperationsRequest::ServiceAction { name: name.clone(), action: ServiceAction::Start }) }, Icon { name: "play" } }
                        button { class: "operations-action-button", title: "{t.restart}", onclick: { let name = name.clone(); move |_| submit_operation(session_id, OperationsRequest::ServiceAction { name: name.clone(), action: ServiceAction::Restart }) }, Icon { name: "refresh" } }
                        button { class: "operations-action-button", title: "{t.enable}", onclick: { let name = name.clone(); move |_| submit_operation(session_id, OperationsRequest::ServiceAction { name: name.clone(), action: ServiceAction::Enable }) }, Icon { name: "check" } }
                        button { class: "operations-action-button", title: "{t.disable}", onclick: { let name = name.clone(); move |_| submit_operation(session_id, OperationsRequest::ServiceAction { name: name.clone(), action: ServiceAction::Disable }) }, Icon { name: "close" } }
                        button { class: if confirm_stop() { "operations-action-button is-confirm" } else { "operations-action-button danger" }, title: if confirm_stop() { t.confirm_action } else { t.stop }, onclick: { let name = name.clone(); move |_| if confirm_stop() { submit_operation(session_id, OperationsRequest::ServiceAction { name: name.clone(), action: ServiceAction::Stop }); confirm_stop.set(false); } else { confirm_stop.set(true); } }, Icon { name: "stop" } }
                    }
                }
            }
        }
    }
}

#[component]
fn ProcessRow(
    process: kt_core::ProcessSummary,
    session_id: Option<SessionId>,
    language: AppLanguage,
) -> Element {
    let t = texts(language).operations;
    let mut confirm_kill = use_signal(|| false);
    rsx! {
        div { class: "operations-resource-row",
            "data-resource-id": process.pid.to_string(),
            div { class: "operations-resource-primary", strong { "{process.command}" }, small { "{t.process_id} {process.pid} · {t.parent_process_id} {process.ppid} · {t.user_id} {process.uid} · {process.elapsed}" } }
            div { class: "operations-resource-meta",
                span { "{t.cpu} {process.cpu_percent:.1}%" }
                span { "{t.memory} {process.memory_percent:.1}%" }
                if session_id.is_some() {
                    div { class: "operations-row-actions",
                        button { class: "operations-action-button", title: "{t.details}", onclick: move |_| submit_operation(session_id, OperationsRequest::ProcessDetails { pid: process.pid }), Icon { name: "list" } }
                        button { class: "operations-action-button", title: "{t.terminate}", onclick: move |_| submit_operation(session_id, OperationsRequest::ProcessAction { pid: process.pid, action: ProcessAction::Terminate }), Icon { name: "stop" } }
                        button { class: if confirm_kill() { "operations-action-button is-confirm" } else { "operations-action-button danger" }, title: if confirm_kill() { t.confirm_action } else { t.kill }, onclick: move |_| if confirm_kill() { submit_operation(session_id, OperationsRequest::ProcessAction { pid: process.pid, action: ProcessAction::Kill }); confirm_kill.set(false); } else { confirm_kill.set(true); }, Icon { name: "trash" } }
                    }
                }
            }
        }
    }
}

#[component]
fn NetworkRow(connection: kt_core::NetworkConnection, language: AppLanguage) -> Element {
    let t = texts(language).operations;
    rsx! { div { class: "operations-resource-row", div { class: "operations-resource-primary", strong { "{connection.protocol} · {connection.state}" }, small { "{connection.local} -> {connection.peer}" }, if let Some(owner) = connection.owner { small { "{owner}" } } else { small { "{t.process_owner_unavailable}" } } } } }
}

#[component]
fn ContainerRow(
    container: kt_core::DockerContainer,
    usage: super::docker_usage::DockerUsageView,
    session_id: Option<SessionId>,
    language: AppLanguage,
) -> Element {
    let t = texts(language).operations;
    let mut confirm_remove = use_signal(|| false);
    let id = container.id.clone();
    rsx! {
        div { class: "operations-resource-row",
            "data-resource-id": container.id.clone(),
            div { class: "operations-resource-primary", strong { "{container.name}" }, small { "{container.image.clone().unwrap_or_else(|| t.unknown_image.to_string())}" }, small { "{container.id}" } }
            DockerUsage { value: usage, language }
            div { class: "operations-resource-meta",
                if let Some(status) = container.status { span { class: "operations-status", "{status}" } }
                if session_id.is_some() { div { class: "operations-row-actions",
                    button { class: "operations-action-button tooltip-trigger", "data-tooltip": "{t.container_open}", aria_label: "{t.container_open}", onclick: { let id = id.clone(); move |_| if let Some(session_id) = session_id { if let Ok(mut app_state) = crate::components::app::get_state().clone().lock() { let _ = app_state.open_container_terminal(session_id, id.clone(), PtySize::default()); } } }, Icon { name: "terminal" } }
                    button { class: "operations-action-button", title: "{t.details}", onclick: { let id = id.clone(); move |_| submit_operation(session_id, OperationsRequest::DockerInspect { resource: DockerResource::Containers, id: id.clone() }) }, Icon { name: "list" } }
                    button { class: "operations-action-button", title: "{t.logs}", onclick: { let id = id.clone(); move |_| submit_operation(session_id, OperationsRequest::DockerLogs { container: id.clone(), tail: 200 }) }, Icon { name: "file" } }
                    button { class: "operations-action-button", title: "{t.start}", onclick: { let id = id.clone(); move |_| submit_operation(session_id, OperationsRequest::DockerAction { resource: DockerResource::Containers, id: id.clone(), action: DockerAction::Start }) }, Icon { name: "play" } }
                    button { class: "operations-action-button", title: "{t.stop}", onclick: { let id = id.clone(); move |_| submit_operation(session_id, OperationsRequest::DockerAction { resource: DockerResource::Containers, id: id.clone(), action: DockerAction::Stop }) }, Icon { name: "stop" } }
                    button { class: "operations-action-button", title: "{t.restart}", onclick: { let id = id.clone(); move |_| submit_operation(session_id, OperationsRequest::DockerAction { resource: DockerResource::Containers, id: id.clone(), action: DockerAction::Restart }) }, Icon { name: "refresh" } }
                    button { class: if confirm_remove() { "operations-action-button is-confirm" } else { "operations-action-button danger" }, title: if confirm_remove() { t.confirm_action } else { t.remove }, onclick: { let id = id.clone(); move |_| if confirm_remove() { submit_operation(session_id, OperationsRequest::DockerAction { resource: DockerResource::Containers, id: id.clone(), action: DockerAction::Remove }); confirm_remove.set(false); } else { confirm_remove.set(true); } }, Icon { name: "trash" } }
                    button { class: "operations-action-button", title: "{t.resources}", onclick: { let id = id.clone(); move |_| submit_operation(session_id, OperationsRequest::DockerStats { container: Some(id.clone()) }) }, Icon { name: "monitor" } }
                    button { class: "operations-action-button", title: "{t.network}", onclick: { let id = id.clone(); move |_| submit_operation(session_id, OperationsRequest::DockerContainerNetwork { container: id.clone() }) }, Icon { name: "network" } }
                } }
            }
        }
    }
}

#[component]
fn DockerImageRow(
    image: kt_core::DockerImage,
    session_id: Option<SessionId>,
    language: AppLanguage,
) -> Element {
    let t = texts(language).operations;
    let mut confirm_remove = use_signal(|| false);
    rsx! { div { class: "operations-resource-row", "data-resource-id": image.id.clone(), div { class: "operations-resource-primary", strong { "{image.repository}:{image.tag}" }, small { "{image.id} · {image.size} · {image.created}" } }, div { class: "operations-resource-meta", if session_id.is_some() { div { class: "operations-row-actions",
        button { class: "operations-action-button", title: "{t.details}", onclick: { let id = image.id.clone(); move |_| submit_operation(session_id, OperationsRequest::DockerInspect { resource: DockerResource::Images, id: id.clone() }) }, Icon { name: "list" } }
        button { class: if confirm_remove() { "operations-action-button is-confirm" } else { "operations-action-button danger" }, title: if confirm_remove() { t.confirm_action } else { t.remove }, onclick: { let id = image.id.clone(); move |_| if confirm_remove() { submit_operation(session_id, OperationsRequest::DockerAction { resource: DockerResource::Images, id: id.clone(), action: DockerAction::Remove }); confirm_remove.set(false); } else { confirm_remove.set(true); } }, Icon { name: "trash" } }
    } } } } }
}

#[component]
fn DockerVolumeRow(
    volume: kt_core::DockerVolume,
    session_id: Option<SessionId>,
    language: AppLanguage,
) -> Element {
    let t = texts(language).operations;
    let mut confirm_remove = use_signal(|| false);
    rsx! { div { class: "operations-resource-row", "data-resource-id": volume.name.clone(), div { class: "operations-resource-primary", strong { "{volume.name}" }, small { "{volume.driver} · {volume.scope}" } }, div { class: "operations-resource-meta", if session_id.is_some() { div { class: "operations-row-actions",
        button { class: "operations-action-button", title: "{t.details}", onclick: { let name = volume.name.clone(); move |_| submit_operation(session_id, OperationsRequest::DockerInspect { resource: DockerResource::Volumes, id: name.clone() }) }, Icon { name: "list" } }
        button { class: if confirm_remove() { "operations-action-button is-confirm" } else { "operations-action-button danger" }, title: if confirm_remove() { t.confirm_action } else { t.remove }, onclick: { let name = volume.name.clone(); move |_| if confirm_remove() { submit_operation(session_id, OperationsRequest::DockerAction { resource: DockerResource::Volumes, id: name.clone(), action: DockerAction::Remove }); confirm_remove.set(false); } else { confirm_remove.set(true); } }, Icon { name: "trash" } }
    } } } } }
}

#[component]
fn DockerNetworkRow(
    network: kt_core::DockerNetwork,
    session_id: Option<SessionId>,
    language: AppLanguage,
) -> Element {
    let t = texts(language).operations;
    let mut confirm_remove = use_signal(|| false);
    rsx! { div { class: "operations-resource-row", "data-resource-id": network.id.clone(), div { class: "operations-resource-primary", strong { "{network.name}" }, small { "{network.id} · {network.driver} · {network.scope}" } }, div { class: "operations-resource-meta", if session_id.is_some() { div { class: "operations-row-actions",
        button { class: "operations-action-button", title: "{t.details}", onclick: { let id = network.id.clone(); move |_| submit_operation(session_id, OperationsRequest::DockerInspect { resource: DockerResource::Networks, id: id.clone() }) }, Icon { name: "list" } }
        button { class: if confirm_remove() { "operations-action-button is-confirm" } else { "operations-action-button danger" }, title: if confirm_remove() { t.confirm_action } else { t.remove }, onclick: { let id = network.id.clone(); move |_| if confirm_remove() { submit_operation(session_id, OperationsRequest::DockerAction { resource: DockerResource::Networks, id: id.clone(), action: DockerAction::Remove }); confirm_remove.set(false); } else { confirm_remove.set(true); } }, Icon { name: "trash" } }
    } } } } }
}

#[component]
fn DockerComposeRow(
    project: kt_core::DockerComposeProject,
    session_id: Option<SessionId>,
    language: AppLanguage,
) -> Element {
    let t = texts(language).operations;
    let mut confirm_remove = use_signal(|| false);
    rsx! { div { class: "operations-resource-row", div { class: "operations-resource-primary", strong { "{project.name}" }, small { "{project.config_files}" } }, div { class: "operations-resource-meta", span { class: "operations-status", "{project.status}" }, if session_id.is_some() { div { class: "operations-row-actions",
        button { class: "operations-action-button", title: "{t.start}", onclick: { let name = project.name.clone(); move |_| submit_operation(session_id, OperationsRequest::DockerAction { resource: DockerResource::Compose, id: name.clone(), action: DockerAction::Start }) }, Icon { name: "play" } }
        button { class: "operations-action-button", title: "{t.stop}", onclick: { let name = project.name.clone(); move |_| submit_operation(session_id, OperationsRequest::DockerAction { resource: DockerResource::Compose, id: name.clone(), action: DockerAction::Stop }) }, Icon { name: "stop" } }
        button { class: "operations-action-button", title: "{t.restart}", onclick: { let name = project.name.clone(); move |_| submit_operation(session_id, OperationsRequest::DockerAction { resource: DockerResource::Compose, id: name.clone(), action: DockerAction::Restart }) }, Icon { name: "refresh" } }
        button { class: if confirm_remove() { "operations-action-button is-confirm" } else { "operations-action-button danger" }, title: if confirm_remove() { t.confirm_action } else { t.remove }, onclick: { let name = project.name.clone(); move |_| if confirm_remove() { submit_operation(session_id, OperationsRequest::DockerAction { resource: DockerResource::Compose, id: name.clone(), action: DockerAction::Remove }); confirm_remove.set(false); } else { confirm_remove.set(true); } }, Icon { name: "trash" } }
    } } } } }
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
                        terminal_wrap: settings_value.terminal_wrap,
                        font_family: settings_value.font_family,
                        font_size: settings_value.font_size,
                        cursor_style: settings_value.cursor_style,
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

fn tool_button_is_active(
    can_use: bool,
    panel_visible: bool,
    selected: OperationsTool,
    item: OperationsTool,
) -> bool {
    can_use && panel_visible && selected == item
}

#[cfg(test)]
mod tests {
    use super::*;
    use kt_core::OperationsError;
    use std::time::Instant;

    #[test]
    fn panel_never_renders_previous_sessions_cached_signals() {
        assert!(snapshot_matches_session(
            Some(SessionId(1)),
            Some(SessionId(1))
        ));
        assert!(!snapshot_matches_session(
            Some(SessionId(1)),
            Some(SessionId(2))
        ));
        assert!(!snapshot_matches_session(Some(SessionId(1)), None));
        assert!(!snapshot_matches_session(None, None));
    }

    fn view() -> OperationsViewState {
        OperationsViewState {
            request_id: Some(kt_core::OperationId(1)),
            loading: false,
            error: None,
            result: None,
            requested_at: None,
            updated_at: Some(Instant::now()),
            refresh_due: false,
        }
    }

    #[test]
    fn capability_gate_blocks_stale_failed_and_unavailable_probe_results() {
        let mut view = view();
        let caps = kt_core::remote_ops::parse_capabilities(r#"{"linux":true,"systemd":false,"ps":true,"proc":false,"ss":false,"proc_net":true,"docker_daemon":true,"compose":false}"#).unwrap();
        view.result = Some(OperationsResult::Capabilities(caps));
        assert!(!probe_allows_refresh(
            None,
            OperationsDomain::Processes,
            false
        ));
        assert!(probe_allows_refresh(
            Some(&view),
            OperationsDomain::Processes,
            false
        ));
        assert!(!probe_allows_refresh(
            Some(&view),
            OperationsDomain::Services,
            false
        ));
        assert!(probe_allows_refresh(
            Some(&view),
            OperationsDomain::Docker,
            false
        ));
        assert!(!probe_allows_refresh(
            Some(&view),
            OperationsDomain::Docker,
            true
        ));
        view.loading = true;
        assert!(!probe_allows_refresh(
            Some(&view),
            OperationsDomain::Docker,
            false
        ));
        view.loading = false;
        view.error = Some(OperationsError::new(
            kt_core::OperationsErrorKind::Cancelled,
            "cancelled",
        ));
        assert!(!probe_allows_refresh(
            Some(&view),
            OperationsDomain::Docker,
            false
        ));
    }

    #[test]
    fn tool_button_is_only_active_when_its_panel_is_visible() {
        assert!(!tool_button_is_active(
            true,
            false,
            OperationsTool::Metrics,
            OperationsTool::Metrics
        ));
        assert!(!tool_button_is_active(
            false,
            true,
            OperationsTool::Metrics,
            OperationsTool::Metrics
        ));
        assert!(tool_button_is_active(
            true,
            true,
            OperationsTool::Metrics,
            OperationsTool::Metrics
        ));
        assert!(!tool_button_is_active(
            true,
            true,
            OperationsTool::Metrics,
            OperationsTool::Network
        ));
    }
}
