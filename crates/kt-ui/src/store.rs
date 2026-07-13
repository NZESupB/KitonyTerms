//! Store 桥接层（复用 egui 版本的持久化逻辑）

use kt_config::{
    normalize_group_name, AppSettings, Config, KnownHostCheck, KnownHosts, Paths, SessionProfile,
};
use kt_secrets::Vault;
use rand_core::{OsRng, RngCore};
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
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

const LEGACY_APP_MANAGED_VAULT_PASSWORD: &str = "kitonyterms:app-managed-vault:v1";
const VAULT_KEY_BYTES: usize = 32;

#[cfg(test)]
fn vault_key_file_for(vault_file: &Path) -> PathBuf {
    vault_file.with_file_name("secrets.vault.key")
}

fn load_or_create_vault_key(vault_key_file: &Path) -> anyhow::Result<String> {
    match std::fs::read_to_string(vault_key_file) {
        Ok(contents) => parse_vault_key(&contents).ok_or_else(|| {
            anyhow::anyhow!("本机密码库密钥文件格式无效: {}", vault_key_file.display())
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let key = generate_vault_key();
            match write_new_vault_key(vault_key_file, &key) {
                Ok(()) => Ok(key),
                Err(write_err) if write_err.kind() == std::io::ErrorKind::AlreadyExists => {
                    let contents = std::fs::read_to_string(vault_key_file)?;
                    parse_vault_key(&contents).ok_or_else(|| {
                        anyhow::anyhow!("本机密码库密钥文件格式无效: {}", vault_key_file.display())
                    })
                }
                Err(write_err) => Err(write_err.into()),
            }
        }
        Err(err) => Err(err.into()),
    }
}

fn parse_vault_key(contents: &str) -> Option<String> {
    let key = contents.trim();
    (key.len() == VAULT_KEY_BYTES * 2 && key.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| key.to_ascii_lowercase())
}

fn generate_vault_key() -> String {
    let mut bytes = [0u8; VAULT_KEY_BYTES];
    OsRng.fill_bytes(&mut bytes);
    let mut key = String::with_capacity(VAULT_KEY_BYTES * 2);
    for byte in bytes {
        key.push_str(&format!("{byte:02x}"));
    }
    key
}

fn write_new_vault_key(vault_key_file: &Path, key: &str) -> std::io::Result<()> {
    if let Some(parent) = vault_key_file
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
    }

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(vault_key_file)?;
    file.write_all(key.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

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

    pub fn matches(&self, host: &str, port: u16, fingerprint: &str) -> bool {
        self.host.eq_ignore_ascii_case(host) && self.port == port && self.fingerprint == fingerprint
    }
}

/// Store 包装器：桥接会话配置与加密 vault
pub struct Store {
    config_file: PathBuf,
    known_hosts_file: PathBuf,
    config: Mutex<Config>,
    known_hosts: Mutex<KnownHosts>,
    vault: Mutex<VaultAccess>,
    pending_host_keys: Mutex<VecDeque<PendingHostKey>>,
    temporary_host_keys: Mutex<Vec<PendingHostKey>>,
    status_notices: Mutex<VecDeque<String>>,
}

impl Store {
    pub fn load() -> anyhow::Result<Self> {
        let paths = Paths::discover().map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Self::load_from_files_with_vault_key(
            paths.config_file(),
            paths.vault_file(),
            paths.vault_key_file(),
            paths.known_hosts_file(),
        )
    }

    #[cfg(test)]
    pub(crate) fn load_from_files(
        config_file: PathBuf,
        vault_file: PathBuf,
        known_hosts_file: PathBuf,
    ) -> anyhow::Result<Self> {
        let vault_key_file = vault_key_file_for(&vault_file);
        Self::load_from_files_with_vault_key(
            config_file,
            vault_file,
            vault_key_file,
            known_hosts_file,
        )
    }

    fn load_from_files_with_vault_key(
        config_file: PathBuf,
        vault_file: PathBuf,
        vault_key_file: PathBuf,
        known_hosts_file: PathBuf,
    ) -> anyhow::Result<Self> {
        let config = Config::load_from(&config_file).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let known_hosts =
            KnownHosts::load_from(&known_hosts_file).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let vault = Self::load_app_managed_vault(&vault_file, &vault_key_file);

        Ok(Self {
            config_file,
            known_hosts_file,
            config: Mutex::new(config),
            known_hosts: Mutex::new(known_hosts),
            vault: Mutex::new(vault),
            pending_host_keys: Mutex::new(VecDeque::new()),
            temporary_host_keys: Mutex::new(Vec::new()),
            status_notices: Mutex::new(VecDeque::new()),
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
        self.update_config(move |config| {
            config.settings = settings;
            true
        })
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

    /// 取出下一条需要展示给用户的持久化告警。
    pub fn take_status_notice(&self) -> Option<String> {
        self.status_notices.lock().unwrap().pop_front()
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
            VaultAccess::Unlocked { vault, .. } => vault
                .set_and_save(vault_id, value)
                .map_err(|e| anyhow::anyhow!(e.to_string())),
            VaultAccess::Locked { reason } => Err(anyhow::anyhow!(reason.clone())),
        }
    }

    fn load_app_managed_vault(vault_file: &Path, vault_key_file: &Path) -> VaultAccess {
        let vault_key = match load_or_create_vault_key(vault_key_file) {
            Ok(key) => key,
            Err(err) => {
                return VaultAccess::Locked {
                    reason: format!("无法初始化本机密码库密钥，连接密码不会保存: {err}"),
                };
            }
        };

        match Vault::open_or_create(vault_file, &vault_key) {
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

                if let Ok(vault) = Self::migrate_legacy_app_managed_vault(vault_file, &vault_key) {
                    return VaultAccess::Unlocked {
                        vault,
                        notice: Some(
                            "本机密码库已升级为当前安装独立密钥保护；旧固定密钥不再可用"
                                .to_string(),
                        ),
                    };
                }

                match Self::replace_legacy_vault(vault_file, &vault_key) {
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

    fn migrate_legacy_app_managed_vault(
        vault_file: &Path,
        vault_key: &str,
    ) -> anyhow::Result<Vault> {
        let mut vault = Vault::open(vault_file, LEGACY_APP_MANAGED_VAULT_PASSWORD)
            .map_err(|err| anyhow::anyhow!("旧固定密钥密码库无法打开: {err}"))?;
        vault
            .change_master_password(vault_key)
            .map_err(|err| anyhow::anyhow!("升级本机密码库密钥失败: {err}"))?;
        vault
            .save()
            .map_err(|err| anyhow::anyhow!("保存升级后的本机密码库失败: {err}"))?;
        Ok(vault)
    }

    fn replace_legacy_vault(
        vault_file: &Path,
        vault_key: &str,
    ) -> anyhow::Result<(Vault, PathBuf)> {
        let backup_file = legacy_vault_backup_path(vault_file);
        std::fs::rename(vault_file, &backup_file).map_err(|err| {
            anyhow::anyhow!("备份旧密码库到 {} 失败: {err}", backup_file.display())
        })?;
        let mut vault = Vault::open_or_create(vault_file, vault_key)
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
        let mut known_hosts = self.known_hosts.lock().unwrap();
        let mut next = known_hosts.clone();
        let decision = next.check_or_trust(host, port, fingerprint);
        match decision {
            KnownHostCheck::Trusted => {
                if let Err(err) = next.save_to(&self.known_hosts_file) {
                    self.record_last_seen_warning(host, port, &err.to_string());
                } else {
                    *known_hosts = next;
                }
                Ok(KnownHostCheck::Trusted)
            }
            KnownHostCheck::NewlyTrusted => {
                next.save_to(&self.known_hosts_file)
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                *known_hosts = next;
                Ok(KnownHostCheck::NewlyTrusted)
            }
            decision => Ok(decision),
        }
    }

    /// 只校验主机密钥，不自动写入未知主机。
    pub fn check_host_key(
        &self,
        host: &str,
        port: u16,
        fingerprint: &str,
    ) -> anyhow::Result<KnownHostCheck> {
        let mut known_hosts = self.known_hosts.lock().unwrap();
        let mut next = known_hosts.clone();
        let decision = next.check(host, port, fingerprint);
        if matches!(decision, KnownHostCheck::Trusted) {
            if let Err(err) = next.save_to(&self.known_hosts_file) {
                self.record_last_seen_warning(host, port, &err.to_string());
            } else {
                *known_hosts = next;
            }
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
        let mut known_hosts = self.known_hosts.lock().unwrap();
        let mut next = known_hosts.clone();
        let decision = next.trust(host, port, fingerprint);
        next.save_to(&self.known_hosts_file)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        *known_hosts = next;
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
        let mut pending_host_keys = self.pending_host_keys.lock().unwrap();
        if !pending_host_keys
            .iter()
            .any(|queued| queued.matches(&pending.host, pending.port, &pending.fingerprint))
        {
            pending_host_keys.push_back(pending.clone());
        }
        Some(pending)
    }

    pub fn allow_host_key_once(&self, pending: PendingHostKey) {
        let mut temporary_host_keys = self.temporary_host_keys.lock().unwrap();
        if !temporary_host_keys
            .iter()
            .any(|queued| queued.matches(&pending.host, pending.port, &pending.fingerprint))
        {
            temporary_host_keys.push(pending);
        }
    }

    pub fn peek_pending_host_key(&self) -> Option<PendingHostKey> {
        self.pending_host_keys.lock().unwrap().front().cloned()
    }

    pub fn find_pending_host_key(
        &self,
        host: &str,
        port: u16,
        fingerprint: &str,
    ) -> Option<PendingHostKey> {
        self.pending_host_keys
            .lock()
            .unwrap()
            .iter()
            .find(|pending| pending.matches(host, port, fingerprint))
            .cloned()
    }

    pub fn pop_pending_host_key(
        &self,
        host: &str,
        port: u16,
        fingerprint: &str,
    ) -> Option<PendingHostKey> {
        let mut pending_host_keys = self.pending_host_keys.lock().unwrap();
        let index = pending_host_keys
            .iter()
            .position(|pending| pending.matches(host, port, fingerprint))?;
        pending_host_keys.remove(index)
    }

    pub fn clear_pending_host_key(&self, host: &str, port: u16, fingerprint: &str) -> bool {
        self.pop_pending_host_key(host, port, fingerprint).is_some()
    }

    /// 删除指定名称的会话
    pub fn delete_session(&self, name: &str) -> anyhow::Result<()> {
        self.update_config(|config| {
            config
                .sessions
                .iter()
                .position(|session| session.name == name)
                .map(|index| config.sessions.remove(index))
                .is_some()
        })
    }

    /// 新增分组。
    pub fn add_group(&self, name: &str) -> anyhow::Result<()> {
        self.update_config(|config| {
            config.add_group(name);
            true
        })
    }

    /// 重命名分组，并同步会话引用。
    pub fn rename_group(&self, old_name: &str, new_name: &str) -> anyhow::Result<()> {
        self.update_config(|config| {
            config.rename_group(old_name, new_name);
            true
        })
    }

    /// 将默认分组中的会话迁移到新分组，用于重命名“未显式分组”的默认节点。
    pub fn rename_default_group(&self, new_name: &str) -> anyhow::Result<()> {
        let Some(new_name) = normalize_group_name(new_name) else {
            return Ok(());
        };
        self.update_config(|config| {
            for session in &mut config.sessions {
                if session.group.is_none() {
                    session.group = Some(new_name.clone());
                }
            }
            config.add_group(new_name);
            true
        })
    }

    /// 删除分组；引用该分组的会话会回到默认分组。
    pub fn delete_group(&self, name: &str) -> anyhow::Result<()> {
        self.update_config(|config| {
            config.delete_group(name);
            true
        })
    }

    /// 新增或更新会话（按 name 去重）
    pub fn save_session(&self, mut profile: SessionProfile) -> anyhow::Result<()> {
        profile.group = profile.group.as_deref().and_then(normalize_group_name);
        self.update_config(move |config| {
            if let Some(group) = profile.group.clone() {
                config.add_group(group);
            }
            config.upsert_session(profile);
            true
        })
    }

    fn update_config(&self, update: impl FnOnce(&mut Config) -> bool) -> anyhow::Result<()> {
        let mut config = self.config.lock().unwrap();
        let mut next = config.clone();
        if !update(&mut next) {
            return Ok(());
        }
        next.save_to(&self.config_file)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        *config = next;
        Ok(())
    }

    fn record_last_seen_warning(&self, host: &str, port: u16, error: &str) {
        let message =
            format!("已接受可信主机 {host}:{port}，但更新 known_hosts 最近访问时间失败: {error}");
        tracing::warn!("{message}");
        let mut notices = self.status_notices.lock().unwrap();
        if notices.back() != Some(&message) {
            notices.push_back(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kt_config::DEFAULT_LIGHT_THEME;
    use std::sync::{Arc, Barrier};

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
        assert!(dir.path().join("secrets.vault.key").exists());
        assert_eq!(store.vault_status_message(), None);
        assert_eq!(store.get_secret("root@example.com:22").unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn generated_vault_key_file_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let (dir, _store) = test_store();
        let key_path = dir.path().join("secrets.vault.key");
        let mode = std::fs::metadata(key_path).unwrap().permissions().mode() & 0o777;

        assert_eq!(mode, 0o600);
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
    fn legacy_fixed_key_vault_is_migrated_to_install_key() {
        let dir = tempfile::tempdir().unwrap();
        let vault_path = dir.path().join("secrets.vault");
        let mut legacy_vault =
            Vault::create(&vault_path, LEGACY_APP_MANAGED_VAULT_PASSWORD).unwrap();
        legacy_vault.set("root@example.com:22", "pw");
        legacy_vault.save().unwrap();

        let store = Store::load_from_files(
            dir.path().join("config.toml"),
            vault_path.clone(),
            dir.path().join("known_hosts.toml"),
        )
        .unwrap();

        assert_eq!(store.vault_state(), VaultState::Unlocked);
        assert_eq!(
            store.get_secret("root@example.com:22").unwrap(),
            Some("pw".to_string())
        );
        let message = store.vault_status_message().unwrap();
        assert!(message.contains("独立密钥"));
        assert!(Vault::open(&vault_path, LEGACY_APP_MANAGED_VAULT_PASSWORD).is_err());
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
    fn config_save_failure_keeps_previous_settings_in_memory() {
        let (dir, store) = test_store();
        let original = store.settings();
        std::fs::create_dir(dir.path().join("config.toml")).unwrap();
        let mut changed = original.clone();
        changed.theme = DEFAULT_LIGHT_THEME.to_string();

        assert!(store.update_settings(changed).is_err());
        assert_eq!(store.settings(), original);
    }

    #[test]
    fn vault_save_failure_restores_previous_secret() {
        let (dir, store) = test_store();
        store.set_secret("existing", "old").unwrap();
        std::fs::create_dir(dir.path().join(".secrets.vault.tmp")).unwrap();

        assert!(store.set_secret("existing", "new").is_err());
        assert_eq!(
            store.get_secret("existing").unwrap().as_deref(),
            Some("old")
        );
        assert!(store.set_secret("new-key", "value").is_err());
        assert_eq!(store.get_secret("new-key").unwrap(), None);
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
    fn concurrent_known_host_updates_keep_all_hosts() {
        let (dir, store) = test_store();
        let store = Arc::new(store);
        let barrier = Arc::new(Barrier::new(8));
        let threads = (0..8)
            .map(|index| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    store
                        .trust_host_key(
                            &format!("host-{index}.example.com"),
                            22,
                            &format!("SHA256:key-{index}"),
                        )
                        .unwrap();
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap();
        }

        let known_hosts = KnownHosts::load_from(dir.path().join("known_hosts.toml")).unwrap();
        assert_eq!(known_hosts.hosts.len(), 8);
    }

    #[test]
    fn trusted_host_is_accepted_when_last_seen_save_fails_with_notice() {
        let dir = tempfile::tempdir().unwrap();
        let known_hosts_path = dir.path().join("known_hosts.toml");
        let mut known_hosts = KnownHosts::default();
        known_hosts.trust("example.com", 22, "SHA256:key");
        known_hosts.save_to(&known_hosts_path).unwrap();
        let store = Store::load_from_files(
            dir.path().join("config.toml"),
            dir.path().join("secrets.vault"),
            known_hosts_path.clone(),
        )
        .unwrap();
        std::fs::remove_file(&known_hosts_path).unwrap();
        std::fs::create_dir(&known_hosts_path).unwrap();

        assert_eq!(
            store
                .check_host_key("example.com", 22, "SHA256:key")
                .unwrap(),
            KnownHostCheck::Trusted
        );
        let notice = store.take_status_notice().unwrap();
        assert!(notice.contains("已接受可信主机"));
        assert!(notice.contains("最近访问时间失败"));
    }

    #[test]
    fn new_host_is_not_trusted_when_persistence_fails() {
        let (dir, store) = test_store();
        let known_hosts_path = dir.path().join("known_hosts.toml");
        std::fs::create_dir(&known_hosts_path).unwrap();

        assert!(store
            .trust_host_key("example.com", 22, "SHA256:key")
            .is_err());
        assert_eq!(
            store
                .check_host_key("example.com", 22, "SHA256:key")
                .unwrap(),
            KnownHostCheck::Unknown {
                fingerprint: "SHA256:key".to_string()
            }
        );
    }

    #[test]
    fn pending_host_key_queue_deduplicates_and_removes_exact_items() {
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
        assert_eq!(store.peek_pending_host_key(), Some(unknown.clone()));

        store.record_pending_host_key(
            "EXAMPLE.com",
            22,
            KnownHostCheck::Unknown {
                fingerprint: "SHA256:first".to_string(),
            },
        );

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
        assert_eq!(
            store.find_pending_host_key("example.com", 22, "SHA256:second"),
            Some(changed.clone())
        );
        assert_eq!(
            store.pop_pending_host_key("example.com", 22, "SHA256:first"),
            Some(unknown)
        );
        assert_eq!(store.peek_pending_host_key(), Some(changed.clone()));
        assert!(store.clear_pending_host_key("example.com", 22, "SHA256:second"));
        assert!(!store.clear_pending_host_key("example.com", 22, "SHA256:second"));
        assert_eq!(store.peek_pending_host_key(), None);
    }

    #[test]
    fn removing_one_unknown_host_keeps_the_next_prompt_queued() {
        let (_dir, store) = test_store();
        store.record_pending_host_key(
            "alpha.example.com",
            22,
            KnownHostCheck::Unknown {
                fingerprint: "SHA256:alpha".to_string(),
            },
        );
        let beta = store
            .record_pending_host_key(
                "beta.example.com",
                2222,
                KnownHostCheck::Unknown {
                    fingerprint: "SHA256:beta".to_string(),
                },
            )
            .unwrap();

        assert!(store.clear_pending_host_key("alpha.example.com", 22, "SHA256:alpha"));
        assert_eq!(store.peek_pending_host_key(), Some(beta));
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
