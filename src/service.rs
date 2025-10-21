use crate::{btrfs::Btrfs, config::Config, ServiceError};
use futures::TryStreamExt;
use reqwest::Client;
use rsa::{pkcs1::DecodeRsaPublicKey, RsaPublicKey};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWriteExt};
use tokio::process::Command;
use tokio::{sync::RwLock, task::JoinHandle};
use tokio_stream::StreamExt;
use tokio_tar::Archive;
use tokio_util::io::StreamReader;

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

    update_checker: Option<JoinHandle<()>>,
}

impl Drop for Service {
    fn drop(&mut self) {
        //assert!(
        //    self.update_checker.is_none(),
        //    "Update checker task not terminated before Service drop"
        //);
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

        let update_checker = Some({
            let service_data_clone = service_data.clone();
            let btrfs_clone = btrfs.clone();
            tokio::spawn(async move { Self::update_check(service_data_clone, btrfs_clone).await })
        });

        Ok(Self {
            config,
            service_data,
            btrfs,
            update_checker,
        })
    }

    pub async fn terminate_update_check(&mut self) {
        let data_lock = self.service_data.read().await;
        data_lock.notify.notify_waiters();

        match self.update_checker.take().unwrap().await {
            Ok(_) => println!("Update checker task terminated successfully"),
            Err(err) => eprintln!("Error terminating update checker task: {err}"),
        }
    }

    async fn handle_archive<R>(mut archive: Archive<R>)
    where
        R: AsyncRead + Unpin,
    {
        let mut entries = archive.entries().unwrap();
        'update: while let Some(file) = entries.next().await {
            match file {
                Ok(f) => {
                    println!("{}", f.path().unwrap().display());
                    //f.take(limit)
                }
                Err(e) => {
                    eprintln!("Error reading archive entry: {e}");
                    break 'update;
                }
            }
        }
    }

    pub async fn update_check(data: Arc<RwLock<ServiceInner>>, btrfs: Arc<Btrfs>) {
        let notifications_source = {
            let data_lock = data.read().await;
            data_lock.notify.clone()
        };

        let client = Client::new();
        let mut update_done = false;

        'check: while !update_done {
            tokio::select! {
                _ = notifications_source.notified() => break 'check,
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                    // if an update has already been done just loop waiting for notification
                    if update_done { continue 'check; }

                    match client.get("http://10.0.0.33:8080/factory.btrfs.xz").send().await {
                        Ok(resp) => {
                            // reqwest gives Stream<Item = Result<Bytes, reqwest::Error>>
                            let byte_stream = resp.bytes_stream()
                                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));

                            // StreamReader expects Stream<Item = Result<impl Buf, E>>
                            let reader = StreamReader::new(byte_stream);

                            let subvolume = data.read().await.receive_btrfs_stream(&btrfs, reader).await.unwrap();
                            match subvolume {
                                Some(name) => {
                                    println!("Received subvolume: {}", name);
                                    let subvolume_path = data.read().await.deployments_dir.join(name);
                                    match btrfs.btrfs_subvol_get_id(subvolume_path) {
                                        Ok(subvolid) => println!("Created btrfs subvolume with id {subvolid}"),
                                        Err(e) => println!("Error checking if subvolume is a btrfs subvolume: {e}"),
                                    }
                                },
                                None => println!("No subvolume name found in btrfs receive output"),
                            }
/*
                            // reader implements AsyncRead + AsyncBufRead + Unpin -> usable by tokio_tar
                            let archive = Archive::new(reader);
                            // Example usage of btrfs inside update_check:
                            // (Currently just demonstrate access to version)
                            println!("btrfs version in update_check: {}", btrfs.version());
                            Self::handle_archive(archive).await;
*/
                            println!("Update applied successfully");

                            update_done = true;
                        }
                        Err(err) => {
                            eprintln!("request error: {err}");
                        }
                    }
                }
            }
        }
    }
}
