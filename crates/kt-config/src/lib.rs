//! KitonyTerms configuration: connection parameters, session profiles, app
//! settings, and `~/.ssh/config` integration.
//!
//! Everything here is UI-agnostic and serializable so it can be shared between
//! the headless example, the core engine, and the GUI.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

mod ssh_config;
pub use ssh_config::{lookup_ssh_config, SshConfigHost};

/// Errors for config loading/saving.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not determine a config directory for this platform")]
    NoConfigDir,

    #[error("failed to parse config: {0}")]
    Parse(String),

    #[error("failed to serialize config: {0}")]
    Serialize(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

type Result<T> = std::result::Result<T, ConfigError>;

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
    /// Use the system ssh-agent (later phase; declared now for forward-compat).
    Agent,
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
    }
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

/// Visual / behavioral app settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppSettings {
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
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            font_family: default_mono_font().to_string(),
            font_size: 13.0,
            theme: "default-dark".to_string(),
            scrollback_lines: 10_000,
            cursor_style: CursorStyle::Block,
            use_ssh_config: true,
        }
    }
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
    #[serde(default)]
    pub sessions: Vec<SessionProfile>,
}

/// Resolve cross-platform application directories.
///
/// * Linux:   `~/.config/KitonyTerms`, `~/.local/share/KitonyTerms`
/// * macOS:   `~/Library/Application Support/com.kitony.KitonyTerms`
/// * Windows: `%APPDATA%\KitonyTerms\config`
pub struct Paths {
    dirs: ProjectDirs,
}

impl Paths {
    pub fn discover() -> Result<Self> {
        let dirs =
            ProjectDirs::from("com", "kitony", "KitonyTerms").ok_or(ConfigError::NoConfigDir)?;
        Ok(Self { dirs })
    }

    /// `config.toml` location.
    pub fn config_file(&self) -> PathBuf {
        self.dirs.config_dir().join("config.toml")
    }

    /// Encrypted secrets vault location.
    pub fn vault_file(&self) -> PathBuf {
        self.dirs.data_dir().join("secrets.vault")
    }

    /// known_hosts-style trust store location.
    pub fn known_hosts_file(&self) -> PathBuf {
        self.dirs.data_dir().join("known_hosts.toml")
    }

    pub fn config_dir(&self) -> &Path {
        self.dirs.config_dir()
    }

    pub fn data_dir(&self) -> &Path {
        self.dirs.data_dir()
    }
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
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, toml.as_bytes())?;
        std::fs::rename(&tmp, path)?;
        Ok(())
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
        cfg.upsert_session(SessionProfile {
            name: "prod-web".into(),
            group: None,
            params: ConnectParams {
                host: "10.0.0.5".into(),
                port: 2222,
                user: "deploy".into(),
                auth: vec![AuthMethod::PublicKey {
                    key_path: PathBuf::from("/home/me/.ssh/id_ed25519"),
                }],
                vault_id: None,
            },
        });
        let toml = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&toml).unwrap();
        assert_eq!(back.sessions.len(), 1);
        assert_eq!(back.session("prod-web").unwrap().params.port, 2222);
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
    fn merge_ssh_config_fills_gaps() {
        let mut p = ConnectParams {
            host: "myserver".into(), // alias
            port: 22,
            user: String::new(),
            auth: vec![AuthMethod::Password],
            vault_id: None,
        };
        let cfg = SshConfigHost {
            alias: "myserver".into(),
            hostname: Some("192.168.1.10".into()),
            port: Some(2200),
            user: Some("root".into()),
            identity_files: vec![PathBuf::from("/home/me/.ssh/id_rsa")],
            proxy_jump: None,
        };
        p.merge_ssh_config(&cfg);
        assert_eq!(p.host, "192.168.1.10");
        assert_eq!(p.port, 2200);
        assert_eq!(p.user, "root");
        // key auth inserted ahead of password
        assert_eq!(
            p.auth[0],
            AuthMethod::PublicKey {
                key_path: PathBuf::from("/home/me/.ssh/id_rsa")
            }
        );
    }
}
