use kt_config::{Config, ConnectParams, FileOpener, SessionProfile, SshProxy};

#[test]
fn groups_persist_and_keep_session_references() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let mut config = Config::default();
    config.add_group("Prod");
    config.upsert_session(SessionProfile {
        name: "gateway".to_string(),
        group: Some("Prod".to_string()),
        params: ConnectParams::new("10.0.0.2", "root"),
    });

    config.save_to(&path).unwrap();
    let loaded = Config::load_from(&path).unwrap();

    assert_eq!(loaded.group_names(), vec!["Prod"]);
    assert_eq!(
        loaded.session("gateway").unwrap().group.as_deref(),
        Some("Prod")
    );
}

#[test]
fn deleted_group_moves_sessions_back_to_default_bucket() {
    let mut config = Config::default();
    config.add_group("Ops");
    config.upsert_session(SessionProfile {
        name: "ops-1".to_string(),
        group: Some("Ops".to_string()),
        params: ConnectParams::new("10.0.0.3", "root"),
    });

    config.delete_group("Ops");

    assert!(config.group_names().is_empty());
    assert_eq!(config.session("ops-1").unwrap().group, None);
}

#[test]
fn new_settings_fields_have_backward_compatible_defaults() {
    let loaded: Config = toml::from_str(
        r#"
        [settings]
        font_family = "Menlo"
        font_size = 13.0
        theme = "default-dark"
        scrollback_lines = 10000
        cursor_style = "block"
        use_ssh_config = true
        "#,
    )
    .unwrap();

    assert_eq!(loaded.settings.default_text_editor, None);
    assert!(loaded.settings.file_openers.is_empty());
    assert_eq!(loaded.settings.default_ssh_proxy, SshProxy::None);
    assert!(!loaded.settings.terminal_show_timestamps);
    assert!(!loaded.settings.terminal_show_line_numbers);
}

#[test]
fn ssh_proxy_and_openers_roundtrip_through_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let mut config = Config::default();
    config.settings.default_text_editor = Some("code --wait".to_string());
    config.settings.file_openers = vec![FileOpener {
        extension: "log".to_string(),
        command: "less".to_string(),
    }];
    config.settings.default_ssh_proxy = SshProxy::Socks {
        host: "127.0.0.1".to_string(),
        port: 1080,
        version: 5,
    };
    config.settings.terminal_show_timestamps = true;
    config.settings.terminal_show_line_numbers = true;
    config.upsert_session(SessionProfile {
        name: "via-proxy".to_string(),
        group: None,
        params: ConnectParams {
            proxy: SshProxy::Http {
                host: "proxy.local".to_string(),
                port: 8080,
            },
            ..ConnectParams::new("10.0.0.8", "root")
        },
    });

    config.save_to(&path).unwrap();
    let loaded = Config::load_from(&path).unwrap();

    assert_eq!(
        loaded.settings.default_text_editor.as_deref(),
        Some("code --wait")
    );
    assert_eq!(loaded.settings.file_openers, config.settings.file_openers);
    assert_eq!(
        loaded.settings.default_ssh_proxy,
        config.settings.default_ssh_proxy
    );
    assert!(loaded.settings.terminal_show_timestamps);
    assert!(loaded.settings.terminal_show_line_numbers);
    assert_eq!(
        loaded.session("via-proxy").unwrap().params.proxy,
        SshProxy::Http {
            host: "proxy.local".to_string(),
            port: 8080,
        }
    );
}
