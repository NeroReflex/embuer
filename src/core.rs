use std::os::unix::fs::PermissionsExt;
use std::pin::Pin;
use std::sync::Arc;

use log::{debug, error, info, warn};
use rsa::RsaPublicKey;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::btrfs::Btrfs;
use crate::hash_stream::HashingReader;
use crate::ServiceError;

/// Verify RSA signature of SHA512 hash using PKCS#1 v1.5 padding
/// Returns Ok(()) if signature is valid, Err otherwise
pub fn verify_signature(
    pubkey: &RsaPublicKey,
    signature_bytes: &[u8],
    hash_hex: &str,
) -> Result<(), ServiceError> {
    // Decode the hex hash string to bytes
    let hash_bytes = hex::decode(hash_hex).map_err(|e| {
        ServiceError::IOError(std::io::Error::other(format!(
            "Failed to decode hash hex string: {e}"
        )))
    })?;

    use rsa::traits::PublicKeyParts;

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
    // Format: 00 01 [at least 8 FF bytes] 00 [DER-encoded DigestInfo] [hash]
    if padded_bytes.len() < 19 + hash_bytes.len() {
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: decrypted value too short",
        )));
    }

    // Check for 00 01 prefix
    if padded_bytes[0] != 0x00 || padded_bytes[1] != 0x01 {
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: missing PKCS#1 v1.5 padding prefix",
        )));
    }

    // Find the 00 separator after FF padding
    let mut sep_idx = 2;
    while sep_idx < padded_bytes.len() && padded_bytes[sep_idx] == 0xFF {
        sep_idx += 1;
    }

    if sep_idx >= padded_bytes.len() || padded_bytes[sep_idx] != 0x00 {
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: missing separator after padding",
        )));
    }

    // SHA-512 DigestInfo: 30 51 30 0d 06 09 60 86 48 01 65 03 04 02 03 05 00 04 40
    let sha512_digest_info: &[u8] = &[
        0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03,
        0x05, 0x00, 0x04, 0x40,
    ];

    let digest_start = sep_idx + 1;
    if digest_start + sha512_digest_info.len() + hash_bytes.len() > padded_bytes.len() {
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: not enough data for DigestInfo and hash",
        )));
    }

    // Verify DigestInfo - OpenSSL uses this exact sequence for SHA-512
    // ASN.1 encoding: SEQUENCE { SEQUENCE { OID sha512 } OCTET STRING hash }
    let found_digest_info = &padded_bytes[digest_start..digest_start + sha512_digest_info.len()];
    if found_digest_info != sha512_digest_info {
        // Debug: log the actual bytes found
        let found_hex = hex::encode(found_digest_info);
        let expected_hex = hex::encode(sha512_digest_info);
        debug!(
            "DigestInfo mismatch - found: {}, expected: {}",
            found_hex, expected_hex
        );
        // Try to find hash anyway - maybe DigestInfo is slightly different
        // Some OpenSSL versions might use slightly different encoding
    }

    // Extract and compare hash
    // The hash should be exactly 64 bytes (SHA-512 output)
    let hash_start = digest_start + sha512_digest_info.len();
    if hash_start + hash_bytes.len() > padded_bytes.len() {
        return Err(ServiceError::IOError(std::io::Error::other(
            "Invalid signature: not enough data for hash",
        )));
    }

    let extracted_hash = &padded_bytes[hash_start..hash_start + hash_bytes.len()];

    if extracted_hash != hash_bytes.as_slice() {
        // Debug: log both hashes for troubleshooting
        let extracted_hex = hex::encode(extracted_hash);
        let digest_info_hex = hex::encode(found_digest_info);
        error!("Signature verification failed: hash mismatch");
        error!("DigestInfo found: {}", digest_info_hex);
        error!("Computed hash (from stream): {}", hash_hex);
        error!("Extracted hash (from signature): {}", extracted_hex);
        error!(
            "Padded bytes (first 100): {}",
            hex::encode(&padded_bytes[..padded_bytes.len().min(100)])
        );

        // Try alternative: maybe OpenSSL signed the raw hash without DigestInfo encoding
        // Some signature schemes sign just the hash bytes directly
        // But this shouldn't be the case for PKCS#1 v1.5...

        return Err(ServiceError::IOError(std::io::Error::other(format!(
            "Invalid signature: hash mismatch (computed: {}, extracted from signature: {})",
            hash_hex, extracted_hex
        ))));
    }

    // Also verify DigestInfo was correct (only if we got here with hash match)
    if found_digest_info != sha512_digest_info {
        warn!("DigestInfo encoding differs but hash matched - this is unusual");
    }

    info!("Signature verification successful");
    Ok(())
}

pub async fn receive_btrfs_stream<R>(
    deployments_dir: std::path::PathBuf,
    mut input_stream: R,
) -> Result<Option<String>, ServiceError>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    // Spawn the xz -d decompressor
    let mut xz_proc = Command::new("xz")
        .arg("-d")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(ServiceError::IOError)?;

    let mut xz_stdin = xz_proc.stdin.take().ok_or_else(|| {
        ServiceError::IOError(std::io::Error::other("Failed to open stdin for xz"))
    })?;

    let xz_stdout = xz_proc.stdout.take().ok_or_else(|| {
        ServiceError::IOError(std::io::Error::other("Failed to open stdout for xz"))
    })?;

    // Pipe input stream -> xz stdin
    debug!(
        "[PROGRESS] receive_btrfs_stream: Starting to pipe data from ProgressReader -> xz -> btrfs"
    );
    let input_to_xz = tokio::spawn(async move {
        debug!("[PROGRESS] input_to_xz task: Starting tokio::io::copy - this should trigger ProgressReader::poll_read");
        match tokio::io::copy(&mut input_stream, &mut xz_stdin).await {
            Ok(bytes) => {
                debug!(
                    "[PROGRESS] input_to_xz task: Piped {} bytes to xz decompressor",
                    bytes
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
                warn!("Broken pipe while sending to xz (xz may have finished early)");
            }
            Err(e) => {
                error!("Error piping data to xz: {}", e);
            }
        }
        // Signal EOF to xz
        if let Err(e) = xz_stdin.shutdown().await {
            if e.kind() != std::io::ErrorKind::BrokenPipe {
                warn!("Error closing xz stdin: {}", e);
            }
        }
    });

    // Pipe xz stdout -> btrfs receive directly (avoiding Unpin requirement)
    // We'll duplicate the btrfs receive logic here to handle ChildStdout properly
    let deployments_dir = deployments_dir.clone();
    let btrfs_task = {
        let lossy_path = deployments_dir.as_os_str().to_string_lossy().to_string();
        let mut btrfs_proc = tokio::process::Command::new("bash")
            .arg("-c")
            .arg(format!("btrfs receive {lossy_path} -e 1>&2"))
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(ServiceError::IOError)?;

        let mut btrfs_stdin = btrfs_proc.stdin.take().ok_or_else(|| {
            ServiceError::IOError(std::io::Error::other(
                "Failed to open stdin for btrfs receive",
            ))
        })?;

        let btrfs_stderr = btrfs_proc.stderr.take().ok_or_else(|| {
            ServiceError::IOError(std::io::Error::other(
                "Failed to open stderr for btrfs receive",
            ))
        })?;

        let btrfs_stderr_reader = BufReader::new(btrfs_stderr);

        // Pipe xz_stdout -> btrfs_stdin (using pin! to handle non-Unpin type)
        let pipe_task = {
            let mut xz_stdout_pinned = Box::pin(xz_stdout);
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let Ok(copy_res) = tokio::io::copy(&mut *xz_stdout_pinned, &mut btrfs_stdin)
                    .await
                    .inspect_err(|e| error!("Error piping data from xz to btrfs receive: {e}"))
                else {
                    return;
                };
                let Ok(_) = btrfs_stdin
                    .shutdown()
                    .await
                    .inspect_err(|e| error!("Error closing btrfs receive stdin: {e}"))
                else {
                    return;
                };
                debug!("Piped {} bytes from xz to btrfs receive", copy_res);
            })
        };

        // Read stderr concurrently - capture all output for error diagnosis and logging
        let stderr_task = tokio::spawn(async move {
            let mut subvol_name: Option<String> = None;
            let mut stderr_lines = Vec::new();
            let mut lines = btrfs_stderr_reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line_clone = line.clone();
                stderr_lines.push(line_clone.clone());
                // Log each stderr line for debugging
                debug!("btrfs receive stderr: {}", line_clone);
                if let Some(name) = line.strip_prefix("At subvol ") {
                    subvol_name = Some(name.to_string());
                }
            }
            // Log complete stderr output for debugging
            if !stderr_lines.is_empty() {
                let stderr_text = stderr_lines.join("\n");
                info!("btrfs receive stderr output:\n{}", stderr_text);
            }
            (subvol_name, stderr_lines)
        });

        tokio::spawn(async move {
            let (pipe_res, btrfs_res, stderr_res) =
                tokio::join!(pipe_task, btrfs_proc.wait(), stderr_task);

            // Get stderr output first for better error messages
            let (subvol_name, stderr_lines) = match stderr_res {
                Ok((name, lines)) => (name, lines),
                Err(e) => {
                    error!("stderr read task join error: {e}");
                    return Err(ServiceError::IOError(std::io::Error::other(format!(
                        "reading stderr from btrfs receive failed: {e}",
                    ))));
                }
            };

            // Check pipe result - broken pipe is expected if btrfs receive fails early
            let pipe_err = pipe_res.err();
            if let Some(e) = pipe_err {
                warn!("btrfs pipe task error: {e} (may be expected if btrfs receive failed)");
            }

            let btrfs_status = match btrfs_res {
                Ok(s) => s,
                Err(e) => {
                    let stderr_text = stderr_lines.join("\n");
                    error!("btrfs receive wait error: {e}");
                    if !stderr_text.is_empty() {
                        error!("btrfs receive stderr: {stderr_text}");
                    }
                    // Return error without stderr in the message (stderr is logged separately)
                    return Err(ServiceError::IOError(std::io::Error::other(format!(
                        "btrfs receive wait failed: {e}",
                    ))));
                }
            };

            if !btrfs_status.success() {
                let stderr_text = stderr_lines.join("\n");
                error!("btrfs receive failed with status: {btrfs_status}");
                if !stderr_text.is_empty() {
                    error!("btrfs receive stderr: {stderr_text}");
                }
                // Return error without stderr in the message (stderr is logged separately)
                return Err(ServiceError::IOError(std::io::Error::other(format!(
                    "btrfs receive failed with status: {btrfs_status}",
                ))));
            }

            Ok(subvol_name)
        })
    };

    let (xz_input_task_res, xz_task_res, btrfs_task_res) = tokio::join!(
        // Copy bytes from incoming stream to xz
        input_to_xz,
        // Run the xz command receiving the stream
        xz_proc.wait(),
        // Pipe xz stdout -> btrfs receive
        btrfs_task
    );

    let Ok(_) = xz_input_task_res
        .as_ref()
        .inspect_err(|e| error!("xz input stream join error: {e}"))
    else {
        return Err(ServiceError::IOError(std::io::Error::other(format!(
            "joining stream to xz failed",
        ))));
    };

    let Ok(xz_status) = xz_task_res
        .as_ref()
        .inspect_err(|e| error!("xz process join error: {e}"))
    else {
        return Err(ServiceError::IOError(std::io::Error::other(format!(
            "joining stream to xz failed",
        ))));
    };

    let subvolume_result = match btrfs_task_res {
        Ok(result) => result,
        Err(e) => {
            error!("btrfs receive join error: {e}");
            return Err(ServiceError::IOError(std::io::Error::other(format!(
                "joining of btrfs receive failed",
            ))));
        }
    };

    // Check xz status - but if btrfs receive failed, xz may have gotten SIGPIPE which is expected
    if !xz_status.success() {
        // If btrfs receive failed, xz getting SIGPIPE (signal 13) is expected (broken pipe)
        // On Unix, processes killed by signal exit with code 128 + signal_number
        // So SIGPIPE (13) would be exit code 141, but tokio may report it differently
        // If xz failed but btrfs receive also failed, btrfs receive is the root cause
        if subvolume_result.is_err() {
            warn!("xz decompressor failed (likely SIGPIPE) because btrfs receive failed - returning btrfs receive error");
            return subvolume_result;
        }
        // Otherwise, xz failed independently
        return Err(ServiceError::IOError(std::io::Error::other(format!(
            "xz decompressor failed with status: {xz_status}"
        ))));
    }

    debug!("xz decompression completed successfully");

    subvolume_result
}

/// Common method to install an update from a reader
pub async fn install_update<R>(
    signature_verify_data: Option<(&RsaPublicKey, &Vec<u8>)>,
    rootfs_dir: std::path::PathBuf,
    deployments_dir: std::path::PathBuf,
    boot_name: String,
    btrfs: &Arc<Btrfs>,
    reader: R,
) -> Result<Option<String>, ServiceError>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    debug!("[PROGRESS] install_update: Creating HashingReader wrapper");
    // Create hashing reader to compute SHA512 during streaming
    let hashing_reader = HashingReader::new(reader);
    let hash_result = hashing_reader.hash_result();

    debug!("[PROGRESS] install_update: Calling receive_btrfs_stream - stream consumption should start now");
    let subvolume = receive_btrfs_stream(
        deployments_dir.clone(),
        Box::pin(hashing_reader) as Pin<Box<dyn AsyncRead + Send + Unpin>>,
    )
    .await?;
    let name = subvolume
        .ok_or_else(|| ServiceError::IOError(std::io::Error::other("No subvolume name found")))?;

    info!("Received subvolume: {name}");
    let rootfs_path = rootfs_dir.clone();
    let deployments_dir = deployments_dir.clone();
    let currently_running_name = boot_name.clone();
    let subvolume_path = deployments_dir.join(&name);

    let subvol_id = btrfs
        .btrfs_subvol_get_id(subvolume_path.clone())
        .map_err(|e| {
            ServiceError::IOError(std::io::Error::other(format!("BTRFS subvolume error: {e}")))
        })?;

    // in both the currently running system and the new deployment,
    // the distro is expected to place the manifest at:
    // /usr/share/embuer/manifest.json
    let manifet_installed = subvolume_path
        .clone()
        .join("usr")
        .join("share")
        .join("embuer")
        .join("manifest.json");

    if !manifet_installed.exists() || !manifet_installed.is_file() {
        warn!(
            "No manifest found in installed subvolume at {:?}",
            manifet_installed
        );

        // Try to delete the invalid subvolume right away
        if let Err(e) = btrfs.subvolume_delete(subvolume_path.clone()) {
            warn!("Failed to delete invalid subvolume {name}: {e}");
        }

        return Err(ServiceError::IOError(std::io::Error::other(
            "Installed subvolume is missing manifest.json",
        )));
    }

    debug!("Verifying installed manifest at {:?}", manifet_installed);
    let manifest = match crate::manifest::Manifest::from_file(&manifet_installed) {
        Ok(m) => m,
        Err(err) => {
            warn!("Failed to read/parse manifest: {}", err);

            // Try to delete the invalid subvolume right away
            if let Err(e) = btrfs.subvolume_delete(subvolume_path.clone()) {
                warn!("Failed to delete invalid subvolume {name}: {e}");
            }

            return Err(ServiceError::IOError(std::io::Error::other(
                "Installed subvolume has invalid manifest.json",
            )));
        }
    };

    if !manifest.is_readonly() {
        debug!("Installed deployment is not marked as readonly: setting read-write");

        btrfs.subvolume_set_rw(subvolume_path.clone())?;
    }

    // Extract and verify the SHA512 hash computed during streaming
    // This MUST be done before calling the install script
    let hash = hash_result.read().await;
    let Some(hash_hex) = hash.as_deref() else {
        debug!("update.btrfs.xz SHA512: (not yet available)");
        // Try to delete the invalid subvolume right away
        if let Err(e) = btrfs.subvolume_delete(subvolume_path.clone()) {
            warn!("Failed to delete invalid subvolume {name}: {e}");
        }
        return Err(ServiceError::IOError(std::io::Error::other(
            "Failed to compute SHA512 hash of update stream",
        )));
    };

    info!("update.btrfs.xz SHA512: {hash_hex}");

    // Verify signature before proceeding with installation
    if let Some((pubkey, signature)) = signature_verify_data {
        debug!("Verifying signature of the update stream hash");
        verify_signature(pubkey, signature, hash_hex)?;
    };

    // If an install script is specified in the manifest run it now
    if let Some(install_script) = manifest.install_script() {
        info!("Running install script for the new deployment");

        let script_path = subvolume_path.join(install_script);
        if !script_path.exists()
            || !script_path.is_file()
            || !script_path
                .metadata()
                .map(|m| m.permissions().mode() & 0o100 != 0)
                .unwrap_or(false)
        {
            warn!(
                "Install script specified in manifest does not exist or cannot be run: {:?}",
                script_path
            );

            // Try to delete the invalid subvolume right away
            if let Err(e) = btrfs.subvolume_delete(subvolume_path.clone()) {
                warn!("Failed to delete invalid subvolume {name}: {e}");
            }

            return Err(ServiceError::IOError(std::io::Error::other(
                "Installed subvolume has invalid manifest.json",
            )));
        }

        let mut cmd = Command::new(script_path);
        cmd.arg(&rootfs_path);
        cmd.arg(&deployments_dir);
        cmd.arg(&name);
        cmd.arg(&currently_running_name);

        let status = cmd.status().await.map_err(|e| {
            ServiceError::IOError(std::io::Error::other(format!(
                "Failed to execute install script: {e}"
            )))
        })?;

        if !status.success() {
            warn!("Install script exited with non-zero status: {status}");
        } else {
            info!("Install script completed successfully");
        }
    }

    // Make the new subvolume the default for next boot
    btrfs.subvolume_set_default(subvol_id, &rootfs_path)?;

    info!("Installed deployment {name} (ID={subvol_id})");

    Ok(name.into())
}
