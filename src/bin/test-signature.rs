use embuer::ServiceError;
use rsa::{pkcs1::DecodeRsaPublicKey, RsaPublicKey};
use rsa::traits::PublicKeyParts;
use std::fs;
use std::io::Read;
use sha2::{Digest, Sha512};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("Usage: {} <public_key.pem> <file_to_verify> <signature_file>", args[0]);
        eprintln!("Example: {} pub.pem update.btrfs.xz update.signature", args[0]);
        std::process::exit(1);
    }

    let pubkey_path = &args[1];
    let file_path = &args[2];
    let sig_path = &args[3];

    println!("=== Signature Verification Test ===\n");

    // Load public key
    println!("1. Loading public key from: {}", pubkey_path);
    let pub_pem = fs::read_to_string(pubkey_path)?;
    let pubkey: RsaPublicKey = RsaPublicKey::from_pkcs1_pem(&pub_pem)
        .map_err(|e| format!("Failed to parse public key: {}", e))?;
    println!("   ✓ Public key loaded (key size: {} bits)\n", pubkey.n().bits());

    // Read file and compute SHA512
    println!("2. Computing SHA512 hash of: {}", file_path);
    let mut file = fs::File::open(file_path)?;
    let mut hasher = Sha512::new();
    let mut buffer = [0u8; 8192];
    let mut total_bytes = 0;
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        total_bytes += n;
    }
    let hash = hasher.finalize();
    let hash_hex = hex::encode(hash);
    println!("   File size: {} bytes", total_bytes);
    println!("   SHA512 hash: {}\n", hash_hex);

    // Read signature
    println!("3. Reading signature from: {}", sig_path);
    let signature_bytes = fs::read(sig_path)?;
    println!("   Signature size: {} bytes", signature_bytes.len());
    println!("   Signature first 20 bytes (hex): {}\n", hex::encode(&signature_bytes[..signature_bytes.len().min(20)]));

    // Verify signature
    println!("4. Verifying signature...");
    match verify_signature(&pubkey, &signature_bytes, &hash_hex) {
        Ok(()) => {
            println!("   ✓ Signature verification SUCCESSFUL!");
            Ok(())
        }
        Err(e) => {
            eprintln!("   ✗ Signature verification FAILED: {}", e);
            Err(e.into())
        }
    }
}

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

    println!("   Decoded hash bytes: {} bytes", hash_bytes.len());
    
    // Get public key components
    let n = pubkey.n();
    let e = pubkey.e();
    let key_size = (n.bits() + 7) / 8;
    
    println!("   Key size: {} bytes", key_size);
    println!("   Signature length: {} bytes", signature_bytes.len());
    
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
    println!("   Computing signature^e mod n...");
    let padded = signature_biguint.modpow(e, n);
    let padded_bytes_raw = padded.to_bytes_be();
    
    // Ensure we have enough bytes (key size)
    let mut full_padded = vec![0u8; key_size];
    let offset = key_size.saturating_sub(padded_bytes_raw.len());
    full_padded[offset..].copy_from_slice(&padded_bytes_raw);
    let padded_bytes = full_padded;
    
    println!("   Decrypted padded length: {} bytes", padded_bytes.len());
    println!("   First 50 bytes of decrypted (hex): {}", hex::encode(&padded_bytes[..padded_bytes.len().min(50)]));
    
    // Verify PKCS#1 v1.5 padding structure
    // Format: 00 01 [at least 8 FF bytes] 00 [DER-encoded DigestInfo] [hash]
    if padded_bytes.len() < 19 + hash_bytes.len() {
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: decrypted value too short"
        )));
    }
    
    // Check for 00 01 prefix
    if padded_bytes[0] != 0x00 || padded_bytes[1] != 0x01 {
        println!("   ERROR: Missing PKCS#1 v1.5 padding prefix!");
        println!("   First 2 bytes: {:02x} {:02x}", padded_bytes[0], padded_bytes[1]);
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: missing PKCS#1 v1.5 padding prefix"
        )));
    }
    println!("   ✓ Found PKCS#1 v1.5 padding: 00 01");
    
    // Find the 00 separator after FF padding
    let mut sep_idx = 2;
    while sep_idx < padded_bytes.len() && padded_bytes[sep_idx] == 0xFF {
        sep_idx += 1;
    }
    
    println!("   Found {} padding bytes (0xFF)", sep_idx - 2);
    
    if sep_idx >= padded_bytes.len() || padded_bytes[sep_idx] != 0x00 {
        println!("   ERROR: Missing separator after padding!");
        println!("   Byte at index {}: {:02x}", sep_idx, if sep_idx < padded_bytes.len() { padded_bytes[sep_idx] } else { 0 });
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: missing separator after padding"
        )));
    }
    println!("   ✓ Found separator (00) at index {}", sep_idx);
    
    // SHA-512 DigestInfo: 30 51 30 0d 06 09 60 86 48 01 65 03 04 02 03 05 00 04 40
    let sha512_digest_info: &[u8] = &[
        0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03, 0x05, 0x00, 0x04, 0x40
    ];
    
    let digest_start = sep_idx + 1;
    println!("   DigestInfo starts at index: {}", digest_start);
    
    if digest_start + sha512_digest_info.len() + hash_bytes.len() > padded_bytes.len() {
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: not enough data for DigestInfo and hash"
        )));
    }
    
    // Verify DigestInfo
    let found_digest_info = &padded_bytes[digest_start..digest_start + sha512_digest_info.len()];
    println!("   Expected DigestInfo (hex): {}", hex::encode(sha512_digest_info));
    println!("   Found DigestInfo (hex):    {}", hex::encode(found_digest_info));
    
    if found_digest_info != sha512_digest_info {
        println!("   ERROR: DigestInfo mismatch!");
        // Continue anyway to see what hash we get
    } else {
        println!("   ✓ DigestInfo matches SHA-512");
    }
    
    // Extract and compare hash
    let hash_start = digest_start + sha512_digest_info.len();
    if hash_start + hash_bytes.len() > padded_bytes.len() {
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: not enough data for hash"
        )));
    }
    
    let extracted_hash = &padded_bytes[hash_start..hash_start + hash_bytes.len()];
    let extracted_hex = hex::encode(extracted_hash);
    
    println!("   Expected hash (hex): {}", hash_hex);
    println!("   Extracted hash (hex): {}", extracted_hex);
    
    if extracted_hash != hash_bytes.as_slice() {
        println!("   ERROR: Hash mismatch!");
        return Err(ServiceError::IOError(std::io::Error::other(format!(
            "Invalid signature: hash mismatch (computed: {}, extracted from signature: {})",
            hash_hex, extracted_hex
        ))));
    }
    
    println!("   ✓ Hash matches!");
    
    Ok(())
}

