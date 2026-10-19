//! 容器资源卡片只接收当前容器的轻量快照，数值未变化时不重建整行。

use super::metrics_format::format_bytes;
use crate::{i18n::texts, operations_state::DockerUsageCache};
use dioxus::prelude::*;
use kt_config::AppLanguage;
use kt_core::{DockerContainer, DockerContainerStorage, DockerRuntimeUsage};

#[derive(Clone, Default, PartialEq)]
pub(crate) struct DockerUsageView {
    pub usage: Option<DockerRuntimeUsage>,
    pub storage: Option<DockerContainerStorage>,
    pub image_size: Option<u64>,
    pub runtime_stale: bool,
    pub storage_stale: bool,
    pub stopped: bool,
}

pub(crate) fn usage_for_container(
    cache: &DockerUsageCache,
    container: &DockerContainer,
) -> DockerUsageView {
    usage_for(cache, &container.id, Some(container))
}

pub(crate) fn usage_for_id(cache: &DockerUsageCache, id: &str) -> DockerUsageView {
    let container = cache
        .containers
        .as_deref()
        .unwrap_or_default()
        .iter()
        .find(|c| c.id == id);
    usage_for(cache, id, container)
}

fn usage_for(
    cache: &DockerUsageCache,
    id: &str,
    container: Option<&DockerContainer>,
) -> DockerUsageView {
    let runtime = cache.runtime.get(id);
    let storage = cache.storage.get(id);
    let image_size = storage
        .and_then(|s| cache.images.get(&s.value.image_id))
        .and_then(|s| s.value.size);
    DockerUsageView {
        usage: runtime.map(|r| r.value.clone()),
        storage: storage.map(|r| r.value.clone()),
        image_size,
        runtime_stale: runtime.is_some_and(|r| r.at.elapsed().as_secs() >= 6),
        storage_stale: storage.is_some_and(|r| r.at.elapsed().as_secs() >= cache.storage_ttl()),
        stopped: container
            .and_then(|c| c.state.as_deref())
            .is_some_and(|s| !matches!(s, "running" | "paused")),
    }
}

#[component]
pub(crate) fn DockerUsage(value: DockerUsageView, language: AppLanguage) -> Element {
    let t = texts(language).operations;
    let cpu = if value.stopped {
        "--".to_string()
    } else {
        value
            .usage
            .as_ref()
            .and_then(|r| r.cpu_percent)
            .map(|v| format!("{v:.1}%"))
            .unwrap_or_else(|| "--".into())
    };
    let memory = if value.stopped {
        "--".to_string()
    } else {
        value
            .usage
            .as_ref()
            .map(|r| {
                format!(
                    "{} / {}{}",
                    optional_bytes(r.memory_used_bytes),
                    optional_bytes(r.memory_limit_bytes),
                    r.memory_percent
                        .map(|p| format!(" ({p:.1}%)"))
                        .unwrap_or_default()
                )
            })
            .unwrap_or_else(|| "--".into())
    };
    rsx! {
        div { class: "docker-usage",
            div { class: "docker-usage-runtime",
                span { "{t.cpu} {cpu}" }
                span { "{t.memory} {memory}" }
                if value.stopped { small { "{t.not_running}" } }
                else if value.runtime_stale { small { class: "is-stale", "{t.stale}" } }
            }
            div { class: "docker-usage-storage", title: "{t.storage_hint}",
                span { "{t.image_size} {optional_bytes(value.image_size)}" }
                span { "{t.writable_size} {optional_bytes(value.storage.as_ref().and_then(|s| s.size_rw))}" }
                span { "{t.rootfs_size} {optional_bytes(value.storage.as_ref().and_then(|s| s.size_rootfs))}" }
                if value.storage_stale { small { class: "is-stale", "{t.stale}" } }
            }
        }
    }
}

fn optional_bytes(value: Option<u64>) -> String {
    value.map(format_bytes).unwrap_or_else(|| "--".into())
}
