use embuer::config::Config;

#[test]
fn parse_config_minimal() {
    let json = r#"{
        "update_url": "http://example.com/update.btrfs.xz",
        "auto_install_updates": false
    }"#;

    let cfg = Config::new(json).expect("should parse config");
    assert!(cfg.update_url().is_some());
    assert_eq!(cfg.update_url().unwrap(), "http://example.com/update.btrfs.xz");
    assert!(!cfg.auto_install_updates());
}

#[test]
fn parse_config_no_updates() {
    let json = r#"{
        "auto_install_updates": false
    }"#;

    let cfg = Config::new(json).expect("should parse config");
    assert!(cfg.update_url().is_none());
    assert!(!cfg.auto_install_updates());
}
