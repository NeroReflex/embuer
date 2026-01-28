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

/// Current status of the update process
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// No update in progress
    Idle,
    /// Checking for updates
    Checking,
    /// Clearing old deployments
    Clearing,
    /// Installing update (with progress 0-100, or -1 if unknown)
    Installing { source: String, progress: i32 },
    /// Awaiting user confirmation to install
    AwaitingConfirmation { version: String, source: String },
    /// Update completed successfully
    Completed { source: String, deployment: String },
    /// Update failed
    Failed { source: String, error: String },
}

impl UpdateStatus {
    /// Convert status to a string representation for DBus
    pub fn as_str(&self) -> &str {
        match self {
            UpdateStatus::Idle => "Idle",
            UpdateStatus::Checking => "Checking",
            UpdateStatus::Clearing => "Clearing",
            UpdateStatus::Installing { .. } => "Installing",
            UpdateStatus::AwaitingConfirmation { .. } => "AwaitingConfirmation",
            UpdateStatus::Completed { .. } => "Completed",
            UpdateStatus::Failed { .. } => "Failed",
        }
    }

    /// Get additional details about the status
    pub fn details(&self) -> String {
        match self {
            UpdateStatus::Idle => String::new(),
            UpdateStatus::Checking => String::new(),
            UpdateStatus::Clearing => String::new(),
            UpdateStatus::Installing { source, .. } => source.clone(),
            UpdateStatus::AwaitingConfirmation { version, source } => {
                format!("{} ({})", version, source)
            }
            UpdateStatus::Completed { source, deployment } => {
                format!("{} installed as {}", source, deployment)
            }
            UpdateStatus::Failed { source, error } => format!("{}: {}", source, error),
        }
    }

    /// Get progress percentage (0-100, or -1 if not applicable/unknown)
    pub fn progress(&self) -> i32 {
        match self {
            UpdateStatus::Installing { progress, .. } => *progress,
            _ => -1,
        }
    }
}
