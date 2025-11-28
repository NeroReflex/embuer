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

use std::sync::Arc;

use log::{debug, error};
use tokio::sync::RwLock;
use zbus::object_server::SignalEmitter;
use zbus::{fdo, interface};

use crate::{
    service::{Service, UpdateRequest, UpdateSource},
    status::UpdateStatus,
};

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
            let mut last_progress = -1;

            loop {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;

                let current_status = status_handle.read().await.clone();
                let current_progress = current_status.progress();

                // Emit signal if status changed OR progress changed
                let should_emit = current_status.as_str() != last_status.as_str()
                    || current_status.details() != last_status.details()
                    || current_progress != last_progress;

                if should_emit {
                    debug!(
                        "Status update: {} - {} ({}%)",
                        current_status.as_str(),
                        current_status.details(),
                        current_progress
                    );

                    // Emit DBus signal with progress
                    if let Err(e) = EmbuerDBus::update_status_changed(
                        &signal_emitter,
                        current_status.as_str(),
                        &current_status.details(),
                        current_progress,
                    )
                    .await
                    {
                        error!("Failed to emit DBus signal: {}", e);
                    }

                    last_status = current_status;
                    last_progress = current_progress;
                }
            }
        });
    }
}

#[interface(
    name = "org.neroreflex.embuer1",
    proxy(
        default_service = "org.neroreflex.embuer",
        default_path = "/org/neroreflex/embuer"
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

    /// Get the boot deployment information
    /// Returns: The subvolume ID and name of the currently running deployment
    async fn get_boot_info(&self) -> fdo::Result<(u64, String)> {
        let service = self.service.read().await;
        Ok((service.get_boot_id().await, service.get_boot_name().await))
    }

    /// Get the pending update awaiting confirmation
    /// Returns: (version: String, changelog: String, source: String)
    /// Returns an error if no update is pending
    async fn get_pending_update(&self) -> fdo::Result<(String, String, String)> {
        let service = self.service.read().await;
        match service.get_pending_update().await {
            Some(pending) => Ok((pending.version, pending.changelog, pending.source)),
            None => Err(fdo::Error::Failed(
                "No pending update awaiting confirmation".to_string(),
            )),
        }
    }

    /// Confirm or reject the pending update
    ///
    /// Parameters:
    /// - accepted: true to accept and install, false to reject
    async fn confirm_update(&self, accepted: bool) -> fdo::Result<String> {
        let service = self.service.read().await;
        service
            .confirm_update(accepted)
            .await
            .map_err(|e| fdo::Error::Failed(format!("Failed to confirm update: {}", e)))?;

        if accepted {
            Ok("Update accepted, installation will proceed".to_string())
        } else {
            Ok("Update rejected".to_string())
        }
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
