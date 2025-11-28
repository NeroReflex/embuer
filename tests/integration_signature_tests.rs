/*
    embuer: an embedded software updater DBUS daemon and CLI interface
    Copyright (C) 2025  Denis Benato
    
    This program is free software; you can redistribute it and/or modify
    it under the terms of the GNU General Public License as published by
    the Free Software Foundation; either version 2 of the License, or
    (at your option) any later version.
    
    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
    GNU General Public License for more details.
    
    You should have received a copy of the GNU General Public License along
    with this program; if not, write to the Free Software Foundation, Inc.,
    51 Franklin Street, Fifth Floor, Boston, MA 02110-1301 USA.
*/

use embuer::hash_stream::HashingReader;
use embuer::ServiceError;
use rsa::{pkcs1::DecodeRsaPublicKey, pkcs1::EncodeRsaPublicKey, pkcs1::EncodeRsaPrivateKey, RsaPrivateKey, RsaPublicKey};
use rsa::traits::PublicKeyParts;
use sha2::{Digest, Sha512};
use std::fs;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, BufReader};

/// Helper function to verify signature (same as in signature_tests.rs)
fn verify_signature(
    pubkey: &RsaPublicKey,
    signature_bytes: &[u8],
    hash_hex: &str,
) -> Result<(), ServiceError> {
    let hash_bytes = hex::decode(hash_hex)
        .map_err(|e| ServiceError::IOError(std::io::Error::other(format!(
            "Failed to decode hash hex string: {e}"
        ))))?;

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

    let signature_biguint = rsa::BigUint::from_bytes_be(signature_bytes);
    let padded = signature_biguint.modpow(e, n);
    let padded_bytes = padded.to_bytes_be();
    
    let mut full_padded = vec![0u8; key_size];
    let offset = key_size.saturating_sub(padded_bytes.len());
    full_padded[offset..].copy_from_slice(&padded_bytes);
    let padded_bytes = full_padded;
    
    if padded_bytes.len() < 19 + hash_bytes.len() {
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: decrypted value too short"
        )));
    }
    
    if padded_bytes[0] != 0x00 || padded_bytes[1] != 0x01 {
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: missing PKCS#1 v1.5 padding prefix"
        )));
    }
    
    let mut sep_idx = 2;
    while sep_idx < padded_bytes.len() && padded_bytes[sep_idx] == 0xFF {
        sep_idx += 1;
    }
    
    if sep_idx >= padded_bytes.len() || padded_bytes[sep_idx] != 0x00 {
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: missing separator after padding"
        )));
    }
    
    let sha512_digest_info: &[u8] = &[
        0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03, 0x05, 0x00, 0x04, 0x40
    ];
    
    let digest_start = sep_idx + 1;
    if digest_start + sha512_digest_info.len() + hash_bytes.len() > padded_bytes.len() {
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: not enough data for DigestInfo and hash"
        )));
    }
    
    let found_digest_info = &padded_bytes[digest_start..digest_start + sha512_digest_info.len()];
    if found_digest_info != sha512_digest_info {
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: DigestInfo mismatch (not SHA-512)"
        )));
    }
    
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

#[tokio::test]
async fn test_hashing_reader_with_openssl_signature() {
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("test.bin");
    let key_file = temp_dir.path().join("key.pem");
    let pub_file = temp_dir.path().join("pub.pem");
    let sig_file = temp_dir.path().join("test.sig");

    // Create test data
    let test_data = b"Integration test data for HashingReader with OpenSSL signature";
    fs::write(&test_file, test_data).unwrap();

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

    // Read file and compute hash using HashingReader (simulating streaming)
    let file = tokio::fs::File::open(&test_file).await.unwrap();
    let mut hashing_reader = HashingReader::new(file);
    let hash_result = hashing_reader.hash_result();

    // Read through HashingReader
    let mut buffer = Vec::new();
    let mut async_reader = BufReader::new(&mut hashing_reader);
    async_reader.read_to_end(&mut buffer).await.unwrap();

    // Wait for hash finalization
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Get computed hash
    let hash_guard = hash_result.read().await;
    let computed_hash = hash_guard.as_ref().unwrap().clone();
    drop(hash_guard);

    // Also compute hash directly for comparison
    let mut direct_hasher = Sha512::new();
    direct_hasher.update(test_data);
    let direct_hash = hex::encode(direct_hasher.finalize());

    // Hashes should match
    assert_eq!(computed_hash, direct_hash, "HashingReader should compute same hash as direct computation");

    // Load public key and signature
    let pub_key_loaded = RsaPublicKey::from_pkcs1_pem(&fs::read_to_string(&pub_file).unwrap()).unwrap();
    let signature_bytes = fs::read(&sig_file).unwrap();

    // Verify signature with hash from HashingReader
    let result = verify_signature(&pub_key_loaded, &signature_bytes, &computed_hash);
    assert!(result.is_ok(), "Signature should verify with hash from HashingReader: {:?}", result);
}

#[tokio::test]
async fn test_full_workflow_large_file() {
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("large.bin");
    let key_file = temp_dir.path().join("key.pem");
    let pub_file = temp_dir.path().join("pub.pem");
    let sig_file = temp_dir.path().join("large.sig");

    // Create large test file (500KB)
    let large_data: Vec<u8> = (0..500 * 1024).map(|i| (i % 256) as u8).collect();
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

    // Stream through HashingReader (simulating actual application usage)
    let file = tokio::fs::File::open(&test_file).await.unwrap();
    let mut hashing_reader = HashingReader::new(file);
    let hash_result = hashing_reader.hash_result();

    // Read in chunks (simulating streaming)
    let mut async_reader = BufReader::new(&mut hashing_reader);
    let mut total_read = 0;
    let mut read_buf = vec![0u8; 8192];
    
    loop {
        let n = async_reader.read(&mut read_buf).await.unwrap();
        if n == 0 {
            break;
        }
        total_read += n;
    }

    assert_eq!(total_read, large_data.len(), "Should read all data");

    // Wait for hash finalization
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Get computed hash
    let hash_guard = hash_result.read().await;
    let computed_hash = hash_guard.as_ref().unwrap().clone();
    drop(hash_guard);

    // Verify with OpenSSL command line (double check)
    let openssl_output = std::process::Command::new("openssl")
        .args(&["dgst", "-sha512", "-hex", test_file.to_str().unwrap()])
        .output()
        .expect("Failed to run openssl dgst");

    let openssl_hash = String::from_utf8_lossy(&openssl_output.stdout)
        .trim()
        .split(' ')
        .last()
        .unwrap()
        .to_lowercase();

    assert_eq!(computed_hash, openssl_hash, "Hash should match OpenSSL output");

    // Verify signature
    let pub_key_loaded = RsaPublicKey::from_pkcs1_pem(&fs::read_to_string(&pub_file).unwrap()).unwrap();
    let signature_bytes = fs::read(&sig_file).unwrap();

    let result = verify_signature(&pub_key_loaded, &signature_bytes, &computed_hash);
    assert!(result.is_ok(), "Signature should verify: {:?}", result);
}

