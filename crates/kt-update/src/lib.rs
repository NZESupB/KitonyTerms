//! 桌面更新后台服务，不依赖 UI 或 SSH 核心。
pub mod archive;
pub mod client;
pub mod download;
pub mod install;
pub mod metadata;
#[cfg(test)]
mod tests;

use anyhow::{ensure, Result};
use client::{Available, ReleaseClient, RetryAfter};
use install::Installation;
use metadata::{BuildInfo, Channel};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preferences {
    pub auto_check: bool,
    pub auto_download: bool,
}
impl Preferences {
    fn defaults(info: &BuildInfo) -> Self {
        let enabled = info.channel != Channel::Development;
        Self {
            auto_check: enabled,
            auto_download: enabled,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Disabled,
    Idle,
    Checking,
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
    pub last_check: Option<u64>,
    pub message: Option<String>,
}
struct State {
    snapshot: Snapshot,
    cancellation: CancellationToken,
    next_check: u64,
    not_before: u64,
    failures: u32,
}
pub struct UpdateService {
    pub root: PathBuf,
    pub current: BuildInfo,
    pub installation: Installation,
    client: ReleaseClient,
    state: Mutex<State>,
    runtime: tokio::runtime::Runtime,
}
impl UpdateService {
    pub fn open(root: PathBuf, installation: Installation) -> Result<Arc<Self>> {
        Self::open_with(
            root,
            installation,
            BuildInfo::current(),
            ReleaseClient::new()?,
        )
    }
    fn open_with(
        root: PathBuf,
        installation: Installation,
        current: BuildInfo,
        client: ReleaseClient,
    ) -> Result<Arc<Self>> {
        install::private_root(&root)?;
        let preferences = if root.join("preferences.json").exists() {
            install::read_json(&root.join("preferences.json"))?
        } else {
            Preferences::defaults(&current)
        };
        let message = if root.join("error.json").exists() {
            Some(install::read_json(&root.join("error.json"))?)
        } else {
            None
        };
        let phase = if current.channel == Channel::Development {
            Phase::Disabled
        } else if message.is_some() {
            Phase::Error
        } else {
            Phase::Idle
        };
        let service = Arc::new(Self {
            root,
            current,
            installation,
            client,
            state: Mutex::new(State {
                snapshot: Snapshot {
                    phase,
                    preferences,
                    available: None,
                    received: 0,
                    last_check: None,
                    message,
                },
                cancellation: CancellationToken::new(),
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
    pub fn preferences(&self, preferences: Preferences) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        ensure!(
            state.snapshot.phase != Phase::Preparing,
            "正在准备安装 / Installation preparation in progress"
        );
        install::atomic_json(&self.root.join("preferences.json"), &preferences)?;
        state.snapshot.preferences = preferences;
        Ok(())
    }
    pub fn check(self: &Arc<Self>) -> Result<()> {
        {
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
            state.snapshot.phase = Phase::Checking;
            state.snapshot.message = None;
            state.snapshot.available = None;
            state.cancellation = CancellationToken::new();
        }
        let service = Arc::clone(self);
        self.runtime.spawn(async move {
            let cancel = service.state.lock().unwrap().cancellation.clone();
            let result = tokio::select! { _ = cancel.cancelled() => Err(anyhow::anyhow!("检查已取消 / Check cancelled")), value = service.client.check(&service.current, service.installation.installed) => value };
            let auto_download = {
                let mut state = service.state.lock().unwrap();
                state.snapshot.last_check = Some(client::now());
                match result {
                    Ok(available) => {
                        state.failures = 0; state.next_check = client::now() + 6 * 3600;
                        state.snapshot.phase = if available.is_some() { Phase::Available } else { Phase::Idle };
                        state.snapshot.available = available;
                        state.snapshot.preferences.auto_download && state.snapshot.phase == Phase::Available
                    }
                    Err(error) => { fail(&mut state, &error); false }
                }
            };
            if auto_download { let _ = service.download(); }
        });
        Ok(())
    }
    pub fn download(self: &Arc<Self>) -> Result<()> {
        let (available, cancel) = {
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
            state.snapshot.phase = Phase::Downloading;
            state.snapshot.received = 0;
            state.snapshot.message = None;
            state.cancellation = CancellationToken::new();
            (available, state.cancellation.clone())
        };
        let service = Arc::clone(self);
        self.runtime.spawn(async move {
            let result = download::download(
                &service.client,
                &available,
                &service.root,
                &cancel,
                |received| {
                    service.state.lock().unwrap().snapshot.received = received;
                },
            )
            .await;
            let mut state = service.state.lock().unwrap();
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
                    fail(&mut state, &error);
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
    pub fn prepare(&self) -> Result<install::InstallTask> {
        let available = {
            let mut state = self.state.lock().unwrap();
            ensure!(
                state.snapshot.phase == Phase::Ready,
                "更新尚未下载完毕 / Update not ready"
            );
            state.snapshot.phase = Phase::Preparing;
            state.snapshot.available.clone().unwrap()
        };
        let result = install::prepare(&self.root, &self.installation, &self.current, &available);
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
        install::recover(
            &self.root,
            &self.current,
            &self.installation.executable,
            true,
        )
    }
}
fn fail(state: &mut State, error: &anyhow::Error) {
    state.snapshot.phase = Phase::Error;
    state.snapshot.message = Some(format!("{error:#}"));
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
    pub launch: fn(&UpdateService) -> Result<()>,
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
                auto_download: true
            })
            .is_err());
        assert_eq!(service.snapshot().preferences, old);
    }
}
