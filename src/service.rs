use rsa::{
    pkcs1::DecodeRsaPublicKey, Error as RSAError, Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey,
};
use crate::{config::Config, ServiceError};
use std::sync::Arc;
use tokio::{sync::RwLock, task::JoinHandle};

pub struct ServiceInnser {
    
}

pub struct Service {
    config: Config,
    pubkey: RsaPublicKey,

    update_checker: JoinHandle<()>,
}

impl Service {
    pub fn public_key(&self) -> &RsaPublicKey {
        &self.pubkey
    }

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

        let service_data = Arc::new(RwLock::new(ServiceInnser {
            
        }));

        let update_checker = {
            let service_data_clone = service_data.clone();
            tokio::spawn(async move {
                Self::update_check(service_data_clone);
            })
        };

        Ok(Self { config, pubkey, update_checker })
    }

    pub fn update_check(data: Arc<RwLock<ServiceInnser>>) {
        
    }

}
