use embuer::ServiceError;
use rsa::{pkcs1::DecodeRsaPublicKey, pkcs1::EncodeRsaPublicKey, pkcs1::EncodeRsaPrivateKey, RsaPrivateKey, RsaPublicKey};
use rsa::traits::PublicKeyParts;
use sha2::{Digest, Sha512};
use std::fs;
use std::io::Read;
use tempfile::TempDir;

/// Helper function to verify a signature using the same algorithm as the service
fn verify_signature(
    pubkey: &RsaPublicKey,
    signature_bytes: &[u8],
    hash_hex: &str,
) -> Result<(), ServiceError> {
    // Decode the hex hash string to bytes
    let hash_bytes = hex::decode(hash_hex)
        .map_err(|e| ServiceError::IOError(std::io::Error::other(format!(
            "Failed to decode hash hex string: {e}"
        ))))?;

    // Get public key components
    let n = pubkey.n();
    let e = pubkey.e();
    let key_size = (n.bits() + 7) / 8;
    
    if signature_bytes.len() != key_size {
        return Err(ServiceError::IOError(std::io::Error::other(format!(
            "Signature length {} does not match key size {}",
            signature_bytes.len(),
            key_size
        ))));
    }

    // Convert signature to BigUint for RSA operation
    let signature_biguint = rsa::BigUint::from_bytes_be(signature_bytes);
    
    // RSA signature verification: signature^e mod n should give us the padded hash
    let padded = signature_biguint.modpow(e, n);
    let padded_bytes = padded.to_bytes_be();
    
    // Ensure we have enough bytes (key size)
    let mut full_padded = vec![0u8; key_size];
    let offset = key_size.saturating_sub(padded_bytes.len());
    full_padded[offset..].copy_from_slice(&padded_bytes);
    let padded_bytes = full_padded;
    
    // Verify PKCS#1 v1.5 padding structure
    if padded_bytes.len() < 19 + hash_bytes.len() {
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: decrypted value too short"
        )));
    }
    
    // Check for 00 01 prefix
    if padded_bytes[0] != 0x00 || padded_bytes[1] != 0x01 {
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: missing PKCS#1 v1.5 padding prefix"
        )));
    }
    
    // Find the 00 separator after FF padding
    let mut sep_idx = 2;
    while sep_idx < padded_bytes.len() && padded_bytes[sep_idx] == 0xFF {
        sep_idx += 1;
    }
    
    if sep_idx >= padded_bytes.len() || padded_bytes[sep_idx] != 0x00 {
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: missing separator after padding"
        )));
    }
    
    // SHA-512 DigestInfo
    let sha512_digest_info: &[u8] = &[
        0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03, 0x05, 0x00, 0x04, 0x40
    ];
    
    let digest_start = sep_idx + 1;
    if digest_start + sha512_digest_info.len() + hash_bytes.len() > padded_bytes.len() {
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: not enough data for DigestInfo and hash"
        )));
    }
    
    // Verify DigestInfo
    let found_digest_info = &padded_bytes[digest_start..digest_start + sha512_digest_info.len()];
    if found_digest_info != sha512_digest_info {
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: DigestInfo mismatch (not SHA-512)"
        )));
    }
    
    // Extract and compare hash
    let hash_start = digest_start + sha512_digest_info.len();
    if hash_start + hash_bytes.len() > padded_bytes.len() {
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: not enough data for hash"
        )));
    }
    
    let extracted_hash = &padded_bytes[hash_start..hash_start + hash_bytes.len()];
    
    if extracted_hash != hash_bytes.as_slice() {
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: hash mismatch"
        )));
    }
    
    Ok(())
}

/// Helper to compute SHA512 hash of file
fn compute_sha512(file_path: &std::path::Path) -> String {
    let mut file = fs::File::open(file_path).unwrap();
    let mut hasher = Sha512::new();
    let mut buffer = [0u8; 8192];
    
    loop {
        let n = file.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    
    hex::encode(hasher.finalize())
}

#[test]
fn test_signature_verification_valid() {
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("test.bin");
    let key_file = temp_dir.path().join("key.pem");
    let pub_file = temp_dir.path().join("pub.pem");
    let sig_file = temp_dir.path().join("test.sig");

    // Create a test file
    fs::write(&test_file, b"test content for signature verification").unwrap();

    // Generate RSA key pair
    let mut rng = rand::thread_rng();
    let priv_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
    let pub_key = priv_key.to_public_key();

    // Save keys
    let priv_pem = priv_key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap();
    let pub_pem = pub_key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap();
    fs::write(&key_file, priv_pem).unwrap();
    fs::write(&pub_file, pub_pem).unwrap();

    // Sign with OpenSSL
    std::process::Command::new("openssl")
        .args(&["dgst", "-sha512", "-sign", key_file.to_str().unwrap(), "-out", sig_file.to_str().unwrap(), test_file.to_str().unwrap()])
        .output()
        .expect("Failed to sign with OpenSSL");

    // Compute hash
    let hash = compute_sha512(&test_file);

    // Load public key
    let pub_key_loaded = RsaPublicKey::from_pkcs1_pem(&fs::read_to_string(&pub_file).unwrap()).unwrap();

    // Read signature
    let signature_bytes = fs::read(&sig_file).unwrap();

    // Verify signature
    let result = verify_signature(&pub_key_loaded, &signature_bytes, &hash);
    assert!(result.is_ok(), "Signature verification should succeed: {:?}", result);
}

#[test]
fn test_signature_verification_wrong_hash() {
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("test.bin");
    let key_file = temp_dir.path().join("key.pem");
    let pub_file = temp_dir.path().join("pub.pem");
    let sig_file = temp_dir.path().join("test.sig");

    // Create a test file
    fs::write(&test_file, b"test content").unwrap();

    // Generate RSA key pair
    let mut rng = rand::thread_rng();
    let priv_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
    let pub_key = priv_key.to_public_key();

    // Save keys
    let priv_pem = priv_key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap();
    let pub_pem = pub_key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap();
    fs::write(&key_file, priv_pem).unwrap();
    fs::write(&pub_file, pub_pem).unwrap();

    // Sign with OpenSSL
    std::process::Command::new("openssl")
        .args(&["dgst", "-sha512", "-sign", key_file.to_str().unwrap(), "-out", sig_file.to_str().unwrap(), test_file.to_str().unwrap()])
        .output()
        .expect("Failed to sign with OpenSSL");

    // Use wrong hash (hash of different content)
    let wrong_hash = "a" .repeat(128); // Wrong hash (should be 128 hex chars = 64 bytes)

    // Load public key
    let pub_key_loaded = RsaPublicKey::from_pkcs1_pem(&fs::read_to_string(&pub_file).unwrap()).unwrap();

    // Read signature
    let signature_bytes = fs::read(&sig_file).unwrap();

    // Verify signature - should fail
    let result = verify_signature(&pub_key_loaded, &signature_bytes, &wrong_hash);
    assert!(result.is_err(), "Signature verification should fail with wrong hash");
    assert!(result.unwrap_err().to_string().contains("hash mismatch"));
}

#[test]
fn test_signature_verification_wrong_key() {
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("test.bin");
    let key_file = temp_dir.path().join("key.pem");
    let wrong_pub_file = temp_dir.path().join("wrong_pub.pem");
    let sig_file = temp_dir.path().join("test.sig");

    // Create a test file
    fs::write(&test_file, b"test content").unwrap();

    // Generate RSA key pair
    let mut rng = rand::thread_rng();
    let priv_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();

    // Save private key
    let priv_pem = priv_key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap();
    fs::write(&key_file, priv_pem).unwrap();

    // Sign with OpenSSL
    std::process::Command::new("openssl")
        .args(&["dgst", "-sha512", "-sign", key_file.to_str().unwrap(), "-out", sig_file.to_str().unwrap(), test_file.to_str().unwrap()])
        .output()
        .expect("Failed to sign with OpenSSL");

    // Generate a different key pair for verification
    let wrong_priv_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
    let wrong_pub_key = wrong_priv_key.to_public_key();
    let wrong_pub_pem = wrong_pub_key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap();
    fs::write(&wrong_pub_file, wrong_pub_pem).unwrap();

    // Compute hash
    let hash = compute_sha512(&test_file);

    // Load wrong public key
    let wrong_pub_key_loaded = RsaPublicKey::from_pkcs1_pem(&fs::read_to_string(&wrong_pub_file).unwrap()).unwrap();

    // Read signature
    let signature_bytes = fs::read(&sig_file).unwrap();

    // Verify signature - should fail
    let result = verify_signature(&wrong_pub_key_loaded, &signature_bytes, &hash);
    assert!(result.is_err(), "Signature verification should fail with wrong key");
}

#[test]
fn test_signature_verification_empty_file() {
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("empty.bin");
    let key_file = temp_dir.path().join("key.pem");
    let pub_file = temp_dir.path().join("pub.pem");
    let sig_file = temp_dir.path().join("empty.sig");

    // Create empty file
    fs::write(&test_file, b"").unwrap();

    // Generate RSA key pair
    let mut rng = rand::thread_rng();
    let priv_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
    let pub_key = priv_key.to_public_key();

    // Save keys
    let priv_pem = priv_key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap();
    let pub_pem = pub_key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap();
    fs::write(&key_file, priv_pem).unwrap();
    fs::write(&pub_file, pub_pem).unwrap();

    // Sign with OpenSSL
    std::process::Command::new("openssl")
        .args(&["dgst", "-sha512", "-sign", key_file.to_str().unwrap(), "-out", sig_file.to_str().unwrap(), test_file.to_str().unwrap()])
        .output()
        .expect("Failed to sign with OpenSSL");

    // Compute hash
    let hash = compute_sha512(&test_file);

    // Load public key
    let pub_key_loaded = RsaPublicKey::from_pkcs1_pem(&fs::read_to_string(&pub_file).unwrap()).unwrap();

    // Read signature
    let signature_bytes = fs::read(&sig_file).unwrap();

    // Verify signature - should work even with empty file
    let result = verify_signature(&pub_key_loaded, &signature_bytes, &hash);
    assert!(result.is_ok(), "Signature verification should succeed even with empty file");
}

#[test]
fn test_signature_verification_large_file() {
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("large.bin");
    let key_file = temp_dir.path().join("key.pem");
    let pub_file = temp_dir.path().join("pub.pem");
    let sig_file = temp_dir.path().join("large.sig");

    // Create a large test file (100KB)
    let large_data = vec![0x42u8; 100 * 1024];
    fs::write(&test_file, &large_data).unwrap();

    // Generate RSA key pair
    let mut rng = rand::thread_rng();
    let priv_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
    let pub_key = priv_key.to_public_key();

    // Save keys
    let priv_pem = priv_key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap();
    let pub_pem = pub_key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap();
    fs::write(&key_file, priv_pem).unwrap();
    fs::write(&pub_file, pub_pem).unwrap();

    // Sign with OpenSSL
    std::process::Command::new("openssl")
        .args(&["dgst", "-sha512", "-sign", key_file.to_str().unwrap(), "-out", sig_file.to_str().unwrap(), test_file.to_str().unwrap()])
        .output()
        .expect("Failed to sign with OpenSSL");

    // Compute hash
    let hash = compute_sha512(&test_file);

    // Load public key
    let pub_key_loaded = RsaPublicKey::from_pkcs1_pem(&fs::read_to_string(&pub_file).unwrap()).unwrap();

    // Read signature
    let signature_bytes = fs::read(&sig_file).unwrap();

    // Verify signature
    let result = verify_signature(&pub_key_loaded, &signature_bytes, &hash);
    assert!(result.is_ok(), "Signature verification should succeed with large file");
}

#[test]
fn test_signature_verification_corrupted_signature() {
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("test.bin");
    let key_file = temp_dir.path().join("key.pem");
    let pub_file = temp_dir.path().join("pub.pem");

    // Create a test file
    fs::write(&test_file, b"test content").unwrap();

    // Generate RSA key pair
    let mut rng = rand::thread_rng();
    let priv_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
    let pub_key = priv_key.to_public_key();

    // Save keys
    let priv_pem = priv_key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap();
    let pub_pem = pub_key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap();
    fs::write(&key_file, priv_pem).unwrap();
    fs::write(&pub_file, pub_pem).unwrap();

    // Compute hash
    let hash = compute_sha512(&test_file);

    // Load public key
    let pub_key_loaded = RsaPublicKey::from_pkcs1_pem(&fs::read_to_string(&pub_file).unwrap()).unwrap();

    // Create corrupted signature (wrong size)
    let corrupted_sig = vec![0u8; 256]; // All zeros

    // Verify signature - should fail
    let result = verify_signature(&pub_key_loaded, &corrupted_sig, &hash);
    assert!(result.is_err(), "Signature verification should fail with corrupted signature");
}

#[test]
fn test_signature_verification_wrong_size() {
    let temp_dir = TempDir::new().unwrap();
    let pub_file = temp_dir.path().join("pub.pem");

    // Generate RSA key pair
    let mut rng = rand::thread_rng();
    let priv_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
    let pub_key = priv_key.to_public_key();

    // Save public key
    let pub_pem = pub_key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap();
    fs::write(&pub_file, pub_pem).unwrap();

    // Load public key
    let pub_key_loaded = RsaPublicKey::from_pkcs1_pem(&fs::read_to_string(&pub_file).unwrap()).unwrap();

    // Create signature with wrong size
    let wrong_size_sig = vec![0u8; 128]; // Too small for 2048-bit key (should be 256 bytes)

    // Verify signature - should fail
    let result = verify_signature(&pub_key_loaded, &wrong_size_sig, "a".repeat(128).as_str());
    assert!(result.is_err(), "Signature verification should fail with wrong signature size");
    assert!(result.unwrap_err().to_string().contains("does not match key size"));
}

#[test]
fn test_signature_verification_mismatched_digest() {
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("test.bin");
    let key_file = temp_dir.path().join("key.pem");
    let pub_file = temp_dir.path().join("pub.pem");
    let sig_file = temp_dir.path().join("test.sig");

    // Create a test file
    fs::write(&test_file, b"test content").unwrap();

    // Generate RSA key pair
    let mut rng = rand::thread_rng();
    let priv_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
    let pub_key = priv_key.to_public_key();

    // Save keys
    let priv_pem = priv_key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap();
    let pub_pem = pub_key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap();
    fs::write(&key_file, priv_pem).unwrap();
    fs::write(&pub_file, pub_pem).unwrap();

    // Sign with SHA-256 instead of SHA-512 (should fail verification)
    std::process::Command::new("openssl")
        .args(&["dgst", "-sha256", "-sign", key_file.to_str().unwrap(), "-out", sig_file.to_str().unwrap(), test_file.to_str().unwrap()])
        .output()
        .expect("Failed to sign with OpenSSL");

    // Compute SHA-512 hash
    let hash = compute_sha512(&test_file);

    // Load public key
    let pub_key_loaded = RsaPublicKey::from_pkcs1_pem(&fs::read_to_string(&pub_file).unwrap()).unwrap();

    // Read signature
    let signature_bytes = fs::read(&sig_file).unwrap();

    // Verify signature - should fail because signature uses SHA-256 but we expect SHA-512
    let result = verify_signature(&pub_key_loaded, &signature_bytes, &hash);
    assert!(result.is_err(), "Signature verification should fail with SHA-256 signature when expecting SHA-512");
}

