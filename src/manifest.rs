use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

use crate::ServiceError;

#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
pub struct Manifest {
    version: String,

    readonly: bool,

    install_script: Option<String>,
    uninstall_script: Option<String>,
}

impl Manifest {
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

    pub fn is_readonly(&self) -> bool {
        self.readonly
    }

    pub fn install_script(&self) -> Option<&str> {
        self.install_script.as_deref()
    }

    pub fn uninstall_script(&self) -> Option<&str> {
        self.uninstall_script.as_deref()
    }
}
