//! Store 桥接层（复用 egui 版本的持久化逻辑）

use kt_config::{normalize_group_name, AppSettings, Config, Paths, SessionProfile};
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
        let config =
            Config::load_from(paths.config_file()).map_err(|e| anyhow::anyhow!(e.to_string()))?;

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

    /// 获取已保存分组列表（包含旧配置中会话引用的分组）。
    pub fn saved_groups(&self) -> Vec<String> {
        self.config.lock().unwrap().group_names()
    }

    /// 获取应用设置（克隆返回，避免 UI 持有锁）。
    pub fn settings(&self) -> AppSettings {
        self.config.lock().unwrap().settings.clone()
    }

    /// 更新并持久化应用设置。
    pub fn update_settings(&self, settings: AppSettings) -> anyhow::Result<()> {
        let mut config = self.config.lock().unwrap();
        config.settings = settings;
        config
            .save_to(self.paths.config_file())
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    pub fn get_secret(&self, vault_id: &str) -> Option<String> {
        let guard = self.vault.lock().unwrap();
        guard
            .as_ref()
            .and_then(|v| v.get(vault_id).map(String::from))
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

    /// 新增分组。
    pub fn add_group(&self, name: &str) -> anyhow::Result<()> {
        let mut config = self.config.lock().unwrap();
        config.add_group(name);
        config
            .save_to(self.paths.config_file())
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    /// 重命名分组，并同步会话引用。
    pub fn rename_group(&self, old_name: &str, new_name: &str) -> anyhow::Result<()> {
        let mut config = self.config.lock().unwrap();
        config.rename_group(old_name, new_name);
        config
            .save_to(self.paths.config_file())
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    /// 将默认分组中的会话迁移到新分组，用于重命名“未显式分组”的默认节点。
    pub fn rename_default_group(&self, new_name: &str) -> anyhow::Result<()> {
        let Some(new_name) = normalize_group_name(new_name) else {
            return Ok(());
        };
        let mut config = self.config.lock().unwrap();
        for session in &mut config.sessions {
            if session.group.is_none() {
                session.group = Some(new_name.clone());
            }
        }
        config.add_group(new_name);
        config
            .save_to(self.paths.config_file())
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    /// 删除分组；引用该分组的会话会回到默认分组。
    pub fn delete_group(&self, name: &str) -> anyhow::Result<()> {
        let mut config = self.config.lock().unwrap();
        config.delete_group(name);
        config
            .save_to(self.paths.config_file())
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    /// 新增或更新会话（按 name 去重）
    pub fn save_session(&self, mut profile: SessionProfile) -> anyhow::Result<()> {
        let mut config = self.config.lock().unwrap();
        profile.group = profile.group.as_deref().and_then(normalize_group_name);
        if let Some(group) = profile.group.clone() {
            config.add_group(group);
        }
        config.upsert_session(profile);
        config
            .save_to(self.paths.config_file())
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }
}
