use crate::{config::Config, ServiceError};
use bytes::Bytes;
use futures::TryStreamExt;
use reqwest::Client;
use rsa::{
    pkcs1::DecodeRsaPublicKey, Error as RSAError, Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey,
};
use std::sync::Arc;
use tokio::io::AsyncRead as Read;
use tokio::{io::SimplexStream, sync::RwLock, task::JoinHandle};
use tokio_stream::StreamExt;
use tokio_tar::Archive;
use tokio_util::io::StreamReader; // for map_ok / map_err if desired

pub struct ServiceInner {
    pubkey: RsaPublicKey,
    notify: Arc<tokio::sync::Notify>,
}

pub struct Service {
    config: Config,
    service_data: Arc<RwLock<ServiceInner>>,

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
    pub fn new(config: Config) -> Result<Self, ServiceError> {
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

        let service_data = Arc::new(RwLock::new(ServiceInner { pubkey, notify }));

        let update_checker = Some({
            let service_data_clone = service_data.clone();
            tokio::spawn(async move { Self::update_check(service_data_clone).await })
        });

        Ok(Self {
            config,
            service_data,
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
        R: Read + Unpin,
    {
        let mut entries = archive.entries().unwrap();
        while let Some(file) = entries.next().await {
            let f = file.unwrap();
            println!("{}", f.path().unwrap().display());
        }
    }

    pub async fn update_check(data: Arc<RwLock<ServiceInner>>) {
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

                    match client.get("http://192.168.0.93:8080/some.tar").send().await {
                        Ok(resp) => {
                            // reqwest gives Stream<Item = Result<Bytes, reqwest::Error>>
                            let byte_stream = resp.bytes_stream()
                                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));

                            // StreamReader expects Stream<Item = Result<impl Buf, E>>
                            let reader = StreamReader::new(byte_stream);

                            // reader implements AsyncRead + AsyncBufRead + Unpin -> usable by tokio_tar
                            let mut archive = Archive::new(reader);
                            Self::handle_archive(archive).await;

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
