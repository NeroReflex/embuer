use embuer::{config::Config, service::Service};
use rsa::{pkcs1::{EncodeRsaPublicKey, LineEnding}, RsaPrivateKey};
use std::io::Write;

#[test]
fn pem_loading_and_parsing() {
    // Generate a small RSA keypair for the test and write the public key PEM to a temp file.
    let mut rng = rand::thread_rng();
    let privkey = RsaPrivateKey::new(&mut rng, 1024).expect("failed to generate key");
    let pubkey = privkey.to_public_key();
    let pub_pem = pubkey
        .to_pkcs1_pem(LineEnding::LF)
        .expect("failed to encode public key to PEM");

    let mut tmp = tempfile::NamedTempFile::new().expect("failed to create temp file");
    write!(tmp, "{}", pub_pem).expect("failed to write pem");
    let tmp_path = tmp.path().to_string_lossy().to_string();

    // Build config JSON string pointing to the PEM file path
    let json = format!(r#"{{
        "check_for_updates": false,
        "auto_install_updates": false,
        "public_key_pem": "{}"
    }}"#, tmp_path);

    let cfg = Config::new(&json).expect("parse config");
    let svc = Service::new(cfg).expect("service should initialize with valid PEM");
    drop(svc);
}

#[test]
fn missing_pem_path_fails() {
    let json = r#"{
        "check_for_updates": false,
        "auto_install_updates": false
    }"#;

    let cfg = Config::new(json).expect("parse config");
    assert!(Service::new(cfg).is_err(), "should fail when no public_key_pem");
}

#[test]
fn invalid_pem_fails() {
    // write invalid content to temp file
    let mut tmp = tempfile::NamedTempFile::new().expect("failed to create temp file");
    write!(tmp, "not a pem").expect("failed to write");
    let tmp_path = tmp.path().to_string_lossy().to_string();

    let json = format!(r#"{{
        "check_for_updates": false,
        "auto_install_updates": false,
        "public_key_pem": "{}"
    }}"#, tmp_path);

    let cfg = Config::new(&json).expect("parse config");
    assert!(Service::new(cfg).is_err(), "should fail on invalid PEM");
}
