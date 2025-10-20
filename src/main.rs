use std::sync::Arc;

use embuer::{config::Config, dbus::EmbuerDBus, service, ServiceError};
use tokio::{
    signal::unix::{signal, SignalKind},
    sync::RwLock,
};
use zbus::connection;

#[tokio::main]
async fn main() -> Result<(), ServiceError> {
    if users::get_current_uid() != 0 {
        eprintln!("🚫 Application started without root privileges: aborting...");
        return Err(ServiceError::MissingPrivilegesError);
    }

    match std::env::var("DBUS_SESSION_BUS_ADDRESS") {
        Ok(value) => println!("Starting dbus service on socket {value}"),
        Err(err) => {
            println!("🟠 Couldn't read dbus socket address: {err} - using default...");
            std::env::set_var(
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/run/dbus/system_bus_socket",
            );
        }
    };

    println!("🔧 Building the dbus object...");

    // Load system configuration (if present). This will return a specific
    // ServiceError::MissingConfigurationError when the file does not exist.
    let cfg_path = match std::fs::exists("/usr/share/embuer/config.json").unwrap_or(false) {
        true => std::path::PathBuf::from("/usr/share/embuer/config.json"),
        false => std::path::PathBuf::from("/etc/embuer/config.json"),
    };
    let config = match Config::load_from(cfg_path) {
        Ok(cfg) => {
            println!("⚙️  Loaded configuration");
            cfg
        }
        Err(e) => match e {
            ServiceError::MissingConfigurationError(path) => {
                println!(
                    "🟡 Configuration not found at {:?}, continuing with defaults",
                    path
                );
                Config::default()
            }
            other => return Err(other),
        },
    };

    let service = Arc::new(RwLock::new(service::Service::new(config.clone())?));

    let dbus_manager = connection::Builder::session()
        .map_err(ServiceError::ZbusError)?
        .name("org.neroreflex.embuer")
        .map_err(ServiceError::ZbusError)?
        .serve_at("/org/neroreflex/embuer", EmbuerDBus::new(service.clone()))
        .map_err(ServiceError::ZbusError)?
        .build()
        .await
        .map_err(ServiceError::ZbusError)?;

    println!("🔄 Application running");

    // Create a signal listener for SIGTERM
    let mut sigterm =
        signal(SignalKind::interrupt()).expect("🚫 Failed to create SIGTERM signal handler");

    // Wait for a SIGTERM signal
    sigterm.recv().await;

    println!("🛑 Termination signal received, shutting down...");
    service.write().await.terminate_update_check().await;

    drop(dbus_manager);

    Ok(())
}
