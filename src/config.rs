use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

use crate::ServiceError;

#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
pub struct Config {
    update_url: Option<String>,
    auto_install_updates: bool,

    public_key_pem: Option<String>,

    // Directory where deployments are stored on the filesystem. Optional.
    rootfs_dir: Option<String>,
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

    /// Return the configured path to the public key PEM file, if any.
    pub fn public_key_pem_path(&self) -> Option<&str> {
        self.public_key_pem.as_deref()
    }

    /// Return the configured deployments directory as an owned `PathBuf`, if present and valid.
    ///
    /// This returns `Some(PathBuf)` only when the configuration contains a
    /// `deployments_dir` and the path exists and is a directory. If the
    /// field is not set or the path does not exist / is not a directory,
    /// this returns `None`.
    pub fn rootfs_dir(&self) -> Result<std::path::PathBuf, ServiceError> {
        let s = self
            .rootfs_dir
            .as_ref()
            .map_or_else(|| Err(ServiceError::MissingRootfsDir), Ok)?;

        let p = std::path::PathBuf::from(s);

        if p.exists() && p.is_dir() {
            Ok(p)
        } else {
            Err(ServiceError::MissingRootfsDir)
        }
    }

    /// Return the configured deployments directory which is the `deployments`
    /// subdirectory under the configured rootfs directory, if present and valid.
    pub fn deployments_dir(&self) -> Result<std::path::PathBuf, ServiceError> {
        self.rootfs_dir()
            .map(|p| p.join("deployments"))
            .and_then(|p| {
                if p.exists() && p.is_dir() {
                    Ok(p)
                } else {
                    Err(ServiceError::MissingDeploymentsDir)
                }
            })
    }

    /// Accessors for config fields for external use/tests.
    pub fn update_url(&self) -> Option<&str> {
        self.update_url.as_deref()
    }

    pub fn auto_install_updates(&self) -> bool {
        self.auto_install_updates
    }
}
