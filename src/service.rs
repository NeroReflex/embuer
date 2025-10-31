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
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
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
    pub async fn receive_btrfs_stream<R>(
        &self,
        btrfs: &Btrfs,
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

        // Wrap xz_stdout in BufReader, then box it to satisfy trait bounds
        // BufReader makes the stream Unpin, and boxing as a trait object satisfies Send + 'static
        let xz_stdout_reader: Pin<Box<dyn AsyncRead + Send + Unpin>> =
            Box::pin(BufReader::new(xz_stdout)) as Pin<Box<dyn AsyncRead + Send + Unpin>>;

        // Pipe input stream -> xz stdin
        let input_to_xz = tokio::spawn(async move {
            match tokio::io::copy(&mut input_stream, &mut xz_stdin).await {
                Ok(bytes) => debug!("Piped {} bytes to xz decompressor", bytes),
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

        let (xz_input_task_res, xz_task_res, btrfs_task_res) = tokio::join!(
            // Copy bytes from incoming stream to xz
            input_to_xz,
            // Run the xz command receiving the stream
            xz_proc.wait(),
            // Use btrfs namespace to receive the stream (xz stdout -> btrfs receive)
            btrfs.receive(&self.deployments_dir, xz_stdout_reader)
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

        let Ok(subvolume_result) = btrfs_task_res
            .as_ref()
            .inspect_err(|e| error!("btrfs receive join error: {e}"))
        else {
            return Err(ServiceError::IOError(std::io::Error::other(format!(
                "joining of btrfs receive failed",
            ))));
        };

        // Wait for xz process to finish
        if !xz_status.success() {
            return Err(ServiceError::IOError(std::io::Error::other(format!(
                "xz decompressor failed with status: {xz_status}"
            ))));
        }

        debug!("xz decompression completed successfully");

        Ok(subvolume_result.clone())
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
    ) -> Result<Option<String>, ServiceError>
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        // Create hashing reader to compute SHA512 during streaming
        let hashing_reader = HashingReader::new(reader);
        let hash_result = hashing_reader.hash_result();

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

        //info!("Installed manifest version: {}", manifest.version);

        // Extract and print the SHA512 hash computed during streaming
        let hash = hash_result.read().await;
        let Some(hash) = hash.as_deref() else {
            debug!("update.btrfs.xz SHA512: (not yet available)");
            return Err(ServiceError::IOError(std::io::Error::other(
                "Failed to compute SHA512 hash of update stream",
            )));
        };

        info!("update.btrfs.xz SHA512: {hash}");

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

            // Update status to Installing and at 0% progress
            let status_handle = data.read().await.update_status.clone();
            *status_handle.write().await = UpdateStatus::Installing {
                source: source_desc.clone(),
                progress: 0,
            };

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

            // Collect CHANGELOG and process update.btrfs.xz
            let mut changelog_content: Option<String> = None;

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
        update_stream: Pin<Box<dyn AsyncRead + Send + Unpin>>,
        update_size: u64,
        source_desc: String,
    ) -> Result<bool, ServiceError> {
        // Extract version from changelog for confirmation/details
        let changelog = changelog_content.ok_or_else(|| {
            ServiceError::IOError(std::io::Error::other("CHANGELOG not found in archive"))
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

        // Clear old deployments before installing new ones
        data.read().await.set_status(UpdateStatus::Clearing).await;
        match Self::clear_old_deployments(data, btrfs).await {
            Ok(count) => {
                info!("Cleared {} old deployments", count);
            }
            Err(err) => {
                warn!("Failed to clear old deployments: {}", err);
                // Continue with installation even if clearing fails
            }
        }

        // Wrap stream with progress tracking
        let status_handle = data.read().await.update_status.clone();
        let wrapped_stream: Pin<Box<dyn AsyncRead + Send + Unpin>> = {
            let progress_reader = ProgressReader::new(
                update_stream,
                Some(update_size),
                status_handle.clone(),
                source_desc.clone(),
                true,
            );
            Box::pin(progress_reader)
        };

        // Install using the stream (tar Entry -> xz -d -> btrfs receive)
        // Hash computation now happens inside install_update
        let result = Self::install_update(data, btrfs, wrapped_stream).await;

        // Update final status
        let status = match result {
            Ok(Some(deployment_name)) => {
                info!("Update installed successfully: {deployment_name}");
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
