//! 桌面更新后台服务，不依赖 UI 或 SSH 核心。
pub mod archive;
pub mod client;
pub mod download;
pub mod install;
pub mod metadata;
pub mod progress;
pub mod proxy;
#[cfg(test)]
mod tests;

use anyhow::{ensure, Result};
use client::{Available, ReleaseClient, RetryAfter};
use install::Installation;
use metadata::{BuildInfo, Channel};
use proxy::UpdateProxy;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

/// 更新偏好：设备本机设置，写入 updates/preferences.json，不参与配置同步。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preferences {
    pub auto_check: bool,
    pub auto_download: bool,
    /// 检查更新与下载更新包使用的代理。
    #[serde(default)]
    pub proxy: UpdateProxy,
}
impl Preferences {
    fn defaults(info: &BuildInfo) -> Self {
        let enabled = info.channel != Channel::Development;
        Self {
            auto_check: enabled,
            auto_download: enabled,
            proxy: UpdateProxy::System,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Disabled,
    Idle,
    Checking,
    Cancelled,
    Available,
    Downloading,
    Ready,
    Preparing,
    Error,
}
impl Phase {
    pub fn busy(self) -> bool {
        matches!(self, Self::Checking | Self::Downloading | Self::Preparing)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub phase: Phase,
    pub preferences: Preferences,
    pub available: Option<Available>,
    pub received: u64,
    /// 平滑后的下载速率（字节/秒）；非下载阶段为 0。
    pub speed: u64,
    pub last_check: Option<u64>,
    pub message: Option<String>,
    /// 当前安装位置的替换需要管理员授权（便携副本位于受保护目录，或上次替换被拒绝）。
    pub requires_elevation: bool,
}
struct State {
    snapshot: Snapshot,
    cancellation: CancellationToken,
    /// 任务代次：改代理或重开任务时自增，用于丢弃过期任务的写回。
    generation: u64,
    next_check: u64,
    not_before: u64,
    failures: u32,
}
pub struct UpdateService {
    pub root: PathBuf,
    pub current: BuildInfo,
    pub installation: Installation,
    /// 当前生效的发布客户端；改代理时整体替换（reqwest Client 内部是 Arc）。
    client: Mutex<ReleaseClient>,
    state: Mutex<State>,
    runtime: tokio::runtime::Runtime,
}
impl UpdateService {
    pub fn open(root: PathBuf, installation: Installation) -> Result<Arc<Self>> {
        let current = BuildInfo::current();
        // 先读偏好再建客户端：落盘的代理设置必须从启动第一次检查开始生效。
        let (preferences, notice) = read_preferences(&root, &current)?;
        Self::open_with(
            root,
            installation,
            current,
            ReleaseClient::configured(&preferences.proxy)?,
            preferences,
            notice,
        )
    }
    fn open_with(
        root: PathBuf,
        installation: Installation,
        current: BuildInfo,
        client: ReleaseClient,
        preferences: Preferences,
        notice: Option<String>,
    ) -> Result<Arc<Self>> {
        install::private_root(&root)?;
        let recorded = if root.join("error.json").exists() {
            Some(install::read_json(&root.join("error.json"))?)
        } else {
            None
        };
        let phase = if current.channel == Channel::Development {
            Phase::Disabled
        } else if recorded.is_some() {
            Phase::Error
        } else {
            Phase::Idle
        };
        // 恢复过程中的记录与启动提示都要显示，任何一条都不能被吞掉。
        let message = match (recorded, notice) {
            (Some(recorded), Some(notice)) => Some(format!("{recorded}\n{notice}")),
            (Some(recorded), None) => Some(recorded),
            (None, notice) => notice,
        };
        // 便携副本位于受保护目录时（或上次替换被拒绝访问）需要管理员授权替换。
        let requires_elevation =
            installation.needs_elevation() || root.join("elevation.json").exists();
        let service = Arc::new(Self {
            root,
            current,
            installation,
            client: Mutex::new(client),
            state: Mutex::new(State {
                snapshot: Snapshot {
                    phase,
                    preferences,
                    available: None,
                    received: 0,
                    speed: 0,
                    last_check: None,
                    message,
                    requires_elevation,
                },
                cancellation: CancellationToken::new(),
                generation: 0,
                next_check: client::now() + 5,
                not_before: 0,
                failures: 0,
            }),
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("kt-update")
                .build()?,
        });
        let weak = Arc::downgrade(&service);
        service.runtime.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let Some(service) = weak.upgrade() else {
                    break;
                };
                let due = {
                    let s = service.state.lock().unwrap();
                    s.snapshot.preferences.auto_check
                        && !s.snapshot.phase.busy()
                        && s.snapshot.phase != Phase::Ready
                        && client::now() >= s.next_check.max(s.not_before)
                };
                if due && service.current.channel != Channel::Development {
                    let _ = service.check();
                }
            }
        });
        Ok(service)
    }
    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().unwrap().snapshot.clone()
    }
    /// 保存更新偏好。代理变化时重建发布客户端，并取消在途任务、丢弃其过期写回。
    pub fn preferences(&self, preferences: Preferences) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        ensure!(
            state.snapshot.phase != Phase::Preparing,
            "正在准备安装 / Installation preparation in progress"
        );
        let proxy_changed = state.snapshot.preferences.proxy != preferences.proxy;
        // 先构建新客户端：构建失败不落盘、不改内存。
        let client = proxy_changed
            .then(|| ReleaseClient::configured(&preferences.proxy))
            .transpose()?;
        if proxy_changed && state.snapshot.phase.busy() {
            // 换代理后旧连接必然失效：取消在途检查/下载，并让它们的写回过期。
            state.cancellation.cancel();
            state.generation = state.generation.wrapping_add(1);
            state.snapshot.phase = Phase::Idle;
            state.snapshot.available = None;
            state.snapshot.received = 0;
        }
        install::atomic_json(&self.root.join("preferences.json"), &preferences)?;
        if let Some(client) = client {
            state.snapshot.message = None;
            // 代理修好后不再承受上一次失败留下的退避。
            state.failures = 0;
            state.next_check = client::now();
            *self.client.lock().unwrap() = client;
        }
        state.snapshot.preferences = preferences;
        Ok(())
    }
    /// 当前生效的发布客户端；克隆成本只是一次 Arc 复制。
    fn client(&self) -> ReleaseClient {
        self.client.lock().unwrap().clone()
    }
    pub fn check(self: &Arc<Self>) -> Result<()> {
        let (client, generation) = {
            let mut state = self.state.lock().unwrap();
            ensure!(
                self.current.channel != Channel::Development,
                "开发构建禁用更新 / Updates disabled in development builds"
            );
            ensure!(
                !state.snapshot.phase.busy() && state.snapshot.phase != Phase::Ready,
                "更新任务已在运行或就绪 / Update already running or ready"
            );
            ensure!(
                client::now() >= state.not_before,
                "请等待限流结束 / Wait for rate limit reset"
            );
            state.generation = state.generation.wrapping_add(1);
            state.snapshot.phase = Phase::Checking;
            state.snapshot.message = None;
            state.snapshot.available = None;
            state.cancellation = CancellationToken::new();
            (self.client(), state.generation)
        };
        let service = Arc::clone(self);
        self.runtime.spawn(async move {
            let cancel = service.state.lock().unwrap().cancellation.clone();
            let result = tokio::select! {
                biased;
                _ = cancel.cancelled() => None,
                value = client.check(&service.current, service.installation.installed) => Some(value),
            };
            let auto_download = {
                let mut state = service.state.lock().unwrap();
                // 代次已前进：本次结果属于被替换的旧任务，不能写回状态。
                if state.generation != generation {
                    return;
                }
                if cancel.is_cancelled() {
                    state.snapshot.phase = Phase::Cancelled;
                    state.snapshot.message = None;
                    state.next_check = client::now() + 6 * 3600;
                    false
                } else {
                    match result.expect("非取消检查必须返回结果") {
                        Ok(available) => {
                            state.failures = 0;
                            state.next_check = client::now() + 6 * 3600;
                            state.snapshot.last_check = Some(client::now());
                            state.snapshot.phase = if available.is_some() { Phase::Available } else { Phase::Idle };
                            state.snapshot.available = available;
                            state.snapshot.preferences.auto_download && state.snapshot.phase == Phase::Available
                        }
                        Err(error) => {
                            let message = describe_failure(&error, &state.snapshot.preferences.proxy);
                            fail(&mut state, &error, message);
                            false
                        }
                    }
                }
            };
            if auto_download { let _ = service.download(); }
        });
        Ok(())
    }
    pub fn download(self: &Arc<Self>) -> Result<()> {
        let (available, cancel, client, generation) = {
            let mut state = self.state.lock().unwrap();
            ensure!(
                !state.snapshot.phase.busy() && state.snapshot.phase != Phase::Ready,
                "更新任务已在运行 / Update already running"
            );
            ensure!(
                client::now() >= state.not_before,
                "请等待限流结束 / Wait for rate limit reset"
            );
            let available = state
                .snapshot
                .available
                .clone()
                .ok_or_else(|| anyhow::anyhow!("请先检查更新 / Check for updates first"))?;
            state.generation = state.generation.wrapping_add(1);
            state.snapshot.phase = Phase::Downloading;
            state.snapshot.received = 0;
            state.snapshot.speed = 0;
            state.snapshot.message = None;
            state.cancellation = CancellationToken::new();
            (
                available,
                state.cancellation.clone(),
                self.client(),
                state.generation,
            )
        };
        let service = Arc::clone(self);
        self.runtime.spawn(async move {
            let result =
                download::download(&client, &available, &service.root, &cancel, |progress| {
                    let mut state = service.state.lock().unwrap();
                    if state.generation == generation {
                        state.snapshot.received = progress.received;
                        state.snapshot.speed = progress.speed;
                    }
                })
                .await;
            let mut state = service.state.lock().unwrap();
            // 代次已前进：本次下载属于被替换的旧任务，不能写回状态。
            if state.generation != generation {
                return;
            }
            let proxy = state.snapshot.preferences.proxy.clone();
            match result {
                Ok(_) if !cancel.is_cancelled() => {
                    state.snapshot.phase = Phase::Ready;
                    state.snapshot.message = None;
                    state.failures = 0;
                }
                Ok(_) => {
                    state.snapshot.phase = Phase::Available;
                }
                Err(_) if cancel.is_cancelled() => {
                    state.snapshot.phase = Phase::Available;
                    state.snapshot.message = None;
                }
                Err(error) => {
                    if error
                        .downcast_ref::<reqwest::Error>()
                        .is_some_and(|e| e.status() == Some(reqwest::StatusCode::NOT_FOUND))
                    {
                        state.snapshot.available = None;
                    }
                    let message = describe_failure(&error, &proxy);
                    fail(&mut state, &error, message);
                }
            }
        });
        Ok(())
    }
    pub fn cancel(&self) {
        let state = self.state.lock().unwrap();
        if matches!(state.snapshot.phase, Phase::Checking | Phase::Downloading) {
            state.cancellation.cancel();
        }
    }
    /// 准备安装事务；`elevate` 表示用户已确认用管理员权限替换受保护目录里的副本。
    pub fn prepare(&self, elevate: bool) -> Result<install::InstallTask> {
        let available = {
            let mut state = self.state.lock().unwrap();
            ensure!(
                state.snapshot.phase == Phase::Ready,
                "更新尚未下载完毕 / Update not ready"
            );
            state.snapshot.phase = Phase::Preparing;
            state.snapshot.available.clone().unwrap()
        };
        let result = install::prepare(
            &self.root,
            &self.installation,
            &self.current,
            &available,
            elevate,
        );
        if let Err(error) = &result {
            self.preparation_failed(format!("{error:#}"));
        }
        result
    }
    pub fn preparation_failed(&self, message: String) {
        let mut state = self.state.lock().unwrap();
        state.snapshot.phase = Phase::Ready;
        state.snapshot.message = Some(message);
    }
    pub fn commit_restart(&self) -> Result<()> {
        install::atomic_json(&self.root.join("commit.json"), &true)
    }
    pub fn abort_restart(&self) -> Result<()> {
        install::atomic_json(&self.root.join("abort.json"), &true)
    }
    pub fn confirm_startup(&self) -> Result<()> {
        // GUI 已持有 installation.lock，此时不存在辅助进程替换。
        let recovered = install::recover(
            &self.root,
            &self.current,
            &self.installation.executable,
            true,
        );
        // 事务已确认清理：上一次失败留下的提权标记不再需要。失败时保留，
        // 让界面继续提供「以管理员权限重试」。
        if recovered.is_ok() {
            install::remove(&self.root.join("elevation.json"))?;
        }
        recovered
    }
}

/// 读取偏好：文件缺失时按构建身份取默认值。
///
/// 落盘数据里不能用的代理会被丢弃并回退为系统代理，同时返回提示交由界面显示：
/// 一个手改坏的代理字段不应该让整个应用无法启动，但也不能被静默忽略。
fn read_preferences(root: &Path, current: &BuildInfo) -> Result<(Preferences, Option<String>)> {
    let path = root.join("preferences.json");
    let mut preferences = if path.exists() {
        install::read_json::<Preferences>(&path)?
    } else {
        Preferences::defaults(current)
    };
    let notice = match preferences.proxy.validate() {
        Ok(()) => None,
        Err(error) => {
            preferences.proxy = UpdateProxy::System;
            Some(format!(
                "代理设置无效，已回退为系统代理 / Invalid proxy settings; falling back to the system proxy: {error:#}"
            ))
        }
    };
    Ok((preferences, notice))
}

/// 把失败归类为可操作提示，并保留原始细节。
///
/// 经代理时最常见的三类失败（代理不可达、代理不转发、代理需要认证）必须能一眼
/// 区分，因此这里先给出原因，再附上底层错误文本。
fn describe_failure(error: &anyhow::Error, proxy: &UpdateProxy) -> String {
    if let Some(retry) = error.downcast_ref::<RetryAfter>() {
        return retry.to_string();
    }
    let cause = error.downcast_ref::<reqwest::Error>().and_then(|error| {
        if error.status() == Some(reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED) {
            Some("代理认证失败 / Proxy authentication failed")
        } else if error.is_timeout() {
            Some("请求超时 / Request timed out")
        } else if error.is_connect() {
            Some("无法连接更新服务器或代理 / Cannot connect to the update server or proxy")
        } else if error.is_redirect() {
            Some("更新重定向被拒绝 / Update redirect rejected")
        } else if error.is_status() {
            Some("更新服务器返回错误状态 / Update server returned an error status")
        } else {
            None
        }
    });
    let hint = proxy.hint();
    match (cause, hint) {
        (Some(cause), Some(hint)) => format!("{cause}；{hint}：{error:#}"),
        (Some(cause), None) => format!("{cause}：{error:#}"),
        (None, Some(hint)) => format!("{hint}：{error:#}"),
        (None, None) => format!("{error:#}"),
    }
}

fn fail(state: &mut State, error: &anyhow::Error, message: String) {
    state.snapshot.phase = Phase::Error;
    state.snapshot.message = Some(message);
    state.failures = state.failures.saturating_add(1);
    let retry = error
        .downcast_ref::<RetryAfter>()
        .map(|e| e.0)
        .unwrap_or_else(|| (30u64.saturating_mul(1u64 << state.failures.min(7))).min(3600));
    if error.downcast_ref::<RetryAfter>().is_some() {
        state.not_before = client::now().saturating_add(retry);
    }
    state.next_check = client::now().saturating_add(retry);
}

/// 桌面启动注入的辅助进程回调；返回前必须完成握手，GUI 才能退出。
#[derive(Clone)]
pub struct UpdateContext {
    pub service: Arc<UpdateService>,
    /// 桌面辅助进程入口；`elevate` 为真时用户已同意用管理员权限替换受保护目录里的副本。
    pub launch: fn(&UpdateService, bool) -> Result<()>,
}

#[cfg(test)]
mod preference_tests {
    use super::*;
    #[test]
    fn development_and_persistence_failure() {
        let d = tempfile::tempdir().unwrap();
        let exe = d.path().join("kitonyterms");
        std::fs::write(&exe, b"test").unwrap();
        let service = UpdateService::open(
            d.path().join("updates"),
            Installation::from_executable(exe).unwrap(),
        )
        .unwrap();
        assert_eq!(service.snapshot().phase, Phase::Disabled);
        assert!(!service.snapshot().preferences.auto_check);
        assert!(service.check().is_err());
        let old = service.snapshot().preferences;
        std::fs::create_dir(service.root.join("preferences.json")).unwrap();
        assert!(service
            .preferences(Preferences {
                auto_check: true,
                auto_download: true,
                proxy: UpdateProxy::System,
            })
            .is_err());
        assert_eq!(service.snapshot().preferences, old);
    }
}
