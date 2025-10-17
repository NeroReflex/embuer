pub extern crate zbus;

pub mod config;
pub mod dbus;
pub mod service;

use zbus::Error as ZError;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("Permission error: not running as the root user")]
    MissingPrivilegesError,

    #[error("Missing configuration: couldn't find the required file or directory")]
    MissingConfigurationError(std::path::PathBuf),

    #[error("DBus error: {0}")]
    ZbusError(#[from] ZError),

    #[error("I/O error: {0}")]
    IOError(#[from] std::io::Error),

    #[error("pkcs1 error: {0}")]
    PKCS1Error(#[from] rsa::pkcs1::Error),

    #[error("Failed to deserialize JSON: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Join error: {0}")]
    JoinError(#[from] tokio::task::JoinError),
}
