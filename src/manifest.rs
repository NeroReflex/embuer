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
