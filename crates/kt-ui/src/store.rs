//! Store 桥接层（复用 egui 版本的持久化逻辑）

use kt_config::{Config, Paths, SessionProfile};
use kt_secrets::Vault;
use std::sync::Mutex;

/// Store 包装器：桥接会话配置与加密 vault
pub struct Store {
    paths: Paths,
    config: Mutex<Config>,
    vault: Mutex<Option<Vault>>,
}

impl Store {
    pub fn load() -> anyhow::Result<Self> {
        let paths = Paths::discover().map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let config = Config::load_from(paths.config_file())
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;

        // 尝试用空密码自动解锁 vault
        let vault = if paths.vault_file().exists() {
            Vault::open(paths.vault_file(), "").ok()
        } else {
            None
        };

        Ok(Self {
            paths,
            config: Mutex::new(config),
            vault: Mutex::new(vault),
        })
    }

    /// 获取已保存会话列表（克隆返回，避免借用冲突）
    pub fn saved_sessions(&self) -> Vec<SessionProfile> {
        self.config.lock().unwrap().sessions.clone()
    }

    pub fn get_secret(&self, vault_id: &str) -> Option<String> {
        let guard = self.vault.lock().unwrap();
        guard.as_ref().and_then(|v| v.get(vault_id).map(String::from))
    }

    /// 写入机密（已解锁时）。vault 以 `user@host:port` 为 key。
    pub fn set_secret(&self, vault_id: &str, value: &str) -> anyhow::Result<()> {
        let mut guard = self.vault.lock().unwrap();
        if let Some(vault) = guard.as_mut() {
            vault.set(vault_id, value);
            vault.save().map_err(|e| anyhow::anyhow!(e.to_string()))?;
        }
        Ok(())
    }

    /// 删除指定名称的会话
    pub fn delete_session(&self, name: &str) -> anyhow::Result<()> {
        let mut config = self.config.lock().unwrap();
        let removed = config
            .sessions
            .iter()
            .position(|s| s.name == name)
            .map(|i| config.sessions.remove(i));

        if removed.is_some() {
            config
                .save_to(self.paths.config_file())
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        }

        Ok(())
    }

    /// 新增或更新会话（按 name 去重）
    pub fn save_session(&self, profile: SessionProfile) -> anyhow::Result<()> {
        let mut config = self.config.lock().unwrap();
        config.upsert_session(profile);
        config
            .save_to(self.paths.config_file())
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }
}
