//! KitonyTerms configuration: connection parameters, session profiles, app
//! settings, and `~/.ssh/config` integration.
//!
//! Everything here is UI-agnostic and serializable so it can be shared between
//! the headless example, the core engine, and the GUI.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(not(target_os = "android"))]
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

mod ssh_config;
pub use ssh_config::{lookup_ssh_config, SshConfigHost};

/// Errors for config loading/saving.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not determine a config directory for this platform")]
    NoConfigDir,

    #[error("failed to resolve platform app directory: {0}")]
    Platform(String),

    #[error("failed to parse config: {0}")]
    Parse(String),

    #[error("failed to serialize config: {0}")]
    Serialize(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

type Result<T> = std::result::Result<T, ConfigError>;

static NEXT_TEMP_FILE_ID: AtomicU64 = AtomicU64::new(1);

fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;

    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config".to_string());
    let temp_path = loop {
        let id = NEXT_TEMP_FILE_ID.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{file_name}.{}.{}.tmp", std::process::id(), id));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                let write_result = (|| -> std::io::Result<()> {
                    file.write_all(contents)?;
                    file.sync_all()
                })();
                if let Err(err) = write_result {
                    drop(file);
                    let _ = std::fs::remove_file(&candidate);
                    return Err(err.into());
                }
                drop(file);
                break candidate;
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.into()),
        }
    };

    if let Err(err) = replace_file(&temp_path, path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(err.into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::rename(source, target)
}

#[cfg(windows)]
fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING};

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let target = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_REPLACE_EXISTING) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// How to authenticate an SSH connection.
///
/// Note: secret *values* (passwords, passphrases) are never stored here — they
/// live in the encrypted vault (`kt-secrets`). This enum only describes the
/// *method* and where to find non-secret inputs (e.g. a key file path).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthMethod {
    /// Prompt for / supply a password interactively.
    #[default]
    Password,
    /// Public-key auth using a private key file. The optional passphrase is
    /// looked up in the vault under [`ConnectParams::vault_id`].
    PublicKey {
        /// Path to the private key file (e.g. `~/.ssh/id_ed25519`).
        key_path: PathBuf,
    },
    /// Keyboard-interactive (server-driven prompts).
    KeyboardInteractive,
    /// Use the system ssh-agent for public-key authentication.
    Agent,
}

/// TCP-layer proxy used to reach the SSH server, applied before the SSH
/// handshake. This is orthogonal to [`ConnectParams::proxy_jump`] (an SSH
/// bastion): the proxy tunnels the raw TCP connection, while ProxyJump opens a
/// nested SSH session.
///
/// Secret values (proxy credentials) are never stored here — only host/port
/// and non-secret metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProxyConfig {
    /// Connect directly, no proxy.
    #[default]
    Direct,
    /// Use the operating system / environment proxy settings
    /// (`ALL_PROXY`/`HTTPS_PROXY`/`HTTP_PROXY`/`SOCKS_PROXY` and, on desktop
    /// platforms, the system network proxy).
    System,
    /// SOCKS5 proxy at `host:port`, optional username auth.
    Socks5 {
        host: String,
        port: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        username: Option<String>,
    },
    /// HTTP CONNECT proxy at `host:port`, optional username auth.
    Http {
        host: String,
        port: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        username: Option<String>,
    },
}

impl ProxyConfig {
    /// True when this config actually routes through a proxy.
    pub fn is_direct(&self) -> bool {
        matches!(self, ProxyConfig::Direct)
    }
}

/// Everything needed to establish one SSH connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectParams {
    /// Hostname or IP to connect to.
    pub host: String,
    /// TCP port.
    #[serde(default = "default_port")]
    pub port: u16,
    /// Login username.
    pub user: String,
    /// Ordered list of auth methods to attempt.
    #[serde(default)]
    pub auth: Vec<AuthMethod>,
    /// Stable id used to namespace secrets for this connection in the vault.
    /// Defaults to `user@host:port` when not set.
    #[serde(default)]
    pub vault_id: Option<String>,
    /// Optional single-hop ProxyJump target, formatted as `[user@]host[:port]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_jump: Option<String>,
    /// TCP-layer proxy applied before the SSH handshake (system/SOCKS5/HTTP).
    #[serde(default, skip_serializing_if = "ProxyConfig::is_direct")]
    pub proxy: ProxyConfig,
    /// Request OpenSSH agent forwarding for the interactive shell.
    #[serde(default)]
    pub forward_agent: bool,
}

fn default_port() -> u16 {
    22
}

impl ConnectParams {
    /// Construct minimal params with sane defaults (password auth).
    pub fn new(host: impl Into<String>, user: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            port: 22,
            user: user.into(),
            auth: vec![AuthMethod::Password],
            vault_id: None,
            proxy_jump: None,
            proxy: ProxyConfig::Direct,
            forward_agent: false,
        }
    }

    /// The vault namespace id for this connection's secrets.
    pub fn effective_vault_id(&self) -> String {
        self.vault_id
            .clone()
            .unwrap_or_else(|| format!("{}@{}:{}", self.user, self.host, self.port))
    }

    /// Merge values from a matching `~/.ssh/config` host block. Explicit values
    /// already present on `self` win; ssh_config only fills the gaps. Identity
    /// files from ssh_config are appended as `PublicKey` auth methods.
    pub fn merge_ssh_config(&mut self, cfg: &SshConfigHost) {
        if let Some(hostname) = &cfg.hostname {
            // Only override host if the user typed an alias (host == queried alias).
            if self.host == cfg.alias {
                self.host = hostname.clone();
            }
        }
        if let Some(port) = cfg.port {
            if self.port == default_port() {
                self.port = port;
            }
        }
        if self.user.is_empty() {
            if let Some(user) = &cfg.user {
                self.user = user.clone();
            }
        }
        for key in &cfg.identity_files {
            let method = AuthMethod::PublicKey {
                key_path: key.clone(),
            };
            if !self.auth.contains(&method) {
                // Prefer key auth ahead of password when ssh_config specifies one.
                self.auth.insert(0, method);
            }
        }
        if self.proxy_jump.is_none() {
            self.proxy_jump = cfg
                .proxy_jump
                .as_deref()
                .map(str::trim)
                .filter(|proxy_jump| !proxy_jump.eq_ignore_ascii_case("none"))
                .filter(|proxy_jump| !proxy_jump.is_empty())
                .map(str::to_string);
        }
    }
}

/// Result of checking a host key against the local trust store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnownHostCheck {
    /// Host key matches the stored fingerprint.
    Trusted,
    /// Host is unknown and should be confirmed before it is persisted.
    Unknown { fingerprint: String },
    /// Host was unknown and has just been persisted with this fingerprint.
    NewlyTrusted,
    /// Host key changed and must be rejected.
    Changed { expected: String, actual: String },
}

/// One persisted known-host entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownHostEntry {
    pub host: String,
    pub port: u16,
    pub fingerprint: String,
    #[serde(default)]
    pub first_seen_unix: u64,
    #[serde(default)]
    pub last_seen_unix: u64,
}

/// Minimal known_hosts-style trust store.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownHosts {
    #[serde(default)]
    pub hosts: Vec<KnownHostEntry>,
}

impl KnownHosts {
    pub fn load_from(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        match std::fs::read_to_string(path) {
            Ok(s) => toml::from_str(&s).map_err(|e| ConfigError::Parse(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(ConfigError::Io(e)),
        }
    }

    pub fn save_to(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let toml =
            toml::to_string_pretty(self).map_err(|e| ConfigError::Serialize(e.to_string()))?;
        atomic_write(path, toml.as_bytes())
    }

    pub fn check_or_trust(
        &mut self,
        host: impl AsRef<str>,
        port: u16,
        fingerprint: impl Into<String>,
    ) -> KnownHostCheck {
        let host = host.as_ref().to_string();
        let fingerprint = fingerprint.into();
        match self.check(&host, port, fingerprint.clone()) {
            KnownHostCheck::Unknown { .. } => self.trust(host, port, fingerprint),
            other => other,
        }
    }

    pub fn check(
        &mut self,
        host: impl AsRef<str>,
        port: u16,
        fingerprint: impl Into<String>,
    ) -> KnownHostCheck {
        let host = normalize_known_host(host.as_ref());
        let fingerprint = fingerprint.into();
        let now = unix_now();

        if let Some(entry) = self
            .hosts
            .iter_mut()
            .find(|entry| entry.host == host && entry.port == port)
        {
            if entry.fingerprint == fingerprint {
                entry.last_seen_unix = now;
                KnownHostCheck::Trusted
            } else {
                KnownHostCheck::Changed {
                    expected: entry.fingerprint.clone(),
                    actual: fingerprint,
                }
            }
        } else {
            KnownHostCheck::Unknown { fingerprint }
        }
    }

    pub fn trust(
        &mut self,
        host: impl AsRef<str>,
        port: u16,
        fingerprint: impl Into<String>,
    ) -> KnownHostCheck {
        let host = normalize_known_host(host.as_ref());
        let fingerprint = fingerprint.into();
        let now = unix_now();

        if let Some(entry) = self
            .hosts
            .iter_mut()
            .find(|entry| entry.host == host && entry.port == port)
        {
            entry.fingerprint = fingerprint;
            entry.last_seen_unix = now;
            if entry.first_seen_unix == 0 {
                entry.first_seen_unix = now;
            }
        } else {
            self.hosts.push(KnownHostEntry {
                host,
                port,
                fingerprint,
                first_seen_unix: now,
                last_seen_unix: now,
            });
        }
        self.hosts.sort_by(|a, b| {
            a.host
                .cmp(&b.host)
                .then_with(|| a.port.cmp(&b.port))
                .then_with(|| a.fingerprint.cmp(&b.fingerprint))
        });
        KnownHostCheck::NewlyTrusted
    }
}

fn normalize_known_host(host: &str) -> String {
    host.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// A saved, named connection the user can reconnect to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionProfile {
    /// Display name in the UI / session list.
    pub name: String,
    /// 可选分组(侧栏文件夹树)。Optional group/folder for the sidebar tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(flatten)]
    pub params: ConnectParams,
}

/// Cursor rendering style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CursorStyle {
    #[default]
    Block,
    Bar,
    Underline,
}

/// Application display language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppLanguage {
    English,
    Chinese,
}

impl Default for AppLanguage {
    fn default() -> Self {
        Self::system_default()
    }
}

impl AppLanguage {
    pub fn system_default() -> Self {
        system_locale_language().unwrap_or_else(env_locale_language)
    }

    fn from_locale_tag(tag: &str) -> Self {
        let normalized = tag.to_ascii_lowercase().replace('_', "-");
        if normalized.starts_with("zh") {
            Self::Chinese
        } else {
            Self::English
        }
    }
}

/// Visual / behavioral app settings.
pub const DEFAULT_DARK_THEME: &str = "default-dark";
pub const DEFAULT_LIGHT_THEME: &str = "default-light";

pub fn normalize_theme_name(theme: &str) -> &'static str {
    match theme.trim() {
        DEFAULT_LIGHT_THEME | "light" => DEFAULT_LIGHT_THEME,
        _ => DEFAULT_DARK_THEME,
    }
}

/// One user-configured external editor entry for the "open with" menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorEntry {
    /// Display name shown in the menu (e.g. "VS Code").
    pub name: String,
    /// 启动命令模板：`{file}` 会替换为本地文件路径；缺省时把路径追加为最后一个参数。
    /// UI 解析器支持基础引号与反斜杠转义。
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppSettings {
    /// Display language for the desktop UI.
    #[serde(default)]
    pub language: AppLanguage,
    /// Monospace font family for the terminal.
    pub font_family: String,
    /// Font size in points.
    pub font_size: f32,
    /// Named color theme.
    pub theme: String,
    /// Scrollback buffer size in lines.
    pub scrollback_lines: usize,
    /// Cursor style.
    pub cursor_style: CursorStyle,
    /// Whether to read `~/.ssh/config` when connecting.
    pub use_ssh_config: bool,
    /// Case-insensitive row-level terminal highlight triggers.
    #[serde(default)]
    pub trigger_highlights: Vec<String>,
    /// Default external editor command for SFTP "open in editor". `{file}` is
    /// replaced with the local path. Empty/None = use the OS default handler.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_editor: Option<String>,
    /// Extra named editors offered in the SFTP file "open with" submenu.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub editors: Vec<EditorEntry>,
    /// Show a line-number gutter in the terminal.
    #[serde(default)]
    pub show_line_numbers: bool,
    /// Show a per-line timestamp gutter (`[HH:MM:SS]`) in the terminal.
    #[serde(default)]
    pub show_timestamps: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            language: AppLanguage::default(),
            font_family: default_mono_font().to_string(),
            font_size: 13.0,
            theme: DEFAULT_DARK_THEME.to_string(),
            scrollback_lines: 10_000,
            cursor_style: CursorStyle::Block,
            use_ssh_config: true,
            trigger_highlights: default_trigger_highlights(),
            default_editor: None,
            editors: Vec::new(),
            show_line_numbers: false,
            show_timestamps: false,
        }
    }
}

impl AppSettings {
    pub fn normalized_theme(&self) -> &'static str {
        normalize_theme_name(&self.theme)
    }

    pub fn is_light_theme(&self) -> bool {
        self.normalized_theme() == DEFAULT_LIGHT_THEME
    }
}

fn default_trigger_highlights() -> Vec<String> {
    ["error", "failed", "warning", "panic"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

#[cfg(target_os = "windows")]
fn system_locale_language() -> Option<AppLanguage> {
    let language_id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
    let primary_language_id = language_id & 0x03ff;
    Some(if primary_language_id == 0x04 {
        AppLanguage::Chinese
    } else {
        AppLanguage::English
    })
}

#[cfg(not(target_os = "windows"))]
fn system_locale_language() -> Option<AppLanguage> {
    None
}

fn env_locale_language() -> AppLanguage {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|value| !value.trim().is_empty())
        .map(|value| AppLanguage::from_locale_tag(&value))
        .unwrap_or(AppLanguage::English)
}

fn default_mono_font() -> &'static str {
    if cfg!(target_os = "macos") {
        "Menlo"
    } else if cfg!(target_os = "windows") {
        "Consolas"
    } else {
        "DejaVu Sans Mono"
    }
}

/// Top-level persisted config: settings + saved sessions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub settings: AppSettings,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    #[serde(default)]
    pub sessions: Vec<SessionProfile>,
}

/// Resolve cross-platform application directories.
///
/// * Linux:   `~/.config/KitonyTerms`, `~/.local/share/KitonyTerms`
/// * macOS:   `~/Library/Application Support/com.kitony.KitonyTerms`
/// * Windows: `%APPDATA%\KitonyTerms\config`
/// * Android: application-private `files/config`, `files/data`
pub struct Paths {
    config_dir: PathBuf,
    data_dir: PathBuf,
}

impl Paths {
    pub fn discover() -> Result<Self> {
        #[cfg(target_os = "android")]
        {
            return android_paths();
        }

        #[cfg(not(target_os = "android"))]
        {
            let dirs = ProjectDirs::from("com", "kitony", "KitonyTerms")
                .ok_or(ConfigError::NoConfigDir)?;
            Ok(Self::from_dirs(
                dirs.config_dir().to_path_buf(),
                dirs.data_dir().to_path_buf(),
            ))
        }
    }

    fn from_dirs(config_dir: PathBuf, data_dir: PathBuf) -> Self {
        Self {
            config_dir,
            data_dir,
        }
    }

    #[cfg(target_os = "android")]
    fn from_files_dir(files_dir: PathBuf) -> Self {
        Self::from_dirs(files_dir.join("config"), files_dir.join("data"))
    }

    /// `config.toml` location.
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    /// Encrypted secrets vault location.
    pub fn vault_file(&self) -> PathBuf {
        self.data_dir.join("secrets.vault")
    }

    /// 本机自动密码库的随机密钥文件位置。
    pub fn vault_key_file(&self) -> PathBuf {
        self.data_dir.join("secrets.vault.key")
    }

    /// known_hosts-style trust store location.
    pub fn known_hosts_file(&self) -> PathBuf {
        self.data_dir.join("known_hosts.toml")
    }

    /// GUI 单实例锁文件位置。
    pub fn instance_lock_file(&self) -> PathBuf {
        self.data_dir.join("kitonyterms.lock")
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

#[cfg(target_os = "android")]
fn android_paths() -> Result<Paths> {
    android_files_dir().map(Paths::from_files_dir)
}

#[cfg(target_os = "android")]
fn android_files_dir() -> Result<PathBuf> {
    use jni::objects::{JObject, JString};

    let context = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(context.vm().cast()) }
        .map_err(|error| ConfigError::Platform(format!("Android JavaVM 初始化失败: {error}")))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|error| ConfigError::Platform(format!("Android JNI 线程附加失败: {error}")))?;
    let context = unsafe { JObject::from_raw(context.context().cast()) };

    let files_dir = env
        .call_method(context, "getFilesDir", "()Ljava/io/File;", &[])
        .and_then(|value| value.l())
        .map_err(|error| ConfigError::Platform(format!("Android getFilesDir 调用失败: {error}")))?;
    let absolute_path = env
        .call_method(files_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .and_then(|value| value.l())
        .map_err(|error| {
            ConfigError::Platform(format!("Android getAbsolutePath 调用失败: {error}"))
        })?;
    let absolute_path = JString::from(absolute_path);
    let path: String = env
        .get_string(&absolute_path)
        .map_err(|error| ConfigError::Platform(format!("Android 私有目录读取失败: {error}")))?
        .into();

    if path.trim().is_empty() {
        return Err(ConfigError::NoConfigDir);
    }

    Ok(PathBuf::from(path))
}

impl Config {
    /// Load config from the given file. Returns [`Config::default`] if the file
    /// does not exist (first run).
    pub fn load_from(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        match std::fs::read_to_string(path) {
            Ok(s) => toml::from_str(&s).map_err(|e| ConfigError::Parse(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(ConfigError::Io(e)),
        }
    }

    /// Serialize and write the config to `path` atomically.
    pub fn save_to(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let toml =
            toml::to_string_pretty(self).map_err(|e| ConfigError::Serialize(e.to_string()))?;
        atomic_write(path, toml.as_bytes())
    }

    /// Find a saved session by name.
    pub fn session(&self, name: &str) -> Option<&SessionProfile> {
        self.sessions.iter().find(|s| s.name == name)
    }

    /// Insert or replace a session profile by name.
    pub fn upsert_session(&mut self, profile: SessionProfile) {
        if let Some(existing) = self.sessions.iter_mut().find(|s| s.name == profile.name) {
            *existing = profile;
        } else {
            self.sessions.push(profile);
        }
    }

    /// 返回显式分组与会话引用分组的并集，用于兼容旧配置。
    pub fn group_names(&self) -> Vec<String> {
        let mut groups = self
            .groups
            .iter()
            .filter_map(|name| normalize_group_name(name))
            .collect::<Vec<_>>();
        for session in &self.sessions {
            if let Some(group) = session.group.as_deref().and_then(normalize_group_name) {
                groups.push(group);
            }
        }
        groups.sort();
        groups.dedup();
        groups
    }

    /// 新增分组。空白名称会被忽略。
    pub fn add_group(&mut self, name: impl AsRef<str>) {
        let Some(name) = normalize_group_name(name.as_ref()) else {
            return;
        };
        if !self.groups.iter().any(|group| group == &name) {
            self.groups.push(name);
            self.groups.sort();
        }
    }

    /// 重命名分组，并同步所有引用该分组的会话。
    pub fn rename_group(&mut self, old_name: &str, new_name: impl AsRef<str>) {
        let Some(old_name) = normalize_group_name(old_name) else {
            return;
        };
        let Some(new_name) = normalize_group_name(new_name.as_ref()) else {
            return;
        };
        if old_name == new_name {
            self.add_group(new_name);
            return;
        }

        for group in &mut self.groups {
            if group == &old_name {
                *group = new_name.clone();
            }
        }
        for session in &mut self.sessions {
            if session.group.as_deref() == Some(old_name.as_str()) {
                session.group = Some(new_name.clone());
            }
        }
        self.groups.retain(|group| !group.trim().is_empty());
        if !self.groups.iter().any(|group| group == &new_name) {
            self.groups.push(new_name);
        }
        self.groups.sort();
        self.groups.dedup();
    }

    /// 删除分组；引用该分组的会话会移入默认分组。
    pub fn delete_group(&mut self, name: &str) {
        let Some(name) = normalize_group_name(name) else {
            return;
        };
        self.groups.retain(|group| group != &name);
        for session in &mut self.sessions {
            if session.group.as_deref() == Some(name.as_str()) {
                session.group = None;
            }
        }
    }
}

pub fn normalize_group_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_params_defaults_and_vault_id() {
        let p = ConnectParams::new("example.com", "alice");
        assert_eq!(p.port, 22);
        assert_eq!(p.effective_vault_id(), "alice@example.com:22");
        assert_eq!(p.auth, vec![AuthMethod::Password]);
    }

    #[test]
    fn config_toml_roundtrip() {
        let mut cfg = Config::default();
        cfg.settings.language = AppLanguage::Chinese;
        cfg.add_group("Web");
        cfg.upsert_session(SessionProfile {
            name: "prod-web".into(),
            group: Some("Web".into()),
            params: ConnectParams {
                host: "10.0.0.5".into(),
                port: 2222,
                user: "deploy".into(),
                auth: vec![AuthMethod::PublicKey {
                    key_path: PathBuf::from("/home/me/.ssh/id_ed25519"),
                }],
                vault_id: None,
                proxy_jump: None,
                proxy: ProxyConfig::Direct,
                forward_agent: false,
            },
        });
        let toml = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&toml).unwrap();
        assert_eq!(back.settings.language, AppLanguage::Chinese);
        assert_eq!(back.group_names(), vec!["Web"]);
        assert_eq!(back.sessions.len(), 1);
        assert_eq!(back.session("prod-web").unwrap().params.port, 2222);
    }

    #[test]
    fn paths_join_config_and_data_files() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("config");
        let data_dir = dir.path().join("data");
        let paths = Paths::from_dirs(config_dir.clone(), data_dir.clone());

        assert_eq!(paths.config_dir(), config_dir.as_path());
        assert_eq!(paths.data_dir(), data_dir.as_path());
        assert_eq!(paths.config_file(), config_dir.join("config.toml"));
        assert_eq!(paths.vault_file(), data_dir.join("secrets.vault"));
        assert_eq!(paths.vault_key_file(), data_dir.join("secrets.vault.key"));
        assert_eq!(paths.known_hosts_file(), data_dir.join("known_hosts.toml"));
        assert_eq!(
            paths.instance_lock_file(),
            data_dir.join("kitonyterms.lock")
        );
    }

    #[test]
    fn language_parses_chinese_locale_tags() {
        assert_eq!(AppLanguage::from_locale_tag("zh-CN"), AppLanguage::Chinese);
        assert_eq!(
            AppLanguage::from_locale_tag("zh_TW.UTF-8"),
            AppLanguage::Chinese
        );
        assert_eq!(AppLanguage::from_locale_tag("en-US"), AppLanguage::English);
    }

    #[test]
    fn app_settings_language_is_backward_compatible() {
        let toml = r#"
font_family = "Mono"
font_size = 13.0
theme = "default-dark"
scrollback_lines = 10000
cursor_style = "block"
use_ssh_config = true
"#;
        let settings: AppSettings = toml::from_str(toml).unwrap();
        assert!(matches!(
            settings.language,
            AppLanguage::Chinese | AppLanguage::English
        ));
    }

    #[test]
    fn proxy_config_defaults_to_direct_and_is_skipped_in_toml() {
        let params = ConnectParams::new("example.com", "alice");
        assert_eq!(params.proxy, ProxyConfig::Direct);
        assert!(params.proxy.is_direct());

        let toml = toml::to_string_pretty(&params).unwrap();
        assert!(!toml.contains("[proxy]"));
        let back: ConnectParams = toml::from_str(&toml).unwrap();
        assert_eq!(back.proxy, ProxyConfig::Direct);
    }

    #[test]
    fn proxy_config_socks5_roundtrips() {
        let mut params = ConnectParams::new("example.com", "alice");
        params.proxy = ProxyConfig::Socks5 {
            host: "127.0.0.1".into(),
            port: 1080,
            username: Some("me".into()),
        };
        let toml = toml::to_string_pretty(&params).unwrap();
        let back: ConnectParams = toml::from_str(&toml).unwrap();
        assert_eq!(back.proxy, params.proxy);
    }

    #[test]
    fn app_settings_editor_fields_are_backward_compatible() {
        let toml = r#"
font_family = "Mono"
font_size = 13.0
theme = "default-dark"
scrollback_lines = 10000
cursor_style = "block"
use_ssh_config = true
"#;
        let settings: AppSettings = toml::from_str(toml).unwrap();
        assert_eq!(settings.default_editor, None);
        assert!(settings.editors.is_empty());
        assert!(!settings.show_line_numbers);
        assert!(!settings.show_timestamps);
    }

    #[test]
    fn app_settings_theme_helpers_normalize_known_values() {
        let mut settings = AppSettings::default();
        assert_eq!(settings.normalized_theme(), DEFAULT_DARK_THEME);
        assert!(!settings.is_light_theme());

        settings.theme = DEFAULT_LIGHT_THEME.to_string();
        assert_eq!(settings.normalized_theme(), DEFAULT_LIGHT_THEME);
        assert!(settings.is_light_theme());

        settings.theme = "unknown-theme".to_string();
        assert_eq!(settings.normalized_theme(), DEFAULT_DARK_THEME);
    }

    #[test]
    fn upsert_replaces_by_name() {
        let mut cfg = Config::default();
        cfg.upsert_session(SessionProfile {
            name: "x".into(),
            group: None,
            params: ConnectParams::new("a", "u"),
        });
        cfg.upsert_session(SessionProfile {
            name: "x".into(),
            group: None,
            params: ConnectParams::new("b", "u"),
        });
        assert_eq!(cfg.sessions.len(), 1);
        assert_eq!(cfg.session("x").unwrap().params.host, "b");
    }

    #[test]
    fn group_names_include_legacy_session_groups() {
        let mut cfg = Config::default();
        cfg.add_group("Ops");
        cfg.upsert_session(SessionProfile {
            name: "x".into(),
            group: Some("Web".into()),
            params: ConnectParams::new("a", "u"),
        });

        assert_eq!(cfg.group_names(), vec!["Ops", "Web"]);
    }

    #[test]
    fn rename_and_delete_group_update_sessions() {
        let mut cfg = Config::default();
        cfg.add_group("Old");
        cfg.upsert_session(SessionProfile {
            name: "x".into(),
            group: Some("Old".into()),
            params: ConnectParams::new("a", "u"),
        });

        cfg.rename_group("Old", "New");
        assert_eq!(cfg.group_names(), vec!["New"]);
        assert_eq!(cfg.session("x").unwrap().group.as_deref(), Some("New"));

        cfg.delete_group("New");
        assert!(cfg.group_names().is_empty());
        assert_eq!(cfg.session("x").unwrap().group, None);
    }

    #[test]
    fn load_missing_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.toml");
        let cfg = Config::load_from(&path).unwrap();
        assert!(cfg.sessions.is_empty());
    }

    #[test]
    fn save_then_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut cfg = Config::default();
        cfg.settings.font_size = 16.0;
        cfg.save_to(&path).unwrap();
        let back = Config::load_from(&path).unwrap();
        assert_eq!(back.settings.font_size, 16.0);
    }

    #[test]
    fn failed_atomic_save_removes_unique_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::create_dir(&path).unwrap();

        assert!(Config::default().save_to(&path).is_err());
        let leftovers = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn merge_ssh_config_fills_gaps() {
        let mut p = ConnectParams {
            host: "myserver".into(), // alias
            port: 22,
            user: String::new(),
            auth: vec![AuthMethod::Password],
            vault_id: None,
            proxy_jump: None,
            proxy: ProxyConfig::Direct,
            forward_agent: false,
        };
        let cfg = SshConfigHost {
            alias: "myserver".into(),
            hostname: Some("192.168.1.10".into()),
            port: Some(2200),
            user: Some("root".into()),
            identity_files: vec![PathBuf::from("/home/me/.ssh/id_rsa")],
            proxy_jump: Some("bastion".into()),
        };
        p.merge_ssh_config(&cfg);
        assert_eq!(p.host, "192.168.1.10");
        assert_eq!(p.port, 2200);
        assert_eq!(p.user, "root");
        assert_eq!(p.proxy_jump.as_deref(), Some("bastion"));
        // key auth inserted ahead of password
        assert_eq!(
            p.auth[0],
            AuthMethod::PublicKey {
                key_path: PathBuf::from("/home/me/.ssh/id_rsa")
            }
        );
    }

    #[test]
    fn merge_ssh_config_ignores_proxy_jump_none() {
        let mut p = ConnectParams::new("myserver", "root");
        let cfg = SshConfigHost {
            alias: "myserver".into(),
            proxy_jump: Some("none".into()),
            ..SshConfigHost::default()
        };

        p.merge_ssh_config(&cfg);

        assert_eq!(p.proxy_jump, None);
    }

    #[test]
    fn known_hosts_trusts_first_seen_and_rejects_changed_key() {
        let mut known_hosts = KnownHosts::default();

        assert_eq!(
            known_hosts.check_or_trust("Example.COM", 22, "SHA256:first"),
            KnownHostCheck::NewlyTrusted
        );
        assert_eq!(
            known_hosts.check_or_trust("example.com", 22, "SHA256:first"),
            KnownHostCheck::Trusted
        );
        assert_eq!(
            known_hosts.check_or_trust("example.com", 22, "SHA256:second"),
            KnownHostCheck::Changed {
                expected: "SHA256:first".to_string(),
                actual: "SHA256:second".to_string()
            }
        );
    }

    #[test]
    fn known_hosts_check_requires_explicit_trust_for_unknown_host() {
        let mut known_hosts = KnownHosts::default();

        assert_eq!(
            known_hosts.check("example.com", 22, "SHA256:first"),
            KnownHostCheck::Unknown {
                fingerprint: "SHA256:first".to_string()
            }
        );
        assert!(known_hosts.hosts.is_empty());

        assert_eq!(
            known_hosts.trust("example.com", 22, "SHA256:first"),
            KnownHostCheck::NewlyTrusted
        );
        assert_eq!(
            known_hosts.check("example.com", 22, "SHA256:first"),
            KnownHostCheck::Trusted
        );
        assert_eq!(
            known_hosts.check("example.com", 22, "SHA256:second"),
            KnownHostCheck::Changed {
                expected: "SHA256:first".to_string(),
                actual: "SHA256:second".to_string()
            }
        );
    }

    #[test]
    fn known_hosts_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts.toml");
        let mut known_hosts = KnownHosts::default();
        known_hosts.check_or_trust("[host.local]", 2200, "SHA256:key");
        known_hosts.save_to(&path).unwrap();

        let loaded = KnownHosts::load_from(&path).unwrap();
        assert_eq!(loaded.hosts.len(), 1);
        assert_eq!(loaded.hosts[0].host, "host.local");
        assert_eq!(loaded.hosts[0].port, 2200);
        assert_eq!(loaded.hosts[0].fingerprint, "SHA256:key");
    }
}
