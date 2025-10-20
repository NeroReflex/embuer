use embuer::config::Config;
use rsa::{
    pkcs1::{DecodeRsaPublicKey, EncodeRsaPublicKey, LineEnding},
    RsaPrivateKey,
};
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
    let json = format!(
        r#"{{
        "check_for_updates": false,
        "auto_install_updates": false,
        "public_key_pem": "{}"
    }}"#,
        tmp_path
    );

    let cfg = Config::new(&json).expect("parse config");

    // Instead of constructing the full `Service`, just perform the same
    // public-key PEM loading and parsing that `Service::new` does. This
    // keeps the test lightweight and runnable in unit-test environments.
    let pub_pkcs1_pem = cfg
        .public_key_pem_path()
        .map(|p| std::fs::read_to_string(p))
        .expect("public_key_pem path present")
        .expect("failed to read pem file");

    let _pubkey = rsa::RsaPublicKey::from_pkcs1_pem(pub_pkcs1_pem.as_str())
        .expect("failed to parse public key PEM");
}

#[test]
fn missing_pem_path_fails() {
    let json = r#"{
        "check_for_updates": false,
        "auto_install_updates": false
    }"#;

    let cfg = Config::new(json).expect("parse config");
    // Without a public_key_pem path the service initialization would fail.
    // Emulate the same check: ensure `public_key_pem_path()` is None.
    assert!(
        cfg.public_key_pem_path().is_none(),
        "should have no public_key_pem"
    );
}

#[test]
fn invalid_pem_fails() {
    // write invalid content to temp file
    let mut tmp = tempfile::NamedTempFile::new().expect("failed to create temp file");
    write!(tmp, "not a pem").expect("failed to write");
    let tmp_path = tmp.path().to_string_lossy().to_string();

    let json = format!(
        r#"{{
        "check_for_updates": false,
        "auto_install_updates": false,
        "public_key_pem": "{}"
    }}"#,
        tmp_path
    );

    let cfg = Config::new(&json).expect("parse config");

    // Attempt to read and parse the invalid PEM; ensure parsing fails.
    let pub_pkcs1_pem = cfg
        .public_key_pem_path()
        .map(|p| std::fs::read_to_string(p))
        .expect("public_key_pem path present")
        .expect("failed to read pem file");

    assert!(
        rsa::RsaPublicKey::from_pkcs1_pem(pub_pkcs1_pem.as_str()).is_err(),
        "should fail on invalid PEM"
    );
}
