use std::sync::Arc;

use tokio::sync::RwLock;
use zbus::object_server::SignalEmitter;
use zbus::{fdo, interface};

use crate::service::{Service, UpdateRequest, UpdateSource, UpdateStatus};

pub struct EmbuerDBus {
    service: Arc<RwLock<Service>>,
}

impl EmbuerDBus {
    pub fn new(service: Arc<RwLock<Service>>) -> Self {
        Self { service }
    }

    /// Start a background task to monitor status changes and emit DBus signals
    pub async fn start_status_monitor(
        service: Arc<RwLock<Service>>,
        signal_emitter: SignalEmitter<'static>,
    ) {
        tokio::spawn(async move {
            let status_handle = {
                let svc = service.read().await;
                svc.update_status_handle().await
            };

            let mut last_status = UpdateStatus::Idle;

            loop {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                let current_status = status_handle.read().await.clone();

                if current_status != last_status {
                    println!("Status changed: {:?} -> {:?}", last_status, current_status);

                    // Emit DBus signal with progress
                    if let Err(e) = EmbuerDBus::update_status_changed(
                        &signal_emitter,
                        current_status.as_str(),
                        &current_status.details(),
                        current_status.progress(),
                    )
                    .await
                    {
                        eprintln!("Failed to emit DBus signal: {}", e);
                    }

                    last_status = current_status;
                }
            }
        });
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

    /// Get the current update status (state, details, and progress)
    /// Returns: (status: String, details: String, progress: i32)
    /// Progress is 0-100 for downloads/installs, -1 for other states or unknown size
    async fn get_update_status(&self) -> fdo::Result<(String, String, i32)> {
        let service = self.service.read().await;
        let status = service.get_update_status().await;
        Ok((
            status.as_str().to_string(),
            status.details(),
            status.progress(),
        ))
    }

    /// DBus signal emitted when update status changes
    /// Arguments: status (string), details (string), progress (i32: 0-100, or -1 if N/A)
    #[zbus(signal)]
    async fn update_status_changed(
        signal_emitter: &SignalEmitter<'_>,
        status: &str,
        details: &str,
        progress: i32,
    ) -> zbus::Result<()>;
}
