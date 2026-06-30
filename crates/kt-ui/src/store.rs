//! Store 桥接层（复用 egui 版本的持久化逻辑）

use kt_config::{
    normalize_group_name, AppSettings, Config, KnownHostCheck, KnownHosts, Paths, SessionProfile,
};
use kt_secrets::Vault;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultState {
    Locked,
    Unlocked,
}

enum VaultAccess {
    Locked {
        reason: String,
    },
    Unlocked {
        vault: Vault,
        notice: Option<String>,
    },
}

const APP_MANAGED_VAULT_PASSWORD: &str = "kitonyterms:app-managed-vault:v1";

fn legacy_vault_backup_path(vault_file: &Path) -> PathBuf {
    let file_name = vault_file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "secrets.vault".to_string());
    for index in 0.. {
        let suffix = if index == 0 {
            "legacy".to_string()
        } else {
            format!("legacy.{index}")
        };
        let candidate = vault_file.with_file_name(format!("{file_name}.{suffix}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("无限递增的备份序号总能找到可用路径")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingHostKey {
    pub host: String,
    pub port: u16,
    pub fingerprint: String,
    pub expected: Option<String>,
}

impl PendingHostKey {
    fn unknown(host: &str, port: u16, fingerprint: String) -> Self {
        Self {
            host: host.to_string(),
            port,
            fingerprint,
            expected: None,
        }
    }

    fn changed(host: &str, port: u16, expected: String, actual: String) -> Self {
        Self {
            host: host.to_string(),
            port,
            fingerprint: actual,
            expected: Some(expected),
        }
    }

    pub fn is_changed(&self) -> bool {
        self.expected.is_some()
    }
}

/// Store 包装器：桥接会话配置与加密 vault
pub struct Store {
    config_file: PathBuf,
    known_hosts_file: PathBuf,
    config: Mutex<Config>,
    vault: Mutex<VaultAccess>,
    pending_host_key: Mutex<Option<PendingHostKey>>,
    temporary_host_keys: Mutex<Vec<PendingHostKey>>,
}

impl Store {
    pub fn load() -> anyhow::Result<Self> {
        let paths = Paths::discover().map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Self::load_from_files(
            paths.config_file(),
            paths.vault_file(),
            paths.known_hosts_file(),
        )
    }

    pub(crate) fn load_from_files(
        config_file: PathBuf,
        vault_file: PathBuf,
        known_hosts_file: PathBuf,
    ) -> anyhow::Result<Self> {
        let config = Config::load_from(&config_file).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let vault = Self::load_app_managed_vault(&vault_file);

        Ok(Self {
            config_file,
            known_hosts_file,
            config: Mutex::new(config),
            vault: Mutex::new(vault),
            pending_host_key: Mutex::new(None),
            temporary_host_keys: Mutex::new(Vec::new()),
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
            .save_to(&self.config_file)
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    pub fn vault_state(&self) -> VaultState {
        match &*self.vault.lock().unwrap() {
            VaultAccess::Locked { .. } => VaultState::Locked,
            VaultAccess::Unlocked { .. } => VaultState::Unlocked,
        }
    }

    /// 返回 vault 状态提示；正常自动打开时返回 `None`。
    pub fn vault_status_message(&self) -> Option<String> {
        match &*self.vault.lock().unwrap() {
            VaultAccess::Locked { reason } => Some(reason.clone()),
            VaultAccess::Unlocked { notice, .. } => notice.clone(),
        }
    }

    pub fn get_secret(&self, vault_id: &str) -> anyhow::Result<Option<String>> {
        let guard = self.vault.lock().unwrap();
        match &*guard {
            VaultAccess::Unlocked { vault, .. } => Ok(vault.get(vault_id).map(String::from)),
            VaultAccess::Locked { reason } => Err(anyhow::anyhow!(reason.clone())),
        }
    }

    /// 写入机密（已解锁时）。vault 以 `user@host:port` 为 key。
    pub fn set_secret(&self, vault_id: &str, value: &str) -> anyhow::Result<()> {
        let mut guard = self.vault.lock().unwrap();
        match &mut *guard {
            VaultAccess::Unlocked { vault, .. } => {
                vault.set(vault_id, value);
                vault.save().map_err(|e| anyhow::anyhow!(e.to_string()))?;
                Ok(())
            }
            VaultAccess::Locked { reason } => Err(anyhow::anyhow!(reason.clone())),
        }
    }

    fn load_app_managed_vault(vault_file: &Path) -> VaultAccess {
        match Vault::open_or_create(vault_file, APP_MANAGED_VAULT_PASSWORD) {
            Ok(mut vault) => {
                if vault.is_dirty() {
                    if let Err(err) = vault.save() {
                        return VaultAccess::Locked {
                            reason: format!("无法初始化本机密码库，连接密码不会保存: {err}"),
                        };
                    }
                }
                VaultAccess::Unlocked {
                    vault,
                    notice: None,
                }
            }
            Err(err) => {
                if !vault_file.exists() {
                    return VaultAccess::Locked {
                        reason: format!("无法初始化本机密码库，连接密码不会保存: {err}"),
                    };
                }

                match Self::replace_legacy_vault(vault_file) {
                    Ok((vault, backup_file)) => {
                        VaultAccess::Unlocked {
                            vault,
                            notice: Some(format!(
                                "旧版密码库无法自动打开，已备份到 {} 并创建新的本机密码库；旧保存密码暂不可用",
                                backup_file.display()
                            )),
                        }
                    }
                    Err(replace_err) => VaultAccess::Locked {
                        reason: format!(
                            "无法自动打开或重建本机密码库，连接密码不会保存: {err}; {replace_err}"
                        ),
                    },
                }
            }
        }
    }

    fn replace_legacy_vault(vault_file: &Path) -> anyhow::Result<(Vault, PathBuf)> {
        let backup_file = legacy_vault_backup_path(vault_file);
        std::fs::rename(vault_file, &backup_file).map_err(|err| {
            anyhow::anyhow!("备份旧密码库到 {} 失败: {err}", backup_file.display())
        })?;
        let mut vault = Vault::open_or_create(vault_file, APP_MANAGED_VAULT_PASSWORD)
            .map_err(|err| anyhow::anyhow!("创建新的本机密码库失败: {err}"))?;
        vault
            .save()
            .map_err(|err| anyhow::anyhow!("保存新的本机密码库失败: {err}"))?;
        Ok((vault, backup_file))
    }

    /// 校验或首次信任主机密钥指纹。
    pub fn check_or_trust_host_key(
        &self,
        host: &str,
        port: u16,
        fingerprint: &str,
    ) -> anyhow::Result<KnownHostCheck> {
        let mut known_hosts = KnownHosts::load_from(&self.known_hosts_file)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let decision = known_hosts.check_or_trust(host, port, fingerprint);
        if matches!(
            decision,
            KnownHostCheck::Trusted | KnownHostCheck::NewlyTrusted
        ) {
            known_hosts
                .save_to(&self.known_hosts_file)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        }
        Ok(decision)
    }

    /// 只校验主机密钥，不自动写入未知主机。
    pub fn check_host_key(
        &self,
        host: &str,
        port: u16,
        fingerprint: &str,
    ) -> anyhow::Result<KnownHostCheck> {
        let mut known_hosts = KnownHosts::load_from(&self.known_hosts_file)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let decision = known_hosts.check(host, port, fingerprint);
        if matches!(decision, KnownHostCheck::Trusted) {
            known_hosts
                .save_to(&self.known_hosts_file)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        }
        Ok(decision)
    }

    /// 消费一次性允许的主机密钥。该决策只存在内存中，不写入 known_hosts。
    pub fn consume_temporary_host_key(&self, host: &str, port: u16, fingerprint: &str) -> bool {
        let mut guard = self.temporary_host_keys.lock().unwrap();
        if let Some(index) = guard
            .iter()
            .position(|key| key.host == host && key.port == port && key.fingerprint == fingerprint)
        {
            guard.remove(index);
            true
        } else {
            false
        }
    }

    /// 在用户确认后信任主机密钥。
    pub fn trust_host_key(
        &self,
        host: &str,
        port: u16,
        fingerprint: &str,
    ) -> anyhow::Result<KnownHostCheck> {
        let mut known_hosts = KnownHosts::load_from(&self.known_hosts_file)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let decision = known_hosts.trust(host, port, fingerprint);
        known_hosts
            .save_to(&self.known_hosts_file)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(decision)
    }

    pub fn record_pending_host_key(
        &self,
        host: &str,
        port: u16,
        decision: KnownHostCheck,
    ) -> Option<PendingHostKey> {
        let pending = match decision {
            KnownHostCheck::Unknown { fingerprint } => {
                PendingHostKey::unknown(host, port, fingerprint)
            }
            KnownHostCheck::Changed { expected, actual } => {
                PendingHostKey::changed(host, port, expected, actual)
            }
            KnownHostCheck::Trusted | KnownHostCheck::NewlyTrusted => return None,
        };
        *self.pending_host_key.lock().unwrap() = Some(pending.clone());
        Some(pending)
    }

    pub fn allow_host_key_once(&self, pending: PendingHostKey) {
        self.temporary_host_keys.lock().unwrap().push(pending);
    }

    pub fn pending_host_key(&self) -> Option<PendingHostKey> {
        self.pending_host_key.lock().unwrap().clone()
    }

    pub fn clear_pending_host_key(&self) {
        *self.pending_host_key.lock().unwrap() = None;
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
                .save_to(&self.config_file)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        }

        Ok(())
    }

    /// 新增分组。
    pub fn add_group(&self, name: &str) -> anyhow::Result<()> {
        let mut config = self.config.lock().unwrap();
        config.add_group(name);
        config
            .save_to(&self.config_file)
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    /// 重命名分组，并同步会话引用。
    pub fn rename_group(&self, old_name: &str, new_name: &str) -> anyhow::Result<()> {
        let mut config = self.config.lock().unwrap();
        config.rename_group(old_name, new_name);
        config
            .save_to(&self.config_file)
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
            .save_to(&self.config_file)
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }

    /// 删除分组；引用该分组的会话会回到默认分组。
    pub fn delete_group(&self, name: &str) -> anyhow::Result<()> {
        let mut config = self.config.lock().unwrap();
        config.delete_group(name);
        config
            .save_to(&self.config_file)
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
            .save_to(&self.config_file)
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kt_config::DEFAULT_LIGHT_THEME;

    fn test_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::load_from_files(
            dir.path().join("config.toml"),
            dir.path().join("secrets.vault"),
            dir.path().join("known_hosts.toml"),
        )
        .unwrap();
        (dir, store)
    }

    #[test]
    fn missing_vault_is_created_and_unlocked_automatically() {
        let (dir, store) = test_store();

        assert_eq!(store.vault_state(), VaultState::Unlocked);
        assert!(dir.path().join("secrets.vault").exists());
        assert_eq!(store.vault_status_message(), None);
        assert_eq!(store.get_secret("root@example.com:22").unwrap(), None);
    }

    #[test]
    fn app_managed_vault_reopens_and_reads_saved_password() {
        let (dir, store) = test_store();
        store.set_secret("root@example.com:22", "pw").unwrap();

        let reloaded = Store::load_from_files(
            dir.path().join("config.toml"),
            dir.path().join("secrets.vault"),
            dir.path().join("known_hosts.toml"),
        )
        .unwrap();

        assert_eq!(reloaded.vault_state(), VaultState::Unlocked);
        assert_eq!(
            reloaded.get_secret("root@example.com:22").unwrap(),
            Some("pw".to_string())
        );
    }

    #[test]
    fn legacy_master_password_vault_is_backed_up_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let vault_path = dir.path().join("secrets.vault");
        let mut legacy_vault = Vault::create(&vault_path, "legacy-master").unwrap();
        legacy_vault.set("root@example.com:22", "pw");
        legacy_vault.save().unwrap();
        let backup_path = legacy_vault_backup_path(&vault_path);

        let store = Store::load_from_files(
            dir.path().join("config.toml"),
            vault_path.clone(),
            dir.path().join("known_hosts.toml"),
        )
        .unwrap();

        assert_eq!(store.vault_state(), VaultState::Unlocked);
        assert!(backup_path.exists());
        let message = store.vault_status_message().unwrap();
        assert!(message.contains("已备份"));
        assert!(message.contains("旧保存密码暂不可用"));
        assert_eq!(store.get_secret("root@example.com:22").unwrap(), None);
        store.set_secret("root@example.com:22", "new").unwrap();
        assert_eq!(
            store.get_secret("root@example.com:22").unwrap(),
            Some("new".to_string())
        );
    }

    #[test]
    fn legacy_vault_backup_path_does_not_overwrite_existing_backup() {
        let dir = tempfile::tempdir().unwrap();
        let vault_path = dir.path().join("secrets.vault");
        std::fs::write(dir.path().join("secrets.vault.legacy"), "old").unwrap();

        assert_eq!(
            legacy_vault_backup_path(&vault_path),
            dir.path().join("secrets.vault.legacy.1")
        );
    }

    #[test]
    fn update_settings_persists_theme() {
        let (dir, store) = test_store();
        let mut settings = store.settings();
        settings.theme = DEFAULT_LIGHT_THEME.to_string();

        store.update_settings(settings).unwrap();

        let reloaded = Store::load_from_files(
            dir.path().join("config.toml"),
            dir.path().join("secrets.vault"),
            dir.path().join("known_hosts.toml"),
        )
        .unwrap();
        assert_eq!(reloaded.settings().theme, DEFAULT_LIGHT_THEME);
    }

    #[test]
    fn trusted_known_host_updates_last_seen_on_disk() {
        let (dir, store) = test_store();
        let known_hosts_path = dir.path().join("known_hosts.toml");

        let first = store
            .check_or_trust_host_key("example.com", 22, "SHA256:key")
            .unwrap();
        assert_eq!(first, KnownHostCheck::NewlyTrusted);

        let mut known_hosts = KnownHosts::load_from(&known_hosts_path).unwrap();
        if let Some(entry) = known_hosts.hosts.first_mut() {
            entry.last_seen_unix = 0;
        }
        known_hosts.save_to(&known_hosts_path).unwrap();

        let second = store
            .check_or_trust_host_key("example.com", 22, "SHA256:key")
            .unwrap();
        assert_eq!(second, KnownHostCheck::Trusted);

        let reloaded = KnownHosts::load_from(&known_hosts_path).unwrap();
        assert_ne!(reloaded.hosts[0].last_seen_unix, 0);
    }

    #[test]
    fn host_key_check_does_not_persist_unknown_before_trust() {
        let (dir, store) = test_store();
        let known_hosts_path = dir.path().join("known_hosts.toml");

        let check = store
            .check_host_key("example.com", 22, "SHA256:key")
            .unwrap();
        assert_eq!(
            check,
            KnownHostCheck::Unknown {
                fingerprint: "SHA256:key".to_string()
            }
        );
        assert!(!known_hosts_path.exists());

        let trusted = store
            .trust_host_key("example.com", 22, "SHA256:key")
            .unwrap();
        assert_eq!(trusted, KnownHostCheck::NewlyTrusted);

        let reloaded = KnownHosts::load_from(&known_hosts_path).unwrap();
        assert_eq!(reloaded.hosts.len(), 1);
        assert_eq!(reloaded.hosts[0].fingerprint, "SHA256:key");
    }

    #[test]
    fn pending_host_key_records_unknown_and_changed_decisions() {
        let (_dir, store) = test_store();

        let unknown = store
            .record_pending_host_key(
                "example.com",
                22,
                KnownHostCheck::Unknown {
                    fingerprint: "SHA256:first".to_string(),
                },
            )
            .unwrap();
        assert_eq!(unknown.host, "example.com");
        assert_eq!(unknown.fingerprint, "SHA256:first");
        assert!(!unknown.is_changed());
        assert_eq!(store.pending_host_key(), Some(unknown));

        let changed = store
            .record_pending_host_key(
                "example.com",
                22,
                KnownHostCheck::Changed {
                    expected: "SHA256:first".to_string(),
                    actual: "SHA256:second".to_string(),
                },
            )
            .unwrap();
        assert!(changed.is_changed());
        assert_eq!(changed.expected.as_deref(), Some("SHA256:first"));
        assert_eq!(changed.fingerprint, "SHA256:second");
        assert_eq!(store.pending_host_key(), Some(changed));

        store.clear_pending_host_key();
        assert_eq!(store.pending_host_key(), None);
    }

    #[test]
    fn temporary_host_key_allowance_is_consumed_once_without_persisting() {
        let (dir, store) = test_store();
        let known_hosts_path = dir.path().join("known_hosts.toml");

        let pending = PendingHostKey::unknown("example.com", 22, "SHA256:temp".to_string());
        store.allow_host_key_once(pending);

        assert!(store.consume_temporary_host_key("example.com", 22, "SHA256:temp"));
        assert!(!store.consume_temporary_host_key("example.com", 22, "SHA256:temp"));
        assert!(!known_hosts_path.exists());
    }
}
