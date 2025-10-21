use crate::{btrfs::Btrfs, config::Config, ServiceError};
use futures::TryStreamExt;
use reqwest::Client;
use rsa::{pkcs1::DecodeRsaPublicKey, RsaPublicKey};
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::{sync::RwLock, task::JoinHandle};
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

pub struct ServiceInner {
    pubkey: RsaPublicKey,
    notify: Arc<tokio::sync::Notify>,
    rootfs_dir: std::path::PathBuf,
    deployments_dir: std::path::PathBuf,
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
            ServiceError::IOError(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Failed to open stdin for xz",
            ))
        })?;

        let xz_stdout = xz_proc.stdout.take().ok_or_else(|| {
            ServiceError::IOError(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Failed to open stdout for xz",
            ))
        })?;

        // Pipe input stream -> xz stdin
        let input_to_xz = tokio::spawn(async move {
            let result = tokio::io::copy(&mut input_stream, &mut xz_stdin).await;
            if let Err(e) = result {
                eprintln!("Error piping data to xz: {}", e);
            }
            // Signal EOF to xz
            let _ = xz_stdin.shutdown().await;
        });

        // Use btrfs namespace to receive the stream (xz stdout -> btrfs receive)
        let subvolume = btrfs.receive(&self.deployments_dir, xz_stdout).await;

        // Wait for piping task
        let _ = input_to_xz.await;

        // Wait for xz process to finish
        let xz_status = xz_proc.wait().await?;
        if !xz_status.success() {
            return Err(ServiceError::IOError(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("xz -d failed with status: {}", xz_status),
            )));
        }

        // Return the received subvolume name
        subvolume
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

        let service_data = Arc::new(RwLock::new(ServiceInner {
            pubkey,
            notify,
            rootfs_dir,
            deployments_dir,
        }));

        let btrfs = Arc::new(btrfs);

        // Create channel for update requests (from DBus, periodic checker, etc.)
        let (update_tx, update_rx) = mpsc::channel::<UpdateRequest>(10);

        // Spawn the main update request loop that processes all update requests from the channel
        let update_request_loop = Some({
            let service_data_clone = service_data.clone();
            let btrfs_clone = btrfs.clone();
            tokio::spawn(async move {
                Self::update_request_loop(service_data_clone, btrfs_clone, update_rx).await
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
                Ok(_) => println!("Periodic URL checker task terminated successfully"),
                Err(err) => eprintln!("Error terminating periodic URL checker task: {err}"),
            }
        }

        // Wait for update request loop to finish
        if let Some(request_loop) = self.update_request_loop.take() {
            match request_loop.await {
                Ok(_) => println!("Update request loop task terminated successfully"),
                Err(err) => eprintln!("Error terminating update request loop task: {err}"),
            }
        }
    }

    /// Common method to install an update from a reader
    async fn install_update<R>(
        data: &Arc<RwLock<ServiceInner>>,
        btrfs: &Arc<Btrfs>,
        reader: R,
    ) -> Result<(), ServiceError>
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let subvolume = data
            .read()
            .await
            .receive_btrfs_stream(btrfs, reader)
            .await?;
        match subvolume {
            Some(name) => {
                println!("Received subvolume: {}", name);
                let subvolume_path = data.read().await.deployments_dir.join(&name);
                match btrfs.btrfs_subvol_get_id(subvolume_path) {
                    Ok(subvolid) => {
                        println!("Created btrfs subvolume with id {subvolid}");
                        Ok(())
                    }
                    Err(e) => {
                        eprintln!("Error checking if subvolume is a btrfs subvolume: {e}");
                        Err(ServiceError::IOError(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            format!("BTRFS subvolume error: {}", e),
                        )))
                    }
                }
            }
            None => {
                eprintln!("No subvolume name found in btrfs receive output");
                Err(ServiceError::IOError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "No subvolume name found",
                )))
            }
        }
    }

    /// Main update request loop that processes all update requests from the channel.
    /// This handles requests from DBus, periodic checker, or any other source.
    async fn update_request_loop(
        data: Arc<RwLock<ServiceInner>>,
        btrfs: Arc<Btrfs>,
        mut update_rx: mpsc::Receiver<UpdateRequest>,
    ) {
        println!("Update request loop started");

        while let Some(request) = update_rx.recv().await {
            println!("Processing update request: {:?}", request.source);

            let result = match request.source {
                UpdateSource::Url(url) => {
                    // Download from URL
                    let client = Client::new();
                    match client.get(&url).send().await {
                        Ok(resp) => {
                            println!("Downloading update from {}", url);
                            let byte_stream = resp
                                .bytes_stream()
                                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
                            let reader = StreamReader::new(byte_stream);

                            Self::install_update(&data, &btrfs, reader).await
                        }
                        Err(err) => {
                            eprintln!("Failed to download from {}: {}", url, err);
                            Err(ServiceError::IOError(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                err,
                            )))
                        }
                    }
                }
                UpdateSource::File(path) => {
                    // Read from file
                    match File::open(&path).await {
                        Ok(file) => {
                            println!("Installing update from file: {}", path.display());
                            Self::install_update(&data, &btrfs, file).await
                        }
                        Err(err) => {
                            eprintln!("Failed to open file {}: {}", path.display(), err);
                            Err(ServiceError::IOError(err))
                        }
                    }
                }
            };

            match result {
                Ok(_) => println!("Update installed successfully"),
                Err(err) => eprintln!("Failed to install update: {}", err),
            }
        }

        println!("Update request loop stopped");
    }

    /// Periodic URL checker task that sends update requests to the channel at regular intervals.
    /// This is a simple task with the sole responsibility of triggering periodic updates.
    async fn periodic_url_checker(
        update_url: String,
        update_tx: mpsc::Sender<UpdateRequest>,
        notify: Arc<tokio::sync::Notify>,
    ) {
        println!("Periodic URL checker started for {}", update_url);

        let mut update_done = false;

        'check: loop {
            tokio::select! {
                _ = notify.notified() => {
                    println!("Periodic URL checker received termination signal");
                    break 'check;
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                    // Skip if update already done
                    if update_done {
                        continue 'check;
                    }

                    // TODO: In a real implementation, you would check if an update is available
                    // before sending the request (e.g., by checking a version endpoint)
                    println!("Checking for updates at {}", update_url);

                    // Send update request to the channel
                    let request = UpdateRequest {
                        source: UpdateSource::Url(update_url.clone()),
                    };

                    match update_tx.send(request).await {
                        Ok(_) => {
                            println!("Periodic update request sent");
                            update_done = true;
                        }
                        Err(err) => {
                            eprintln!("Failed to send periodic update request: {}", err);
                            break 'check;
                        }
                    }
                }
            }
        }

        println!("Periodic URL checker stopped");
    }
}
