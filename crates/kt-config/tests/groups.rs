use kt_config::{Config, ConnectParams, SessionProfile};

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
