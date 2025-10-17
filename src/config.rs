use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

use crate::ServiceError;

#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
pub struct Config {
    update_url: Option<String>,
    check_for_updates: bool,
    auto_install_updates: bool,
    notify_on_update: bool,

    public_key_pem: Option<String>,
}

impl Config {
    /// Parse from a JSON string.
    pub fn new(json_str: &str) -> Result<Self, ServiceError> {
        let cfg: Self = serde_json::from_str(json_str)?;
        Ok(cfg)
    }

    /// Read and parse configuration from the given file path.
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, ServiceError> {
        let path = path.as_ref();
        let s = fs::read_to_string(path)?;
        let cfg = Self::new(&s)?;
        Ok(cfg)
    }

    /// Load configuration from the provided path.
    pub fn load_from(path: std::path::PathBuf) -> Result<Self, ServiceError> {
        if !path.exists() {
            return Err(ServiceError::MissingConfigurationError(path));
        }
        Self::from_file(path)
    }
}
