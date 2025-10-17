use embuer::config::Config;

#[test]
fn parse_config_minimal() {
    let json = r#"{
        "check_for_updates": true,
        "auto_install_updates": false
    }"#;

    let cfg = Config::new(json).expect("should parse config");
    assert!(cfg.check_for_updates());
    assert!(!cfg.auto_install_updates());
}
