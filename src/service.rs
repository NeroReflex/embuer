use crate::{btrfs::Btrfs, config::Config, ServiceError};
use futures::TryStreamExt;
use log::{debug, error, info, warn};
use reqwest::Client;
use rsa::{pkcs1::DecodeRsaPublicKey, RsaPublicKey};
use sha2::{Digest, Sha512};
use std::os::unix::fs::PermissionsExt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader, ReadBuf};
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

/// Current status of the update process
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// No update in progress
    Idle,
    /// Checking for updates
    Checking,
    /// Clearing old deployments
    Clearing,
    /// Downloading update (with progress 0-100, or -1 if unknown)
    Downloading { source: String, progress: i32 },
    /// Installing update (with progress 0-100, or -1 if unknown)
    Installing { source: String, progress: i32 },
    /// Awaiting user confirmation to install
    AwaitingConfirmation { version: String, source: String },
    /// Update completed successfully
    Completed { source: String },
    /// Update failed
    Failed { source: String, error: String },
}

impl UpdateStatus {
    /// Convert status to a string representation for DBus
    pub fn as_str(&self) -> &str {
        match self {
            UpdateStatus::Idle => "Idle",
            UpdateStatus::Checking => "Checking",
            UpdateStatus::Clearing => "Clearing",
            UpdateStatus::Downloading { .. } => "Downloading",
            UpdateStatus::Installing { .. } => "Installing",
            UpdateStatus::AwaitingConfirmation { .. } => "AwaitingConfirmation",
            UpdateStatus::Completed { .. } => "Completed",
            UpdateStatus::Failed { .. } => "Failed",
        }
    }

    /// Get additional details about the status
    pub fn details(&self) -> String {
        match self {
            UpdateStatus::Idle => String::new(),
            UpdateStatus::Checking => String::new(),
            UpdateStatus::Clearing => String::new(),
            UpdateStatus::Downloading { source, .. } => source.clone(),
            UpdateStatus::Installing { source, .. } => source.clone(),
            UpdateStatus::AwaitingConfirmation { version, source } => {
                format!("{} ({})", version, source)
            }
            UpdateStatus::Completed { source } => source.clone(),
            UpdateStatus::Failed { source, error } => format!("{}: {}", source, error),
        }
    }

    /// Get progress percentage (0-100, or -1 if not applicable/unknown)
    pub fn progress(&self) -> i32 {
        match self {
            UpdateStatus::Downloading { progress, .. } => *progress,
            UpdateStatus::Installing { progress, .. } => *progress,
            _ => -1,
        }
    }
}

/// A wrapper around AsyncRead that computes SHA512 hash incrementally
/// The hash result is stored in an Arc for retrieval after streaming completes
pub struct HashingReader<R> {
    inner: R,
    hasher: Sha512,
    hash_result: Arc<RwLock<Option<String>>>,
}

impl<R: AsyncRead + Unpin> HashingReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: Sha512::new(),
            hash_result: Arc::new(RwLock::new(None)),
        }
    }

    pub fn hash_result(&self) -> Arc<RwLock<Option<String>>> {
        self.hash_result.clone()
    }

    fn finalize_hash(&mut self) {
        let hash = std::mem::replace(&mut self.hasher, Sha512::new()).finalize();
        let hex_hash = hex::encode(hash);
        if let Ok(mut result) = self.hash_result.try_write() {
            *result = Some(hex_hash);
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for HashingReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);

        if let Poll::Ready(Ok(())) = &result {
            // Update hasher with the newly read data
            let newly_read = &buf.filled()[before..];
            if !newly_read.is_empty() {
                self.hasher.update(newly_read);
            }
            
            // Check if we've reached EOF (no new data and buffer is at capacity)
            if newly_read.is_empty() && buf.remaining() == 0 {
                // Finalize the hash when stream ends
                self.finalize_hash();
            }
        }

        result
    }
}

impl<R> std::fmt::Debug for HashingReader<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HashingReader")
    }
}

impl<R> Unpin for HashingReader<R> {}

/// A wrapper around AsyncRead that tracks progress
struct ProgressReader<R> {
    inner: R,
    bytes_read: u64,
    total_size: Option<u64>,
    status_handle: Arc<RwLock<UpdateStatus>>,
    source: String,
    is_installing: bool,
    last_update: std::time::Instant,
}

impl<R: AsyncRead + Unpin> ProgressReader<R> {
    fn new(
        inner: R,
        total_size: Option<u64>,
        status_handle: Arc<RwLock<UpdateStatus>>,
        source: String,
        is_installing: bool,
    ) -> Self {
        Self {
            inner,
            bytes_read: 0,
            total_size,
            status_handle,
            source,
            is_installing,
            last_update: std::time::Instant::now(),
        }
    }

    fn calculate_progress(&self) -> i32 {
        self.total_size
            .filter(|&size| size > 0)
            .map(|size| ((self.bytes_read as f64 / size as f64) * 100.0) as i32)
            .unwrap_or(-1)
    }

    fn should_update(&self) -> bool {
        self.last_update.elapsed().as_millis() > 100
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for ProgressReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);

        if let Poll::Ready(Ok(())) = &result {
            let reader = self.get_mut();
            reader.bytes_read += (buf.filled().len() - before) as u64;

            if reader.should_update() {
                reader.last_update = std::time::Instant::now();
                let progress = reader.calculate_progress();
                let status_handle = reader.status_handle.clone();
                let source = reader.source.clone();
                let is_installing = reader.is_installing;

                tokio::spawn(async move {
                    let mut status = status_handle.write().await;
                    *status = match is_installing {
                        true => UpdateStatus::Installing { source, progress },
                        false => UpdateStatus::Downloading { source, progress },
                    };
                });
            }
        }

        result
    }
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

        // Use btrfs namespace to receive the stream (xz stdout -> btrfs receive)
        debug!("Starting btrfs receive...");
        let subvolume_result = btrfs.receive(&self.deployments_dir, xz_stdout).await;

        // Wait for piping task
        if let Err(e) = input_to_xz.await {
            warn!("Piping task panicked: {}", e);
        }

        // Wait for xz process to finish
        let xz_status = xz_proc.wait().await?;
        if !xz_status.success() {
            let error_msg = format!("xz decompressor failed with status: {}", xz_status);
            error!("{}", error_msg);
            return Err(ServiceError::IOError(std::io::Error::other(error_msg)));
        }

        debug!("xz decompression completed successfully");

        // Check if btrfs receive succeeded
        match subvolume_result {
            Ok(subvol) => Ok(subvol),
            Err(e) => {
                // Check if the error is due to an existing subvolume
                let error_string = e.to_string();
                if error_string.contains("already exists") {
                    warn!("Subvolume already exists, this should have been cleaned up");
                }
                Err(e)
            }
        }
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
        hash_arc: Arc<RwLock<Option<String>>>,
    ) -> Result<(), ServiceError>
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let subvolume = data
            .read()
            .await
            .receive_btrfs_stream(btrfs, reader)
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

        info!("Installed deployment {name} (ID={subvol_id})");

        // Extract and print the SHA512 hash computed during streaming
        if let Some(hash) = hash_arc.read().await.as_ref() {
            info!("update.btrfs.xz SHA512: {}", hash);
        } else {
            debug!("update.btrfs.xz SHA512: (not yet available)");
        }

        // Make the new subvolume the default for next boot
        btrfs.subvolume_set_default(subvol_id, &rootfs_path)?;

        Ok(())
    }

    /// Extract changelog and update stream from URL
    ///
    /// IMPORTANT: The update.btrfs.xz file is NEVER extracted to disk.
    /// Instead, the tar Entry is returned as a streaming AsyncRead that
    /// will be piped directly through xz decompression to btrfs receive.
    /// Only the CHANGELOG (small text file) is read into memory.
    /// 
    /// Returns: (changelog, update_stream, hash_arc)
    async fn extract_url_update_contents(
        url: String,
        status_handle: Arc<RwLock<UpdateStatus>>,
    ) -> Result<(String, Pin<Box<dyn AsyncRead + Send + Unpin>>, Arc<RwLock<Option<String>>>), ServiceError> {
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
                info!("No update available (404 Not Found) at {}", url);
            } else {
                info!("Server returned {} at {}, treating as no update available", status_code.as_u16(), url);
            }
            return Err(ServiceError::NoUpdateAvailable);
        }

        info!("Downloading update from {}", url);
        let total_size = resp.content_length();
        total_size.map(|size| info!("Download size: {} bytes", size));

        // Set initial downloading status
        *status_handle.write().await = UpdateStatus::Downloading {
            source: url.clone(),
            progress: 0,
        };

        let byte_stream = resp.bytes_stream().map_err(std::io::Error::other);
        let stream_reader = StreamReader::new(byte_stream);

        let progress_reader = ProgressReader::new(
            stream_reader,
            total_size,
            status_handle.clone(),
            url.clone(),
            false,
        );

        // The stream is a tar archive - extract it
        let mut archive = Archive::new(progress_reader);
        let mut entries = archive.entries().unwrap();

        let mut changelog_content = String::new();
        let mut update_stream = None;
        let mut update_size: Option<u64> = None;

        'update: while let Some(file) = entries.next().await {
            match file {
                Ok(entry) => {
                    let path = entry.path().unwrap();
                    let path_str = path.display().to_string();
                    debug!("Found archive entry: {}", path_str);

                    // IMPORTANT: CHANGELOG must come before update.btrfs.xz in the archive
                    if path_str == "CHANGELOG" {
                        debug!("Found CHANGELOG entry");
                        let mut content = String::new();
                        let mut reader = BufReader::new(entry);
                        reader.read_to_string(&mut content).await?;
                        changelog_content = content;
                        info!("Extracted CHANGELOG ({} bytes)", changelog_content.len());
                    } else if path_str == "update.btrfs.xz" {
                        debug!("Found update.btrfs.xz entry");
                        // Get the entry size for progress tracking (size of update.btrfs.xz, not the tar)
                        let header = entry.header();
                        let size = header.entry_size().unwrap_or(0);
                        update_size = Some(size);
                        info!("update.btrfs.xz size: {} bytes", size);
                        // The entry itself is an AsyncRead, so we can use it directly
                        // Wrap it in a box to make it 'static
                        update_stream =
                            Some(Box::pin(entry) as Pin<Box<dyn AsyncRead + Send + Unpin>>);
                        // Continue reading in case there are more entries
                    }
                }
                Err(e) => {
                    error!("Error reading archive entry: {e}");
                    break 'update;
                }
            }
        }

        let update_stream = update_stream.ok_or_else(|| {
            ServiceError::IOError(std::io::Error::other(
                "update.btrfs.xz not found in archive",
            ))
        })?;

        // Wrap the stream with HashingReader to compute SHA512 while streaming
        info!("Starting SHA512 computation for update.btrfs.xz");
        let hashing_reader = HashingReader::new(update_stream);
        let hash_arc = hashing_reader.hash_result();
        
        // Wrap with ProgressReader if we have a size
        let wrapped_stream: Pin<Box<dyn AsyncRead + Send + Unpin>> = if let Some(size) = update_size {
            info!(
                "Wrapping update stream with progress tracking for {} bytes",
                size
            );
            let progress_reader = ProgressReader::new(
                Box::pin(hashing_reader),
                Some(size),
                status_handle.clone(),
                url.clone(),
                true, // This is installation phase
            );
            Box::pin(progress_reader)
        } else {
            Box::pin(hashing_reader)
        };

        Ok((changelog_content, wrapped_stream, hash_arc))
    }

    /// Process update from URL
    ///
    /// IMPORTANT: This streams the update.btrfs.xz file directly from the tar archive
    /// through xz decompression and into btrfs receive. The file is NEVER written to disk.
    async fn process_url_update(
        data: Arc<RwLock<ServiceInner>>,
        btrfs: Arc<Btrfs>,
        url: String,
        update_stream: Pin<Box<dyn AsyncRead + Send + Unpin>>,
        hash_arc: Arc<RwLock<Option<String>>>,
    ) -> Result<(), ServiceError> {
        let status_handle = data.read().await.update_status.clone();

        // Transition to installing
        *status_handle.write().await = UpdateStatus::Installing {
            source: url,
            progress: 0,
        };

        // CRITICAL: Stream update.btrfs.xz directly through pipes without touching disk
        // The stream goes: tar Entry -> xz -d -> btrfs receive (all in memory/pipes)
        // Note: SHA512 hash is computed during streaming inside HashingReader
        Self::install_update(&data, &btrfs, update_stream, hash_arc).await
    }

    /// Extract changelog and update stream from file
    ///
    /// IMPORTANT: The update.btrfs.xz file is NEVER extracted to disk.
    /// Instead, the tar Entry is returned as a streaming AsyncRead that
    /// will be piped directly through xz decompression to btrfs receive.
    /// Only the CHANGELOG (small text file) is read into memory.
    async fn extract_file_update_contents(
        path: std::path::PathBuf,
        status_handle: Arc<RwLock<UpdateStatus>>,
    ) -> Result<(String, Pin<Box<dyn AsyncRead + Send + Unpin>>, Arc<RwLock<Option<String>>>), ServiceError> {
        let file = File::open(&path).await?;

        info!("Opening update archive from file: {}", path.display());
        let total_size = file.metadata().await.ok().map(|m| m.len());
        total_size.map(|size| info!("File size: {} bytes", size));

        // The file is a tar archive - extract it
        let mut archive = Archive::new(file);
        let mut entries = archive.entries().unwrap();

        let mut changelog_content = String::new();
        let mut update_stream = None;
        let mut update_size: Option<u64> = None;

        'update: while let Some(file) = entries.next().await {
            match file {
                Ok(entry) => {
                    let path = entry.path().unwrap();
                    let path_str = path.display().to_string();
                    debug!("Found archive entry: {}", path_str);

                    // IMPORTANT: CHANGELOG must come before update.btrfs.xz in the archive
                    if path_str == "CHANGELOG" {
                        debug!("Found CHANGELOG entry");
                        let mut content = String::new();
                        let mut reader = BufReader::new(entry);
                        reader.read_to_string(&mut content).await?;
                        changelog_content = content;
                        info!("Extracted CHANGELOG ({} bytes)", changelog_content.len());
                    } else if path_str == "update.btrfs.xz" {
                        debug!("Found update.btrfs.xz entry");
                        // Get the entry size for progress tracking (size of update.btrfs.xz, not the tar)
                        let header = entry.header();
                        let size = header.entry_size().unwrap_or(0);
                        update_size = Some(size);
                        info!("update.btrfs.xz size: {} bytes", size);
                        // The entry itself is an AsyncRead, so we can use it directly
                        // Wrap it in a box to make it 'static
                        update_stream =
                            Some(Box::pin(entry) as Pin<Box<dyn AsyncRead + Send + Unpin>>);
                        // Continue reading in case there are more entries
                    }
                }
                Err(e) => {
                    error!("Error reading archive entry: {e}");
                    break 'update;
                }
            }
        }

        let mut update_stream = update_stream.ok_or_else(|| {
            ServiceError::IOError(std::io::Error::other(
                "update.btrfs.xz not found in archive",
            ))
        })?;

        // Wrap the stream with HashingReader to compute SHA512 while streaming
        info!("Starting SHA512 computation for update.btrfs.xz");
        let hashing_reader = HashingReader::new(update_stream);
        let hash_arc = hashing_reader.hash_result();
        
        // Wrap with ProgressReader if we have a size
        let wrapped_stream: Pin<Box<dyn AsyncRead + Send + Unpin>> = if let Some(size) = update_size {
            info!(
                "Wrapping update stream with progress tracking for {} bytes",
                size
            );
            let progress_reader = ProgressReader::new(
                Box::pin(hashing_reader),
                Some(size),
                status_handle,
                path.display().to_string(),
                true, // This is installation phase
            );
            Box::pin(progress_reader)
        } else {
            Box::pin(hashing_reader)
        };

        Ok((changelog_content, wrapped_stream, hash_arc))
    }

    /// Process update from file
    ///
    /// IMPORTANT: This streams the update.btrfs.xz file directly from the tar archive
    /// through xz decompression and into btrfs receive. The file is NEVER written to disk.
    async fn process_file_update(
        data: Arc<RwLock<ServiceInner>>,
        btrfs: Arc<Btrfs>,
        path: std::path::PathBuf,
        update_stream: Pin<Box<dyn AsyncRead + Send + Unpin>>,
        hash_arc: Arc<RwLock<Option<String>>>,
    ) -> Result<(), ServiceError> {
        let path_str = path.display().to_string();
        let status_handle = data.read().await.update_status.clone();

        *status_handle.write().await = UpdateStatus::Installing {
            source: path_str,
            progress: 0,
        };

        // CRITICAL: Stream update.btrfs.xz directly through pipes without touching disk
        // The stream goes: tar Entry -> xz -d -> btrfs receive (all in memory/pipes)
        // Note: SHA512 hash is computed during streaming inside HashingReader
        Self::install_update(&data, &btrfs, update_stream, hash_arc).await
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

        while let Some(request) = update_rx.recv().await {
            info!("Processing update request: {:?}", request.source);

            let source_desc = match &request.source {
                UpdateSource::Url(url) => url.clone(),
                UpdateSource::File(path) => path.display().to_string(),
            };

            // Extract changelog and prepare update stream from the archive
            // This reads the tar archive (CHANGELOG must come before update.btrfs.xz)
            info!("Extracting archive contents...");
            let status_handle = data.read().await.update_status.clone();
            let (changelog, update_stream, version, hash_arc) = match request.source.clone() {
                UpdateSource::Url(url) => {
                    let (changelog_content, update_stream, hash_arc) =
                        match Self::extract_url_update_contents(url, status_handle).await {
                            Ok(result) => result,
                            Err(ServiceError::NoUpdateAvailable) => {
                                // No update available is not an error, just return to Idle
                                info!("No update available at {}", source_desc);
                                data.read()
                                    .await
                                    .set_status(UpdateStatus::Idle)
                                    .await;
                                continue;
                            }
                            Err(err) => {
                                error!("Failed to extract update contents: {}", err);
                                data.read()
                                    .await
                                    .set_status(UpdateStatus::Failed {
                                        source: source_desc,
                                        error: err.to_string(),
                                    })
                                    .await;
                                continue;
                            }
                        };
                    // Extract version from changelog (first line usually has version info)
                    let version = extract_version_from_changelog(&changelog_content);
                    (changelog_content, update_stream, version, hash_arc)
                }
                UpdateSource::File(path) => {
                    let (changelog_content, update_stream, hash_arc) =
                        match Self::extract_file_update_contents(
                            path.clone(),
                            status_handle.clone(),
                        )
                        .await
                        {
                            Ok(result) => result,
                            Err(err) => {
                                error!("Failed to extract update contents: {}", err);
                                data.read()
                                    .await
                                    .set_status(UpdateStatus::Failed {
                                        source: source_desc,
                                        error: err.to_string(),
                                    })
                                    .await;
                                continue;
                            }
                        };
                    // Extract version from changelog (first line usually has version info)
                    let version = extract_version_from_changelog(&changelog_content);
                    (changelog_content, update_stream, version, hash_arc)
                }
            };

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

                info!("Waiting for user confirmation...");

                // SECURITY: Wait for confirmation - this blocks until a valid confirmation is received
                // The channel has minimal capacity (1), combined with pending_update guards to prevent premature confirmations
                match confirmation_rx.recv().await {
                    Some(true) => {
                        info!("Update accepted by user, proceeding with installation");
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
                        continue;
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
                        continue;
                    }
                }
            }

            // Step 1: Clear old deployments before installing new ones
            data.read().await.set_status(UpdateStatus::Clearing).await;
            match Self::clear_old_deployments(&data, &btrfs).await {
                Ok(count) => {
                    info!("Cleared {} old deployments", count);
                }
                Err(err) => {
                    warn!("Failed to clear old deployments: {}", err);
                    // Continue with installation even if clearing fails
                }
            }

            // Step 2: Process the update (download/install) with the already-extracted stream
            // Note: changelog was already used for confirmation above, we don't pass it here
            let (source_desc, result) = match request.source {
                UpdateSource::Url(url) => {
                    let desc = url.clone();
                    let result =
                        Self::process_url_update(data.clone(), btrfs.clone(), url, update_stream, hash_arc.clone())
                            .await;
                    (desc, result)
                }
                UpdateSource::File(path) => {
                    let desc = path.display().to_string();
                    let result =
                        Self::process_file_update(data.clone(), btrfs.clone(), path, update_stream, hash_arc.clone())
                            .await;
                    (desc, result)
                }
            };

            // Update final status
            let status = match result {
                Ok(_) => {
                    info!("Update installed successfully");
                    UpdateStatus::Completed {
                        source: source_desc,
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
        }

        info!("Update request loop stopped");
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

                    // TODO: In a real implementation, you would check if an update is available
                    // before sending the request (e.g., by checking a version endpoint)
                    info!("Checking for updates at {}", update_url);

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
