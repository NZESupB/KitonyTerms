//! 应用持久化层 —— 桥接 `kt-config`(TOML 会话/设置)与 `kt-secrets`(加密 vault)。
//!
//! App persistence layer — bridges `kt-config` (TOML sessions/settings) and
//! `kt-secrets` (encrypted vault).
//!
//! ## 锁定模型 / Lock model
//! - 启动时 [`Store`] 处于**锁定**状态:vault 尚未打开,任何 `get_secret` 都返回 `None`。
//! - 首次运行(无 vault 文件)用 [`Store::create_vault`] 设置主密码。
//! - 后续运行用 [`Store::unlock`] 打开 vault。
//! - 未解锁时,会话仍可连接,但无法读取/保存密码(连接对话框会要求现场输入)。
//!
//! ## 会话与密钥 / Sessions vs. secrets
//! - **会话**(`SessionProfile`:host/port/user/auth/…)**非机密**,明文存于 `config.toml`。
//! - **机密**(密码、私钥口令)按 vault id(`user@host:port`)存入加密 vault,永不明文落盘。

use std::sync::Mutex;

use kt_config::{Config, Paths, SessionProfile};
use kt_secrets::{Vault, VaultError};

/// 解锁结果 / Outcome of an unlock or create-vault attempt.
#[derive(Debug)]
pub enum UnlockOutcome {
    /// 成功打开或创建 vault。
    /// Vault opened or created successfully.
    Ok,
    /// vault 文件已存在但未创建过(提示用户去"设置主密码")。
    /// No vault exists yet — prompt the user to set a master password.
    NeedSetup,
    /// 主密码错误或 vault 损坏。
    /// Wrong master password or corrupted vault.
    BadPassword,
    /// 其他错误。
    /// Some other error.
    Error(String),
}

/// 持久化存储。持有路径、已加载配置,以及(解锁后)的 vault。
///
/// Persisted storage: holds paths, the loaded config, and (once unlocked) the vault.
pub struct Store {
    paths: Paths,
    config: Config,
    /// `None` 表示尚未解锁(启动态)。
    /// `None` means locked (startup state).
    vault: Mutex<Option<Vault>>,
}

impl Store {
    /// 发现应用目录并加载 `config.toml`(不存在则用默认空配置)。
    /// Discover app dirs and load `config.toml` (defaults if absent).
    pub fn load() -> anyhow::Result<Self> {
        let paths = Paths::discover().map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let config = Config::load_from(paths.config_file())
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(Self {
            paths,
            config,
            vault: Mutex::new(None),
        })
    }

    /// 配置目录里的各路径(供 GUI 展示等)。
    /// Paths within the app config dir (for GUI display etc.).
    #[allow(dead_code)]
    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// vault 文件是否已存在(用于区分"首次设置"与"解锁")。
    /// Whether a vault file already exists (distinguishes first-run setup from unlock).
    pub fn vault_exists(&self) -> bool {
        self.paths.vault_file().exists()
    }

    /// 是否已解锁(vault 可用)。
    /// Whether the vault is currently unlocked.
    pub fn is_unlocked(&self) -> bool {
        self.vault.lock().unwrap().is_some()
    }

    /// 保存的会话列表(用于侧栏)。
    /// Saved sessions (for the sidebar list).
    pub fn saved_sessions(&self) -> &[SessionProfile] {
        &self.config.sessions
    }

    /// 新增或更新一个会话(按 name 去重),并落盘。
    /// Insert or replace a session by name and persist.
    pub fn save_session(&mut self, profile: SessionProfile) -> anyhow::Result<()> {
        self.config.upsert_session(profile);
        self.flush_config()
    }

    /// 删除指定名称的会话(同时尝试清理其 vault 中的密码),并落盘。
    /// Delete a session by name (and its vault secret if any) and persist.
    pub fn delete_session(&mut self, name: &str) -> anyhow::Result<()> {
        let removed = self
            .config
            .sessions
            .iter()
            .position(|s| s.name == name)
            .map(|i| self.config.sessions.remove(i));
        if let Some(profile) = removed {
            // 清理对应密码(若已解锁)。
            let id = profile.params.effective_vault_id();
            if let Some(vault) = self.vault.lock().unwrap().as_mut() {
                vault.remove(&id);
                if vault.is_dirty() {
                    vault
                        .save()
                        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                }
            }
            self.flush_config()?;
        }
        Ok(())
    }

    /// 按名称查找已保存会话(克隆返回,便于 GUI 使用)。
    /// Find a saved session by name (cloned for GUI use).
    pub fn session_named(&self, name: &str) -> Option<SessionProfile> {
        self.config.session(name).cloned()
    }

    /// 读取机密(已解锁时);未解锁返回 `None`。
    /// Read a secret if unlocked; `None` when locked.
    pub fn get_secret(&self, vault_id: &str) -> Option<String> {
        let guard = self.vault.lock().unwrap();
        guard.as_ref().and_then(|v| v.get(vault_id).map(String::from))
    }

    /// 写入机密(已解锁时)。vault 以 `user@host:port` 为 key。
    /// Write a secret if unlocked, keyed by `user@host:port`.
    pub fn set_secret(&self, vault_id: &str, value: &str) -> anyhow::Result<()> {
        let mut guard = self.vault.lock().unwrap();
        if let Some(vault) = guard.as_mut() {
            vault.set(vault_id, value);
            vault
                .save()
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        }
        Ok(())
    }

    /// 解锁现有 vault。
    /// Unlock an existing vault.
    pub fn unlock(&self, master_password: &str) -> UnlockOutcome {
        if !self.vault_exists() {
            return UnlockOutcome::NeedSetup;
        }
        match Vault::open(self.paths.vault_file(), master_password) {
            Ok(v) => {
                *self.vault.lock().unwrap() = Some(v);
                UnlockOutcome::Ok
            }
            Err(VaultError::BadPasswordOrCorrupt) => UnlockOutcome::BadPassword,
            Err(e) => UnlockOutcome::Error(e.to_string()),
        }
    }

    /// 首次设置主密码并创建 vault。若已存在则视为错误。
    /// Set the master password and create the vault. Errors if it already exists.
    pub fn create_vault(&self, master_password: &str) -> UnlockOutcome {
        match Vault::create(self.paths.vault_file(), master_password) {
            Ok(mut v) => {
                if let Err(e) = v.save() {
                    return UnlockOutcome::Error(e.to_string());
                }
                *self.vault.lock().unwrap() = Some(v);
                UnlockOutcome::Ok
            }
            Err(VaultError::BadPasswordOrCorrupt) => UnlockOutcome::BadPassword,
            Err(e) => UnlockOutcome::Error(e.to_string()),
        }
    }

    /// 将当前配置落盘到 `config.toml`(原子写)。
    /// Persist the current config to `config.toml` (atomic write).
    fn flush_config(&self) -> anyhow::Result<()> {
        self.config
            .save_to(self.paths.config_file())
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }
}
