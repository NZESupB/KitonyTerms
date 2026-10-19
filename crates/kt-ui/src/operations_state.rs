//! 运维页面、请求身份与 Docker 缓存；仅存在于所属 SSH 会话中。

use kt_core::{
    DockerContainerStorage, DockerImageSize, DockerResource, DockerRuntimeUsage, OperationId,
    OperationsDomain, OperationsErrorKind, OperationsRequest, OperationsResult,
};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OperationsList {
    Docker(DockerResource),
    Services,
    Processes,
    Network,
}
impl OperationsList {
    pub fn domain(self) -> OperationsDomain {
        match self {
            Self::Docker(_) => OperationsDomain::Docker,
            Self::Services => OperationsDomain::Services,
            Self::Processes => OperationsDomain::Processes,
            Self::Network => OperationsDomain::Network,
        }
    }
    pub fn request(self) -> OperationsRequest {
        match self {
            Self::Docker(resource) => OperationsRequest::DockerList(resource),
            _ => OperationsRequest::Refresh(self.domain()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DetailTarget {
    DockerInspect {
        resource: DockerResource,
        id: String,
    },
    DockerLogs {
        id: String,
    },
    DockerNetwork {
        id: String,
    },
    DockerResources {
        id: String,
    },
    ServiceDetails {
        name: String,
    },
    ServiceLogs {
        name: String,
    },
    ProcessDetails {
        pid: u32,
    },
}
impl DetailTarget {
    pub fn parent(&self) -> OperationsList {
        match self {
            Self::DockerInspect { resource, .. } => OperationsList::Docker(*resource),
            Self::DockerLogs { .. } | Self::DockerNetwork { .. } | Self::DockerResources { .. } => {
                OperationsList::Docker(DockerResource::Containers)
            }
            Self::ServiceDetails { .. } | Self::ServiceLogs { .. } => OperationsList::Services,
            Self::ProcessDetails { .. } => OperationsList::Processes,
        }
    }
    pub fn identity(&self) -> String {
        match self {
            Self::DockerInspect { id, .. }
            | Self::DockerLogs { id }
            | Self::DockerNetwork { id }
            | Self::DockerResources { id } => id.clone(),
            Self::ServiceDetails { name } | Self::ServiceLogs { name } => name.clone(),
            Self::ProcessDetails { pid } => pid.to_string(),
        }
    }
    pub fn request(&self) -> OperationsRequest {
        match self {
            Self::DockerInspect { resource, id } => OperationsRequest::DockerInspect {
                resource: *resource,
                id: id.clone(),
            },
            Self::DockerLogs { id } => OperationsRequest::DockerLogs {
                container: id.clone(),
                tail: 200,
            },
            Self::DockerNetwork { id } => OperationsRequest::DockerContainerNetwork {
                container: id.clone(),
            },
            Self::DockerResources { id } => OperationsRequest::DockerStats {
                container: Some(id.clone()),
            },
            Self::ServiceDetails { name } => {
                OperationsRequest::ServiceDetails { name: name.clone() }
            }
            Self::ServiceLogs { name } => OperationsRequest::ServiceLogs {
                name: name.clone(),
                lines: 200,
            },
            Self::ProcessDetails { pid } => OperationsRequest::ProcessDetails { pid: *pid },
        }
    }
    pub fn refresh_interval(&self) -> Option<Duration> {
        match self {
            Self::DockerLogs { .. } | Self::ServiceLogs { .. } => Some(Duration::from_secs(3)),
            Self::DockerResources { .. } | Self::ProcessDetails { .. } => {
                Some(Duration::from_secs(2))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum OperationsPage {
    List(OperationsList),
    Detail {
        parent: OperationsList,
        target: DetailTarget,
    },
}
impl OperationsPage {
    pub fn parent(&self) -> OperationsList {
        match self {
            Self::List(list) | Self::Detail { parent: list, .. } => *list,
        }
    }
    pub fn key(&self) -> OperationKey {
        match self {
            Self::List(list) => OperationKey::List(*list),
            Self::Detail { target, .. } => OperationKey::Detail(target.clone()),
        }
    }
    pub fn request(&self) -> OperationsRequest {
        match self {
            Self::List(list) => list.request(),
            Self::Detail { target, .. } => target.request(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum OperationKey {
    Capabilities,
    List(OperationsList),
    Detail(DetailTarget),
    DockerRuntime,
    DockerStorage,
    DockerImageSizes,
    Action(OperationsDomain),
}
impl From<OperationsDomain> for OperationKey {
    fn from(domain: OperationsDomain) -> Self {
        match domain {
            OperationsDomain::Capabilities => Self::Capabilities,
            OperationsDomain::Docker => {
                Self::List(OperationsList::Docker(DockerResource::Containers))
            }
            OperationsDomain::Services => Self::List(OperationsList::Services),
            OperationsDomain::Processes => Self::List(OperationsList::Processes),
            OperationsDomain::Network => Self::List(OperationsList::Network),
        }
    }
}
impl OperationKey {
    pub fn for_request(request: &OperationsRequest) -> Self {
        match request {
            OperationsRequest::ProbeCapabilities => Self::Capabilities,
            OperationsRequest::Refresh(domain) => (*domain).into(),
            OperationsRequest::DockerList(resource) => {
                Self::List(OperationsList::Docker(*resource))
            }
            OperationsRequest::DockerInspect { resource, id } => {
                Self::Detail(DetailTarget::DockerInspect {
                    resource: *resource,
                    id: id.clone(),
                })
            }
            OperationsRequest::DockerLogs { container, .. } => {
                Self::Detail(DetailTarget::DockerLogs {
                    id: container.clone(),
                })
            }
            OperationsRequest::DockerContainerNetwork { container } => {
                Self::Detail(DetailTarget::DockerNetwork {
                    id: container.clone(),
                })
            }
            OperationsRequest::DockerStats {
                container: Some(id),
            } => Self::Detail(DetailTarget::DockerResources { id: id.clone() }),
            OperationsRequest::DockerStats { container: None } => Self::DockerRuntime,
            OperationsRequest::DockerStorage { .. } => Self::DockerStorage,
            OperationsRequest::DockerImageSizes { .. } => Self::DockerImageSizes,
            OperationsRequest::ServiceDetails { name } => {
                Self::Detail(DetailTarget::ServiceDetails { name: name.clone() })
            }
            OperationsRequest::ServiceLogs { name, .. } => {
                Self::Detail(DetailTarget::ServiceLogs { name: name.clone() })
            }
            OperationsRequest::ProcessDetails { pid } => {
                Self::Detail(DetailTarget::ProcessDetails { pid: *pid })
            }
            _ => Self::Action(request.domain()),
        }
    }
    pub fn domain(&self) -> OperationsDomain {
        match self {
            Self::Capabilities => OperationsDomain::Capabilities,
            Self::List(list) => list.domain(),
            Self::Detail(target) => target.parent().domain(),
            Self::Action(domain) => *domain,
            _ => OperationsDomain::Docker,
        }
    }
    pub fn is_background(&self) -> bool {
        matches!(
            self,
            Self::DockerRuntime | Self::DockerStorage | Self::DockerImageSizes
        )
    }
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct ListContext {
    pub filter: String,
    pub status_filter: String,
    pub memory_sort: bool,
    pub scroll_top: f64,
    pub selected: Option<String>,
}

#[derive(Clone, Default)]
pub struct RetryState {
    pub failures: u32,
    pub last_failure: Option<Instant>,
    pub blocked: bool,
}
impl RetryState {
    pub fn failed(&mut self, kind: OperationsErrorKind) {
        self.failures = self.failures.saturating_add(1);
        self.last_failure = Some(Instant::now());
        self.blocked = matches!(
            kind,
            OperationsErrorKind::PermissionDenied
                | OperationsErrorKind::CommandMissing
                | OperationsErrorKind::UnsupportedBackend
                | OperationsErrorKind::ParseFailed
                | OperationsErrorKind::OutputLimitExceeded
                | OperationsErrorKind::Cancelled
        );
    }
    pub fn ready(&self, now: Instant) -> bool {
        !self.blocked
            && self.last_failure.is_none_or(|time| {
                now.saturating_duration_since(time)
                    >= Duration::from_secs((1u64 << self.failures.min(5)).min(30))
            })
    }
}

#[derive(Clone, PartialEq)]
pub struct Timed<T> {
    pub value: T,
    pub at: Instant,
}

#[derive(Clone, Default, PartialEq)]
pub struct DockerUsageCache {
    pub runtime: HashMap<String, Timed<DockerRuntimeUsage>>,
    pub storage: HashMap<String, Timed<DockerContainerStorage>>,
    pub images: HashMap<String, Timed<DockerImageSize>>,
    pub storage_ttl: u64,
    pub containers: Option<std::sync::Arc<[kt_core::DockerContainer]>>,
}
impl DockerUsageCache {
    pub fn storage_ttl(&self) -> u64 {
        self.storage_ttl.max(60)
    }
    pub fn apply(&mut self, result: &OperationsResult, elapsed: Duration) {
        let now = Instant::now();
        match result {
            OperationsResult::DockerRuntime(rows) => {
                for row in rows.iter() {
                    if self
                        .containers
                        .as_ref()
                        .is_some_and(|rows| !rows.iter().any(|c| c.id == row.container_id))
                    {
                        continue;
                    }
                    self.runtime.insert(
                        row.container_id.clone(),
                        Timed {
                            value: row.clone(),
                            at: now,
                        },
                    );
                }
            }
            OperationsResult::DockerStorage(rows) => {
                self.storage_ttl = if elapsed >= Duration::from_secs(5) {
                    (self.storage_ttl().saturating_mul(2)).min(300)
                } else {
                    60
                };
                for row in rows.iter() {
                    if self
                        .containers
                        .as_ref()
                        .is_some_and(|rows| !rows.iter().any(|c| c.id == row.container_id))
                    {
                        continue;
                    }
                    self.storage.insert(
                        row.container_id.clone(),
                        Timed {
                            value: row.clone(),
                            at: now,
                        },
                    );
                }
            }
            OperationsResult::DockerImageSizes(rows) => {
                for row in rows.iter() {
                    if !self
                        .storage
                        .values()
                        .any(|s| s.value.image_id == row.image_id)
                    {
                        continue;
                    }
                    self.images.insert(
                        row.image_id.clone(),
                        Timed {
                            value: row.clone(),
                            at: now,
                        },
                    );
                }
            }
            OperationsResult::DockerContainers(rows) => {
                self.containers = Some(rows.clone());
                let ids: HashSet<_> = rows.iter().map(|row| row.id.as_str()).collect();
                self.runtime.retain(|id, _| ids.contains(id.as_str()));
                self.storage.retain(|id, _| ids.contains(id.as_str()));
                let images: HashSet<_> = self
                    .storage
                    .values()
                    .map(|row| row.value.image_id.as_str())
                    .collect();
                self.images.retain(|id, _| images.contains(id.as_str()));
            }
            _ => {}
        }
    }
}

#[derive(Clone, Default)]
pub struct OperationsUiState {
    pub pending: HashMap<OperationId, OperationKey>,
    pub pages: HashMap<OperationsDomain, OperationsPage>,
    pub contexts: HashMap<OperationsList, ListContext>,
    pub retry: HashMap<OperationKey, RetryState>,
    pub docker: DockerUsageCache,
}
impl OperationsUiState {
    pub fn page(&self, domain: OperationsDomain) -> OperationsPage {
        self.pages.get(&domain).cloned().unwrap_or_else(|| {
            let OperationKey::List(list) = OperationKey::from(domain) else {
                unreachable!("能力探测没有页面")
            };
            OperationsPage::List(list)
        })
    }
}

impl crate::state::AppState {
    /// 手动刷新当前页面；仅对本领域解除重试门禁。
    pub fn refresh_operation_page(&mut self, id: kt_core::SessionId, domain: OperationsDomain) {
        let Some(session) = self.sessions.get_mut(&id) else {
            return;
        };
        session
            .operations_ui
            .retry
            .retain(|key, _| key.domain() != domain);
        if domain == OperationsDomain::Docker {
            let stale = Instant::now() - Duration::from_secs(301);
            for row in session.operations_ui.docker.storage.values_mut() {
                row.at = stale;
            }
        }
        let request = session.operations_ui.page(domain).request();
        let _ = self.request_operation(id, request);
    }

    pub fn poll_operation_page(
        &mut self,
        id: kt_core::SessionId,
        domain: OperationsDomain,
        interval: Duration,
    ) {
        let Some(session) = self.sessions.get(&id).filter(|s| s.connected) else {
            return;
        };
        let page = session.operations_ui.page(domain);
        let period = match &page {
            OperationsPage::List(_) => Some(interval),
            OperationsPage::Detail { target, .. } => target.refresh_interval(),
        };
        let key = page.key();
        let view = session.operations.get(&key);
        let ready = session
            .operations_ui
            .retry
            .get(&key)
            .is_none_or(|retry| retry.ready(Instant::now()));
        let due = ready
            && view.is_none_or(|v| {
                !v.loading
                    && (v.refresh_due
                        || v.updated_at.is_none()
                        || period.is_some_and(|period| {
                            v.updated_at.is_some_and(|at| at.elapsed() >= period)
                        }))
            });
        if due {
            let _ = self.request_operation(id, page.request());
        }
        if domain == OperationsDomain::Docker {
            self.poll_docker_usage(id, &page);
        }
    }

    fn poll_docker_usage(&mut self, id: kt_core::SessionId, page: &OperationsPage) {
        let active = matches!(
            page,
            OperationsPage::List(OperationsList::Docker(DockerResource::Containers))
                | OperationsPage::Detail {
                    target: DetailTarget::DockerResources { .. },
                    ..
                }
        );
        if !active {
            for key in [
                OperationKey::DockerRuntime,
                OperationKey::DockerStorage,
                OperationKey::DockerImageSizes,
            ] {
                self.cancel_operation_key(id, &key);
            }
            return;
        }
        let Some(session) = self.sessions.get(&id) else {
            return;
        };
        let now = Instant::now();
        let ready = |key: &OperationKey| {
            !session.operations.get(key).is_some_and(|v| v.loading)
                && session
                    .operations_ui
                    .retry
                    .get(key)
                    .is_none_or(|retry| retry.ready(now))
        };
        let runtime_due = matches!(page, OperationsPage::List(_))
            && ready(&OperationKey::DockerRuntime)
            && session
                .operations
                .get(&OperationKey::DockerRuntime)
                .and_then(|v| v.updated_at)
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(2));
        let slow_busy = [OperationKey::DockerStorage, OperationKey::DockerImageSizes]
            .iter()
            .any(|key| session.operations.get(key).is_some_and(|v| v.loading));
        let foreground_busy = session
            .operations
            .iter()
            .any(|(key, view)| !key.is_background() && view.loading);
        let cache = &session.operations_ui.docker;
        let images: Vec<_> = cache
            .storage
            .values()
            .map(|s| s.value.image_id.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .filter(|image| !cache.images.contains_key(image))
            .take(64)
            .collect();
        let selected = match page {
            OperationsPage::Detail {
                target: DetailTarget::DockerResources { id },
                ..
            } => Some(id.as_str()),
            _ => None,
        };
        let containers = session
            .operations
            .get(&OperationKey::from(OperationsDomain::Docker))
            .and_then(|view| match &view.result {
                Some(OperationsResult::DockerContainers(rows)) => Some(rows.clone()),
                _ => None,
            });
        let mut stale: Vec<_> = containers
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|row| selected.is_none_or(|id| id == row.id))
            .filter(|row| {
                cache.storage.get(&row.id).is_none_or(|cached| {
                    cached.at.elapsed() >= Duration::from_secs(cache.storage_ttl())
                })
            })
            .map(|row| row.id.clone())
            .collect();
        stale.sort_by_key(|id| cache.storage.get(id).map(|row| row.at));
        stale.truncate(64);
        let slow_request = if slow_busy || foreground_busy {
            None
        } else if !images.is_empty() && ready(&OperationKey::DockerImageSizes) {
            Some(OperationsRequest::DockerImageSizes { images })
        } else if !stale.is_empty() && ready(&OperationKey::DockerStorage) {
            Some(OperationsRequest::DockerStorage { containers: stale })
        } else {
            None
        };
        if runtime_due && !foreground_busy {
            let _ = self.request_operation(id, OperationsRequest::DockerStats { container: None });
        }
        if let Some(request) = slow_request {
            let _ = self.request_operation(id, request);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requests_keep_lists_details_and_monitoring_separate() {
        let log = OperationKey::for_request(&OperationsRequest::DockerLogs {
            container: "abc".into(),
            tail: 200,
        });
        assert_ne!(log, OperationsDomain::Docker.into());
        assert_ne!(log, OperationKey::DockerRuntime);
        let OperationKey::Detail(target) = log else {
            panic!("详情键缺失")
        };
        assert_eq!(
            OperationKey::for_request(&target.request()),
            OperationKey::Detail(target.clone())
        );
        assert_eq!(
            target.parent(),
            OperationsList::Docker(DockerResource::Containers)
        );
    }

    #[test]
    fn retry_is_bounded_and_permissions_wait_for_explicit_retry() {
        let mut retry = RetryState::default();
        retry.failed(OperationsErrorKind::Timeout);
        let at = retry.last_failure.unwrap();
        assert!(!retry.ready(at + Duration::from_secs(1)));
        assert!(retry.ready(at + Duration::from_secs(2)));
        retry.failed(OperationsErrorKind::PermissionDenied);
        assert!(!retry.ready(at + Duration::from_secs(600)));
    }

    #[test]
    fn docker_cache_prunes_deleted_rows_and_bounds_slow_storage_ttl() {
        let mut cache = DockerUsageCache::default();
        let storage = OperationsResult::DockerStorage(
            vec![DockerContainerStorage {
                container_id: "abc".into(),
                image_id: "sha256:def".into(),
                size_rw: Some(10),
                size_rootfs: Some(100),
            }]
            .into(),
        );
        for _ in 0..8 {
            cache.apply(&storage, Duration::from_secs(6));
        }
        assert_eq!(cache.storage_ttl(), 300);
        cache.apply(
            &OperationsResult::DockerImageSizes(
                vec![DockerImageSize {
                    image_id: "sha256:def".into(),
                    size: Some(90),
                }]
                .into(),
            ),
            Duration::ZERO,
        );
        assert_eq!(cache.images.len(), 1);
        cache.apply(
            &OperationsResult::DockerContainers([].into()),
            Duration::ZERO,
        );
        assert!(cache.storage.is_empty() && cache.images.is_empty());
        cache.apply(&storage, Duration::ZERO);
        assert!(cache.storage.is_empty(), "迟到查询不能恢复已删除容器");
        assert_eq!(cache.storage_ttl(), 60);
    }
}
