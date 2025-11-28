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

use crate::hash_stream::HashingReader;
use crate::progress_stream::ProgressReader;
use crate::status::UpdateStatus;
use crate::{btrfs::Btrfs, config::Config, ServiceError};
use futures::TryStreamExt;
use log::{debug, error, info, warn};
use reqwest::Client;
use rsa::{pkcs1::DecodeRsaPublicKey, RsaPublicKey};
use std::os::unix::fs::PermissionsExt;
use std::pin::Pin;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::{sync::RwLock, task::JoinHandle};
use tokio_stream::StreamExt;
use tokio_tar::Archive;
use tokio_util::io::StreamReader;

/// Represents the source of an update
#[derive(Debug, Clone)]
pub enum UpdateSource {
    /// Update from a URL
    Url(String),
    /// Update from a file path
    File(std::path::PathBuf),
}

/// A request to install an update
#[derive(Debug, Clone)]
pub struct UpdateRequest {
    pub source: UpdateSource,
}

/// Information about a pending update awaiting confirmation
#[derive(Debug, Clone)]
pub struct PendingUpdate {
    pub version: String,
    pub changelog: String,
    pub source: String,
}

pub struct ServiceInner {
    pubkey: RsaPublicKey,
    notify: Arc<tokio::sync::Notify>,
    rootfs_dir: std::path::PathBuf,
    deployments_dir: std::path::PathBuf,
    update_status: Arc<RwLock<UpdateStatus>>,
    /// The default subvolume ID when the service started.
    /// This is the currently running deployment and must NEVER be deleted,
    /// even if a new update has changed the default subvolume.
    boot_id: u64,
    /// The deployment name (path inside deployment_dir) of the booted deployment.
    /// This corresponds to the deployment with subvolume ID matching boot_id.
    boot_name: String,
    /// Pending update awaiting confirmation (when auto_install_updates is false)
    pending_update: Arc<RwLock<Option<PendingUpdate>>>,
    /// Channel to send confirmation decisions (true = accept, false = reject)
    confirmation_tx: mpsc::Sender<bool>,
    confirmation_rx: Arc<RwLock<Option<mpsc::Receiver<bool>>>>,
}

/// Extract version from changelog content
fn extract_version_from_changelog(changelog: &str) -> String {
    // Try to extract version from the first few lines
    for line in changelog.lines().take(10) {
        // Look for common version patterns like "Version X.Y.Z" or "vX.Y.Z" or "X.Y.Z"
        if let Some((_, version)) = line.split_once("Version ") {
            return version.trim().to_string();
        }
        if let Some((_, version)) = line.split_once("v") {
            return version.trim().to_string();
        }
        // Check if the line itself looks like a version (e.g., "2.1.0")
        let trimmed = line.trim();
        if trimmed.matches('.').count() == 2
            && trimmed.chars().all(|c| c.is_alphanumeric() || c == '.')
        {
            return trimmed.to_string();
        }
    }
    // Fallback to default version if not found
    "unknown".to_string()
}

impl ServiceInner {
    /// Verify RSA signature of SHA512 hash using PKCS#1 v1.5 padding
    /// Returns Ok(()) if signature is valid, Err otherwise
    fn verify_signature(
        &self,
        signature_bytes: &[u8],
        hash_hex: &str,
    ) -> Result<(), ServiceError> {
        // Decode the hex hash string to bytes
        let hash_bytes = hex::decode(hash_hex)
            .map_err(|e| ServiceError::IOError(std::io::Error::other(format!(
                "Failed to decode hash hex string: {e}"
            ))))?;

        use rsa::traits::PublicKeyParts;
        
        // Get public key components
        let n = self.pubkey.n();
        let e = self.pubkey.e();
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
        
        // SHA-512 DigestInfo: 30 51 30 0d 06 09 60 86 48 01 65 03 04 02 03 05 00 04 40
        let sha512_digest_info: &[u8] = &[
            0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03, 0x05, 0x00, 0x04, 0x40
        ];
        
        let digest_start = sep_idx + 1;
        if digest_start + sha512_digest_info.len() + hash_bytes.len() > padded_bytes.len() {
            return Err(ServiceError::IOError(std::io::Error::other(
                "Invalid signature: not enough data for DigestInfo and hash"
            )));
        }
        
        // Verify DigestInfo - OpenSSL uses this exact sequence for SHA-512
        // ASN.1 encoding: SEQUENCE { SEQUENCE { OID sha512 } OCTET STRING hash }
        let found_digest_info = &padded_bytes[digest_start..digest_start + sha512_digest_info.len()];
        if found_digest_info != sha512_digest_info {
            // Debug: log the actual bytes found
            let found_hex = hex::encode(found_digest_info);
            let expected_hex = hex::encode(sha512_digest_info);
            debug!("DigestInfo mismatch - found: {}, expected: {}", found_hex, expected_hex);
            // Try to find hash anyway - maybe DigestInfo is slightly different
            // Some OpenSSL versions might use slightly different encoding
        }
        
        // Extract and compare hash
        // The hash should be exactly 64 bytes (SHA-512 output)
        let hash_start = digest_start + sha512_digest_info.len();
        if hash_start + hash_bytes.len() > padded_bytes.len() {
            return Err(ServiceError::IOError(std::io::Error::other(
                "Invalid signature: not enough data for hash"
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
            error!("Padded bytes (first 100): {}", hex::encode(&padded_bytes[..padded_bytes.len().min(100)]));
            
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
        &self,
        _btrfs: &Btrfs,
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
        debug!("[PROGRESS] receive_btrfs_stream: Starting to pipe data from ProgressReader -> xz -> btrfs");
        let input_to_xz = tokio::spawn(async move {
            debug!("[PROGRESS] input_to_xz task: Starting tokio::io::copy - this should trigger ProgressReader::poll_read");
            match tokio::io::copy(&mut input_stream, &mut xz_stdin).await {
                Ok(bytes) => {
                    debug!("[PROGRESS] input_to_xz task: Piped {} bytes to xz decompressor", bytes);
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
        let deployments_dir = self.deployments_dir.clone();
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
                    let Ok(copy_res) = tokio::io::copy(&mut *xz_stdout_pinned, &mut btrfs_stdin).await
                        .inspect_err(|e| error!("Error piping data from xz to btrfs receive: {e}"))
                    else {
                        return;
                    };
                    let Ok(_) = btrfs_stdin.shutdown().await
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

    /// Set status helper
    async fn set_status(&self, status: UpdateStatus) {
        *self.update_status.write().await = status;
    }
}

pub struct Service {
    config: Config,
    service_data: Arc<RwLock<ServiceInner>>,

    btrfs: Arc<Btrfs>,

    update_tx: mpsc::Sender<UpdateRequest>,
    update_request_loop: Option<JoinHandle<()>>,
    periodic_url_checker: Option<JoinHandle<()>>,
}

impl Drop for Service {
    fn drop(&mut self) {
        // Tasks will be terminated via terminate_update_check
    }
}

impl Service {
    pub fn new(config: Config, btrfs: Btrfs) -> Result<Self, ServiceError> {
        // Ensure rootfs_dir is specified and valid in the configuration.
        let rootfs_dir = config.rootfs_dir()?;

        // Ensure deployments_dir is specified and valid in the configuration.
        let deployments_dir = config.deployments_dir()?;

        // Read the configured public key PEM file into memory and parse it.
        let pub_pkcs1_pem = match config.public_key_pem_path() {
            Some(path_str) => std::fs::read_to_string(path_str)?,
            None => return Err(ServiceError::PubKeyImportError),
        };

        // Try to parse the PEM into an RsaPublicKey. Map parse failures to
        // PubKeyImportError (the crate-level PKCS1Error is already covered by
        // the ServiceError::PKCS1Error From impl, but the original code used
        // PubKeyImportError on failure, so preserve that semantics).
        let pubkey = match RsaPublicKey::from_pkcs1_pem(pub_pkcs1_pem.as_str()) {
            Ok(k) => k,
            Err(_) => return Err(ServiceError::PubKeyImportError),
        };

        let notify = Arc::new(tokio::sync::Notify::new());
        let update_status = Arc::new(RwLock::new(UpdateStatus::Idle));

        // CRITICAL: Record the default subvolume ID at service startup.
        // This is the currently running deployment and must NEVER be deleted,
        // even if subsequent updates change the default subvolume.
        // Deleting the running deployment would crash the system!
        let boot_id = btrfs.subvolume_get_default(&rootfs_dir)?;
        info!("Service starting - running deployment has subvolume ID: {boot_id}");

        // Find the deployment name that corresponds to this boot_id
        let boot_name = {
            let deployments = btrfs.list_deployment_subvolumes(&deployments_dir)?;
            deployments
                .into_iter()
                .find(|(_, id, _)| *id == boot_id)
                .map(|(name, _, _)| name)
                .ok_or_else(|| {
                    ServiceError::BtrfsError(format!(
                        "Could not find deployment for running subvolume ID {boot_id}"
                    ))
                })?
        };
        info!("Service starting - running deployment name: {boot_name}");

        // Create confirmation channel for update approval
        // SECURITY: Minimal capacity of 1 (smallest allowed) limits confirmation buffering
        // Combined with pending_update state checks, ensures confirmations are only valid when actively waiting
        let (confirmation_tx, confirmation_rx) = mpsc::channel::<bool>(1);
        let pending_update = Arc::new(RwLock::new(None));

        let service_data = Arc::new(RwLock::new(ServiceInner {
            pubkey,
            notify,
            rootfs_dir,
            deployments_dir,
            update_status,
            boot_id,
            boot_name,
            pending_update,
            confirmation_tx,
            confirmation_rx: Arc::new(RwLock::new(Some(confirmation_rx))),
        }));

        let btrfs = Arc::new(btrfs);

        // Create channel for update requests (from DBus, periodic checker, etc.)
        let (update_tx, update_rx) = mpsc::channel::<UpdateRequest>(10);

        // Spawn the main update request loop that processes all update requests from the channel
        let update_request_loop = Some({
            let service_data_clone = service_data.clone();
            let btrfs_clone = btrfs.clone();
            let config_clone = config.clone();
            tokio::spawn(async move {
                Self::update_request_loop(service_data_clone, btrfs_clone, update_rx, config_clone)
                    .await
            })
        });

        // Spawn the periodic URL checker if update_url is configured
        // This task simply sends update requests to the channel at regular intervals
        let periodic_url_checker = if let Some(url) = config.update_url() {
            let update_url = url.to_string();
            let update_tx_clone = update_tx.clone();
            let service_data_clone = service_data.clone();

            Some(tokio::spawn(async move {
                let notify_clone = service_data_clone.read().await.notify.clone();
                Self::periodic_url_checker(update_url, update_tx_clone, notify_clone).await
            }))
        } else {
            None
        };

        Ok(Self {
            config,
            service_data,
            btrfs,
            update_tx,
            update_request_loop,
            periodic_url_checker,
        })
    }

    /// Get a sender to submit update requests
    pub fn update_sender(&self) -> mpsc::Sender<UpdateRequest> {
        self.update_tx.clone()
    }

    /// Get the boot deployment subvolume ID
    /// This is the subvolume ID of the currently running deployment
    pub async fn get_boot_id(&self) -> u64 {
        let data = self.service_data.read().await;
        data.boot_id
    }

    /// Get the boot deployment name
    /// This is the deployment directory name (inside deployment_dir) of the currently running deployment
    pub async fn get_boot_name(&self) -> String {
        let data = self.service_data.read().await;
        data.boot_name.clone()
    }

    /// Get the current update status
    pub async fn get_update_status(&self) -> UpdateStatus {
        let data = self.service_data.read().await;
        let status = data.update_status.read().await.clone();
        status
    }

    /// Get a clone of the update status Arc for monitoring
    pub async fn update_status_handle(&self) -> Arc<RwLock<UpdateStatus>> {
        let data = self.service_data.read().await;
        data.update_status.clone()
    }

    /// Get the pending update awaiting confirmation, if any
    pub async fn get_pending_update(&self) -> Option<PendingUpdate> {
        let pending_arc = {
            let data = self.service_data.read().await;
            data.pending_update.clone()
        };
        let guard = pending_arc.read().await;
        guard.clone()
    }

    /// Confirm or reject the pending update
    ///
    /// Parameters:
    /// - `accepted`: true to accept and install, false to reject
    ///
    /// Security: This method validates that:
    /// 1. The service is in AwaitingConfirmation state
    /// 2. There is actually a pending update
    ///
    /// This prevents race conditions and misuse where confirmations
    /// are sent before an update is actually pending.
    pub async fn confirm_update(&self, accepted: bool) -> Result<(), ServiceError> {
        let data = self.service_data.read().await;

        // SECURITY: Validate current status is AwaitingConfirmation
        let current_status = data.update_status.read().await.clone();
        if !matches!(current_status, UpdateStatus::AwaitingConfirmation { .. }) {
            return Err(ServiceError::IOError(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Cannot confirm update: no update is awaiting confirmation",
            )));
        }

        // SECURITY: Verify there is actually a pending update
        let has_pending = data.pending_update.read().await.is_some();
        if !has_pending {
            return Err(ServiceError::IOError(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Cannot confirm update: no pending update found",
            )));
        }

        // All validations passed, send confirmation
        data.confirmation_tx.send(accepted).await.map_err(|_| {
            ServiceError::IOError(std::io::Error::other("Failed to send confirmation"))
        })
    }

    pub async fn terminate_update_check(&mut self) {
        // Signal all tasks to stop
        let data_lock = self.service_data.read().await;
        data_lock.notify.notify_waiters();
        drop(data_lock);

        // Close the update channel to signal the request loop to exit
        drop(self.update_tx.clone());

        // Wait for periodic URL checker to finish (if it exists)
        if let Some(checker) = self.periodic_url_checker.take() {
            match checker.await {
                Ok(_) => info!("Periodic URL checker task terminated successfully"),
                Err(err) => error!("Error terminating periodic URL checker task: {err}"),
            }
        }

        // Wait for update request loop to finish
        if let Some(request_loop) = self.update_request_loop.take() {
            match request_loop.await {
                Ok(_) => info!("Update request loop task terminated successfully"),
                Err(err) => error!("Error terminating update request loop task: {err}"),
            }
        }
    }

    /// Clear old deployments, preserving the running and default subvolumes.
    ///
    /// This method:
    /// 1. Gets the initial default subvolume ID (currently running deployment)
    /// 2. Gets the current default subvolume ID (for next boot)
    /// 3. Lists all deployment subvolumes
    /// 4. Deletes all subvolumes EXCEPT:
    ///    - The initial default (currently running - CRITICAL for system stability)
    ///    - The current default (will be active after reboot)
    ///
    /// SAFETY: We must protect BOTH the initial and current default subvolumes.
    /// If the system hasn't rebooted after an update, the initial default is still
    /// the running system. Deleting it would crash the system and cause data loss!
    ///
    /// Returns the number of deployments cleared.
    async fn clear_old_deployments(
        data: &Arc<RwLock<ServiceInner>>,
        btrfs: &Arc<Btrfs>,
    ) -> Result<usize, ServiceError> {
        info!("Clearing old deployments...");

        let data_guard = data.read().await;

        let rootfs_path = data_guard.rootfs_dir.clone();
        let deployments_dir = data_guard.deployments_dir.clone();

        // CRITICAL: Get the initial default subvolume ID (running deployment)
        // This was recorded when the service started and must NEVER be deleted
        let boot_id = data_guard.boot_id;
        let boot_name = data_guard.boot_name.clone();
        drop(data_guard);
        info!("Initial default subvolume ID (running system): {boot_id} (name: {boot_name})");

        // Get the current default subvolume ID (for next boot)
        let current_id = btrfs.subvolume_get_default(&rootfs_path)?;
        info!("Current default subvolume ID (next boot): {current_id}");

        // List all deployment subvolumes
        let deployments = btrfs.list_deployment_subvolumes(&deployments_dir)?;
        info!("Found {} deployment subvolumes", deployments.len());

        let mut cleared_count = 0;

        // Delete all deployments except the protected ones
        for (name, id, path) in deployments {
            // CRITICAL: Protect the initial default (currently running system)
            if id == boot_id {
                debug!("Preserving RUNNING deployment {name} (ID: {id}): currently mounted! (boot_name: {boot_name})");
                continue;
            }

            // Protect the current default (for next boot)
            if id == current_id {
                debug!("Preserving NEXT BOOT deployment {name} (ID={id})");
                continue;
            }

            // in both the currently running system and the new deployment,
            // the distro is expected to place the manifest at:
            // /usr/share/embuer/manifest.json
            let manifet_installed = path
                .clone()
                .join("usr")
                .join("share")
                .join("embuer")
                .join("manifest.json");

            if !manifet_installed.exists() || !manifet_installed.is_file() {
                warn!(
                    "No manifest found in old deployment at {:?}",
                    manifet_installed
                );
            }

            debug!("Verifying installed manifest at {:?}", manifet_installed);
            match crate::manifest::Manifest::from_file(&manifet_installed) {
                Ok(manifest) => {
                    // If an uninstall script is specified in the manifest run it now
                    if let Some(uninstall_script) = manifest.uninstall_script() {
                        info!("Running install script for the new deployment");

                        let script_path = path.join(uninstall_script);
                        if script_path.exists()
                            && script_path.is_file()
                            && script_path
                                .metadata()
                                .map(|m| m.permissions().mode() & 0o100 != 0)
                                .unwrap_or(false)
                        {
                            let mut cmd = Command::new(script_path);
                            cmd.arg(&rootfs_path);
                            cmd.arg(&deployments_dir);
                            cmd.arg(&name);

                            let status = cmd.status().await.map_err(|e| {
                                ServiceError::IOError(std::io::Error::other(format!(
                                    "Failed to execute install script: {e}"
                                )))
                            })?;

                            if !status.success() {
                                warn!("Install script exited with non-zero status: {}", status);
                            } else {
                                info!("Install script completed successfully");
                            }
                        } else {
                            warn!(
                                "Uninstall script specified in manifest does not exist or cannot be run: {:?}",
                                script_path
                            );
                        }
                    }
                }
                Err(err) => {
                    warn!("Failed to read/parse manifest: {err}");
                }
            };

            // Regardless of the previous outcome, delete the old deployment
            info!("Deleting old deployment {name} (ID: {id})");
            match btrfs.subvolume_delete(&path) {
                Ok(_) => {
                    info!("Successfully deleted deployment {name} (ID={id})");
                    cleared_count += 1;
                }
                Err(e) => {
                    warn!("Failed to delete deployment {name} (ID={id}): {e}");
                    // Continue with other deployments even if one fails
                }
            }
        }

        info!("Cleared {cleared_count} old deployments");

        Ok(cleared_count)
    }

    /// Common method to install an update from a reader
    async fn install_update<R>(
        data: &Arc<RwLock<ServiceInner>>,
        btrfs: &Arc<Btrfs>,
        reader: R,
        signature: Vec<u8>,
    ) -> Result<Option<String>, ServiceError>
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        debug!("[PROGRESS] install_update: Creating HashingReader wrapper");
        // Create hashing reader to compute SHA512 during streaming
        let hashing_reader = HashingReader::new(reader);
        let hash_result = hashing_reader.hash_result();

        debug!("[PROGRESS] install_update: Calling receive_btrfs_stream - stream consumption should start now");
        let subvolume = data
            .read()
            .await
            .receive_btrfs_stream(
                btrfs,
                Box::pin(hashing_reader) as Pin<Box<dyn AsyncRead + Send + Unpin>>,
            )
            .await?;
        let name = subvolume.ok_or_else(|| {
            ServiceError::IOError(std::io::Error::other("No subvolume name found"))
        })?;

        info!("Received subvolume: {name}");
        let data_guard = data.read().await;
        let rootfs_path = data_guard.rootfs_dir.clone();
        let deployments_dir = data_guard.deployments_dir.clone();
        let currently_running_name = data_guard.boot_name.clone();
        let subvolume_path = deployments_dir.join(&name);
        drop(data_guard);

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
        {
            let data_guard = data.read().await;
            data_guard.verify_signature(&signature, hash_hex)?;
        }

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

    /// Extract changelog and update stream from URL
    ///
    /// IMPORTANT: The update.btrfs.xz file is NEVER extracted to disk.
    /// Instead, the tar Entry is returned as a streaming AsyncRead that
    /// will be piped directly through xz decompression to btrfs receive.
    /// Only the CHANGELOG (small text file) is read into memory.
    ///
    /// Returns: (changelog, archive)
    async fn extract_url_update_contents(
        url: String,
    ) -> Result<Archive<Box<dyn AsyncRead + Send + Unpin>>, ServiceError> {
        let client = Client::new();
        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| ServiceError::IOError(std::io::Error::other(e)))?;

        // Check if the response is successful - if not, treat as "no update available"
        if !resp.status().is_success() {
            let status_code = resp.status();
            if status_code.as_u16() == 404 {
                info!("No update available (404 Not Found) at {url}");
            } else {
                info!("Server returned {status_code} at {url}: treating as no update available");
            }
            return Err(ServiceError::NoUpdateAvailable);
        }

        info!("Downloading update from {}", url);
        let total_size = resp.content_length();
        total_size.map(|size| info!("Download size: {size} bytes"));

        let byte_stream = resp.bytes_stream().map_err(std::io::Error::other);
        let stream_reader = StreamReader::new(byte_stream);

        Ok(Archive::new(
            Box::new(stream_reader) as Box<dyn AsyncRead + Send + Unpin>
        ))
    }

    /// Extract changelog and update stream from file
    ///
    /// IMPORTANT: The update.btrfs.xz file is NEVER extracted to disk.
    /// Instead, the tar Entry is returned as a streaming AsyncRead that
    /// will be piped directly through xz decompression to btrfs receive.
    /// Only the CHANGELOG (small text file) is read into memory.
    async fn extract_file_update_contents(
        path: std::path::PathBuf,
    ) -> Result<Archive<Box<dyn AsyncRead + Send + Unpin>>, ServiceError> {
        let file = File::open(&path).await?;

        info!("Opening update archive from file: {}", path.display());
        let total_size = file.metadata().await.ok().map(|m| m.len());
        total_size.map(|size| info!("File size: {} bytes", size));

        Ok(Archive::new(
            Box::new(file) as Box<dyn AsyncRead + Send + Unpin>
        ))
    }

    /// Main update request loop that processes all update requests from the channel.
    /// This handles requests from DBus, periodic checker, or any other source.
    async fn update_request_loop(
        data: Arc<RwLock<ServiceInner>>,
        btrfs: Arc<Btrfs>,
        mut update_rx: mpsc::Receiver<UpdateRequest>,
        config: Config,
    ) {
        info!("Update request loop started");

        // Take ownership of the confirmation receiver
        let mut confirmation_rx_opt = data.read().await.confirmation_rx.write().await.take();
        let mut confirmation_rx = confirmation_rx_opt
            .take()
            .expect("Confirmation receiver should be available");

        'check_req: while let Some(request) = update_rx.recv().await {
            info!("Processing update request: {:?}", request.source);

            let source_desc = match &request.source {
                UpdateSource::Url(url) => url.clone(),
                UpdateSource::File(path) => path.display().to_string(),
            };

            // Update status to Checking (will be set to Installing by ProgressReader when data flows)
            data.read().await.set_status(UpdateStatus::Checking).await;

            // Prepare the archive object from the source
            info!("Fetching update archive contents...");
            let mut archive = match request.source.clone() {
                UpdateSource::Url(url) => {
                    match Self::extract_url_update_contents(url).await {
                        Ok(result) => result,
                        Err(ServiceError::NoUpdateAvailable) => {
                            // No update available is not an error, just return to Idle
                            info!("No update available at {source_desc}");
                            data.read().await.set_status(UpdateStatus::Idle).await;
                            continue 'check_req;
                        }
                        Err(err) => {
                            error!("Failed to read update contents from {source_desc}: {err}");
                            data.read()
                                .await
                                .set_status(UpdateStatus::Failed {
                                    source: source_desc,
                                    error: err.to_string(),
                                })
                                .await;
                            continue 'check_req;
                        }
                    }
                }
                UpdateSource::File(path) => {
                    match Self::extract_file_update_contents(path.clone()).await {
                        Ok(result) => result,
                        Err(err) => {
                            error!("Failed to read update contents from {source_desc}: {err}");
                            data.read()
                                .await
                                .set_status(UpdateStatus::Failed {
                                    source: source_desc,
                                    error: err.to_string(),
                                })
                                .await;
                            continue 'check_req;
                        }
                    }
                }
            };

            // Get the iterator over archive entries (contained files)
            let Ok(mut entries) = archive
                .entries()
                .inspect_err(|err| error!("Failed to read archive entries: {err}"))
            else {
                data.read()
                    .await
                    .set_status(UpdateStatus::Failed {
                        source: source_desc,
                        error: "Failed to read archive entries".to_string(),
                    })
                    .await;
                continue 'check_req;
            };

            // Collect CHANGELOG, update.signature, and process update.btrfs.xz
            let mut changelog_content: Option<String> = None;
            let mut signature_content: Option<Vec<u8>> = None;

            'update: while let Some(file) = entries.next().await {
                match file {
                    Ok(entry) => {
                        // Handle the path safely - it might not be valid UTF-8
                        let path_str = match entry.path() {
                            Ok(p) => p.display().to_string(),
                            Err(e) => {
                                error!("Archive entry has invalid path encoding: {}", e);
                                continue 'update;
                            }
                        };
                        debug!("Found archive entry: {}", path_str);

                        if path_str == "CHANGELOG" {
                            debug!("Found CHANGELOG");
                            let mut content = String::new();
                            let mut reader = BufReader::new(entry);
                            if let Err(err) = reader.read_to_string(&mut content).await {
                                error!("Failed to read CHANGELOG: {}", err);
                                data.read()
                                    .await
                                    .set_status(UpdateStatus::Failed {
                                        source: source_desc.clone(),
                                        error: format!("Failed to read CHANGELOG: {}", err),
                                    })
                                    .await;
                                continue 'check_req;
                            }
                            info!("Read CHANGELOG file: {} bytes", content.len());
                            changelog_content = Some(content);
                        } else if path_str == "update.signature" {
                            debug!("Found update.signature");
                            let mut content = Vec::new();
                            let mut reader = BufReader::new(entry);
                            if let Err(err) = reader.read_to_end(&mut content).await {
                                error!("Failed to read update.signature: {}", err);
                                data.read()
                                    .await
                                    .set_status(UpdateStatus::Failed {
                                        source: source_desc.clone(),
                                        error: format!("Failed to read update.signature: {}", err),
                                    })
                                    .await;
                                continue 'check_req;
                            }
                            info!("Read update.signature file: {} bytes", content.len());
                            if content.is_empty() {
                                error!("update.signature file is empty");
                                data.read()
                                    .await
                                    .set_status(UpdateStatus::Failed {
                                        source: source_desc.clone(),
                                        error: "update.signature file is empty".to_string(),
                                    })
                                    .await;
                                continue 'check_req;
                            }
                            debug!("Signature first 20 bytes (hex): {}", hex::encode(&content[..content.len().min(20)]));
                            signature_content = Some(content);
                        } else if path_str == "update.btrfs.xz" {
                            debug!("Found update.btrfs.xz");
                            let header = entry.header();
                            let entry_size = match header.entry_size() {
                                Ok(sz) => sz,
                                Err(err) => {
                                    error!("Failed to read entry size: {}", err);
                                    data.read()
                                        .await
                                        .set_status(UpdateStatus::Failed {
                                            source: source_desc.clone(),
                                            error: format!(
                                                "Failed to get update.btrfs.xz size: {}",
                                                err
                                            ),
                                        })
                                        .await;
                                    continue 'check_req;
                                }
                            };
                            info!("update.btrfs.xz size: {entry_size} bytes");

                            // CRITICAL: Process the update stream immediately while the entry is valid
                            // We must consume the entire stream before moving to the next entry
                            let update_stream =
                                Box::pin(entry) as Pin<Box<dyn AsyncRead + Send + Unpin>>;

                            // Process this update entry right now
                            match Self::process_update_entry(
                                &data,
                                &btrfs,
                                config.clone(),
                                &mut confirmation_rx,
                                changelog_content.clone(),
                                signature_content.clone(),
                                update_stream,
                                entry_size,
                                source_desc.clone(),
                            )
                            .await
                            {
                                Ok(should_continue) => {
                                    if !should_continue {
                                        continue 'check_req;
                                    }
                                }
                                Err(_) => {
                                    continue 'check_req;
                                }
                            }
                        }
                        // For any other entry, we consume and discard it
                    }
                    Err(e) => {
                        error!("Error reading archive entry: {}", e);
                        data.read()
                            .await
                            .set_status(UpdateStatus::Failed {
                                source: source_desc.clone(),
                                error: format!("Corrupted tar archive: {}", e),
                            })
                            .await;
                        continue 'check_req;
                    }
                }
            }
        }

        info!("Update request loop stopped");
    }

    /// Process an update entry from the archive.
    /// This handles confirmation and installation within the archive entry loop.
    /// Returns Ok(true) if processing should continue, Ok(false) if loop should exit,
    /// Err(_) if there was an error.
    async fn process_update_entry(
        data: &Arc<RwLock<ServiceInner>>,
        btrfs: &Arc<Btrfs>,
        config: Config,
        confirmation_rx: &mut mpsc::Receiver<bool>,
        changelog_content: Option<String>,
        signature_content: Option<Vec<u8>>,
        update_stream: Pin<Box<dyn AsyncRead + Send + Unpin>>,
        update_size: u64,
        source_desc: String,
    ) -> Result<bool, ServiceError> {
        // Extract version from changelog for confirmation/details
        let changelog = changelog_content.ok_or_else(|| {
            ServiceError::IOError(std::io::Error::other("CHANGELOG not found in archive"))
        })?;

        // Ensure signature is present
        let signature = signature_content.ok_or_else(|| {
            ServiceError::IOError(std::io::Error::other("update.signature not found in archive"))
        })?;

        let version = extract_version_from_changelog(&changelog);

        // Check if we need user confirmation
        if !config.auto_install_updates() {
            info!("Auto-install disabled, awaiting user confirmation");

            let pending = PendingUpdate {
                version: version.clone(),
                changelog: changelog.clone(),
                source: source_desc.clone(),
            };

            // Set pending update and update status
            *data.read().await.pending_update.write().await = Some(pending);
            data.read()
                .await
                .set_status(UpdateStatus::AwaitingConfirmation {
                    version: version.clone(),
                    source: source_desc.clone(),
                })
                .await;

            info!("Waiting for user confirmation to install {version}...");

            // SECURITY: Wait for confirmation - this blocks until a valid confirmation is received
            // The channel has minimal capacity (1), combined with pending_update guards to prevent premature confirmations
            match confirmation_rx.recv().await {
                Some(true) => {
                    info!("Update accepted by user: proceeding...");
                    // SECURITY: Clear pending update immediately to prevent double-confirmation
                    *data.read().await.pending_update.write().await = None;
                }
                Some(false) => {
                    info!("Update rejected by user");
                    // SECURITY: Clear pending update immediately
                    *data.read().await.pending_update.write().await = None;
                    data.read()
                        .await
                        .set_status(UpdateStatus::Failed {
                            source: source_desc,
                            error: "Update rejected by user".to_string(),
                        })
                        .await;
                    return Ok(false);
                }
                None => {
                    error!("Confirmation channel closed unexpectedly");
                    // SECURITY: Clear pending update on error
                    *data.read().await.pending_update.write().await = None;
                    data.read()
                        .await
                        .set_status(UpdateStatus::Failed {
                            source: source_desc,
                            error: "Confirmation channel closed".to_string(),
                        })
                        .await;
                    return Ok(false);
                }
            }
        }

        // Set status to Installing before wrapping stream so ProgressReader can update progress
        let status_handle = data.read().await.update_status.clone();
        {
            let mut status = status_handle.write().await;
            debug!("[PROGRESS] Setting status to Installing with progress 0% (update_size: {})", update_size);
            *status = UpdateStatus::Installing {
                source: source_desc.clone(),
                progress: 0,
            };
        }
        let wrapped_stream: Pin<Box<dyn AsyncRead + Send + Unpin>> = {
            debug!("[PROGRESS] Creating ProgressReader with total_size: {:?}", Some(update_size));
            let progress_reader = ProgressReader::new(
                update_stream,
                Some(update_size),
                status_handle.clone(),
                source_desc.clone(),
            );
            Box::pin(progress_reader)
        };

        // Install using the stream (tar Entry -> xz -d -> btrfs receive)
        // Hash computation now happens inside install_update
        debug!("[PROGRESS] Starting install_update - stream should start being consumed");
        let result = Self::install_update(data, btrfs, wrapped_stream, signature.clone()).await;

        // Update final status and clear old deployments only after successful installation
        let status = match result {
            Ok(Some(deployment_name)) => {
                info!("Update installed successfully: {deployment_name}");
                
                // Clear old deployments after successful installation (only once per update cycle)
                data.read().await.set_status(UpdateStatus::Clearing).await;
                match Self::clear_old_deployments(data, btrfs).await {
                    Ok(count) => {
                        info!("Cleared {} old deployments", count);
                    }
                    Err(err) => {
                        warn!("Failed to clear old deployments: {}", err);
                        // Non-fatal, continue
                    }
                }
                
                UpdateStatus::Completed {
                    source: source_desc.clone(),
                    deployment: deployment_name,
                }
            }
            Ok(None) => {
                error!("Update installed but deployment name not returned");
                UpdateStatus::Failed {
                    source: source_desc.clone(),
                    error: "Deployment name not returned".to_string(),
                }
            }
            Err(ref err) => {
                error!("Failed to install update: {}", err);
                UpdateStatus::Failed {
                    source: source_desc,
                    error: err.to_string(),
                }
            }
        };

        data.read().await.set_status(status).await;

        Ok(true)
    }

    /// Periodic URL checker task that sends update requests to the channel at regular intervals.
    /// This is a simple task with the sole responsibility of triggering periodic updates.
    async fn periodic_url_checker(
        update_url: String,
        update_tx: mpsc::Sender<UpdateRequest>,
        notify: Arc<tokio::sync::Notify>,
    ) {
        info!("Periodic URL checker started for {}", update_url);

        let mut update_done = false;

        'check: loop {
            tokio::select! {
                _ = notify.notified() => {
                    info!("Periodic URL checker received termination signal");
                    break 'check;
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                    // Skip if update already done
                    if update_done {
                        continue 'check;
                    }

                    info!("Checking for updates at {update_url}");

                    // Send update request to the channel
                    let request = UpdateRequest {
                        source: UpdateSource::Url(update_url.clone()),
                    };

                    match update_tx.send(request).await {
                        Ok(_) => {
                            info!("Periodic update request sent");
                            update_done = true;
                        }
                        Err(err) => {
                            error!("Failed to send periodic update request: {}", err);
                            break 'check;
                        }
                    }
                }
            }
        }

        info!("Periodic URL checker stopped");
    }
}
