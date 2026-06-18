//! Thin wrapper over `russh-config` for reading `~/.ssh/config`.
//!
//! We expose our own [`SshConfigHost`] type rather than leaking `russh_config`
//! types into the rest of the app, so the dependency stays swappable.

use std::path::{Path, PathBuf};

/// The subset of `~/.ssh/config` host options KitonyTerms understands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SshConfigHost {
    /// The alias that was queried (the `Host` pattern the user typed).
    pub alias: String,
    /// Resolved `HostName`.
    pub hostname: Option<String>,
    /// `Port`.
    pub port: Option<u16>,
    /// `User`.
    pub user: Option<String>,
    /// `IdentityFile` entries (may be several).
    pub identity_files: Vec<PathBuf>,
    /// `ProxyJump` target (used in a later phase).
    pub proxy_jump: Option<String>,
}

/// Look up `host` in the given ssh config file. Returns `Ok(None)` if the file
/// does not exist; otherwise returns the merged host config (ssh's own
/// first-match semantics are handled by `russh-config`).
pub fn lookup_ssh_config(
    config_path: impl AsRef<Path>,
    host: &str,
) -> std::io::Result<Option<SshConfigHost>> {
    let config_path = config_path.as_ref();
    if !config_path.exists() {
        return Ok(None);
    }
    let parsed = match russh_config::parse_path(config_path, host) {
        Ok(c) => c,
        // A malformed config shouldn't be fatal — treat as "no config".
        Err(_) => return Ok(None),
    };

    let hc = &parsed.host_config;
    Ok(Some(SshConfigHost {
        alias: host.to_string(),
        hostname: Some(parsed.host().to_string()).filter(|h| h != host),
        port: hc.port,
        user: hc.user.clone(),
        identity_files: hc.identity_file.clone().unwrap_or_default(),
        proxy_jump: hc.proxy_jump.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_config(contents: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        (dir, path)
    }

    #[test]
    fn missing_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope");
        assert_eq!(lookup_ssh_config(&path, "anything").unwrap(), None);
    }

    #[test]
    fn parses_basic_host_block() {
        let (_dir, path) = write_config(
            "\
Host myserver
    HostName 192.168.50.10
    User deploy
    Port 2222
    IdentityFile /home/me/.ssh/id_ed25519
",
        );
        let got = lookup_ssh_config(&path, "myserver").unwrap().unwrap();
        assert_eq!(got.hostname.as_deref(), Some("192.168.50.10"));
        assert_eq!(got.user.as_deref(), Some("deploy"));
        assert_eq!(got.port, Some(2222));
        assert_eq!(
            got.identity_files,
            vec![PathBuf::from("/home/me/.ssh/id_ed25519")]
        );
    }

    #[test]
    fn unknown_host_yields_empty_fields() {
        let (_dir, path) = write_config(
            "\
Host known
    HostName 10.0.0.1
",
        );
        // russh-config returns a config with defaults for unknown hosts.
        let got = lookup_ssh_config(&path, "totally-unknown").unwrap().unwrap();
        assert_eq!(got.hostname, None);
        assert_eq!(got.user, None);
        assert_eq!(got.port, None);
        assert!(got.identity_files.is_empty());
    }
}
