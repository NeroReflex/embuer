use std::sync::Arc;

use tokio::sync::RwLock;
use zbus::{fdo, interface};

use crate::service::{Service, UpdateRequest, UpdateSource};

pub struct EmbuerDBus {
    service: Arc<RwLock<Service>>,
}

impl EmbuerDBus {
    pub fn new(service: Arc<RwLock<Service>>) -> Self {
        Self { service }
    }
}

#[interface(
    name = "org.neroreflex.embuer1",
    proxy(
        default_service = "org.neroreflex.embuer",
        default_path = "/org/neroreflex/login_ng_service"
    )
)]
impl EmbuerDBus {
    /// Install an update from a file path
    async fn install_update_from_file(&self, file_path: String) -> fdo::Result<String> {
        let service = self.service.read().await;
        let update_tx = service.update_sender();

        let path = std::path::PathBuf::from(&file_path);
        if !path.exists() {
            return Err(fdo::Error::Failed(format!(
                "File does not exist: {}",
                file_path
            )));
        }

        let request = UpdateRequest {
            source: UpdateSource::File(path),
        };

        update_tx
            .send(request)
            .await
            .map_err(|e| fdo::Error::Failed(format!("Failed to send update request: {}", e)))?;

        Ok(format!("Update request queued for file: {}", file_path))
    }

    /// Install an update from a URL
    async fn install_update_from_url(&self, url: String) -> fdo::Result<String> {
        let service = self.service.read().await;
        let update_tx = service.update_sender();

        let request = UpdateRequest {
            source: UpdateSource::Url(url.clone()),
        };

        update_tx
            .send(request)
            .await
            .map_err(|e| fdo::Error::Failed(format!("Failed to send update request: {}", e)))?;

        Ok(format!("Update request queued for URL: {}", url))
    }
}
