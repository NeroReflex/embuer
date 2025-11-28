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

use embuer::{config::Config, dbus::EmbuerDBus, service, ServiceError};
use log::{error, info, warn};
use tokio::{
    signal::unix::{signal, SignalKind},
    sync::RwLock,
};
use zbus::connection;

#[tokio::main]
async fn main() -> Result<(), ServiceError> {
    // Initialize logger
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    if users::get_current_uid() != 0 {
        error!("Application started without root privileges: aborting...");
        return Err(ServiceError::MissingPrivilegesError);
    }

    info!("Building the dbus object...");

    // Load system configuration (if present). This will return a specific
    // ServiceError::MissingConfigurationError when the file does not exist.
    let cfg_path = match std::fs::exists("/usr/share/embuer/config.json").unwrap_or(false) {
        true => std::path::PathBuf::from("/usr/share/embuer/config.json"),
        false => std::path::PathBuf::from("/etc/embuer/config.json"),
    };
    let config = match Config::load_from(cfg_path) {
        Ok(cfg) => {
            info!("Loaded configuration");
            cfg
        }
        Err(e) => match e {
            ServiceError::MissingConfigurationError(path) => {
                warn!(
                    "Configuration not found at {:?}, continuing with defaults",
                    path
                );
                Config::default()
            }
            other => return Err(other),
        },
    };

    // Probe for btrfs tool and construct wrapper. If this fails, terminate.
    let btrfs = match embuer::btrfs::Btrfs::new() {
        Ok(b) => {
            info!("Found btrfs: {}", b.version());
            b
        }
        Err(e) => {
            error!("btrfs probe failed: {e}");
            return Err(e);
        }
    };

    let service = Arc::new(RwLock::new(service::Service::new(config.clone(), btrfs)?));

    let dbus_manager = connection::Builder::system()
        .map_err(ServiceError::ZbusError)?
        .name("org.neroreflex.embuer")
        .map_err(ServiceError::ZbusError)?
        .serve_at("/org/neroreflex/embuer", EmbuerDBus::new(service.clone()))
        .map_err(ServiceError::ZbusError)?
        .build()
        .await
        .map_err(ServiceError::ZbusError)?;

    // Start the status monitor to emit DBus signals on status changes
    let interface_ref = dbus_manager
        .object_server()
        .interface::<_, EmbuerDBus>("/org/neroreflex/embuer")
        .await
        .map_err(ServiceError::ZbusError)?;

    let signal_emitter = interface_ref.signal_emitter().clone();
    EmbuerDBus::start_status_monitor(service.clone(), signal_emitter).await;

    info!("Application running (status monitor active)");

    // Create a signal listener for SIGTERM
    let mut sigterm = match signal(SignalKind::interrupt()) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to create SIGTERM signal handler: {}", e);
            return Err(ServiceError::IOError(e));
        }
    };

    // Wait for a SIGTERM signal
    sigterm.recv().await;

    info!("Termination signal received, shutting down...");
    service.write().await.terminate_update_check().await;

    drop(dbus_manager);

    Ok(())
}
