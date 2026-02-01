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

use crate::ServiceError;
use log::{error, info};
use std::os::unix::fs::MetadataExt;
use std::process::Command;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command as TokioCommand;

/// Lightweight wrapper for invoking the `btrfs` command-line tool.
///
/// `Btrfs::new()` attempts to run `btrfs --version` and returns an error
/// if the executable is not available or returns a non-zero exit status.
pub struct Btrfs {
    version: String,
}

impl Btrfs {
    /// Try to construct a new `Btrfs` instance by probing the installed tool.
    ///
    /// Returns an IOError-wrapped `ServiceError` when the `btrfs` executable
    /// can't be executed (missing on PATH) or it returns a failing exit
    /// status when asked for its version.
    pub fn new() -> Result<Self, ServiceError> {
        let output = Command::new("btrfs").arg("--version").output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(ServiceError::BtrfsError(format!(
                "btrfs returned non-zero exit status: {}",
                stderr
            )));
        }

        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(Self { version })
    }

    /// Return the discovered btrfs version string.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Run `btrfs` with arbitrary arguments and return stdout on success.
    ///
    /// Errors are returned as `ServiceError::IOError` when process spawning
    /// or execution fails, or when the command exits non-zero.
    pub fn run_and_get_stdout<I, S>(&self, args: I) -> Result<String, ServiceError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let output = Command::new("btrfs").args(args).output()?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Err(ServiceError::BtrfsError(stderr))
        }
    }

    /// Example helper: list subvolumes under `path` using `btrfs subvolume list`.
    pub fn subvolume_list<P: AsRef<std::path::Path>>(
        &self,
        path: P,
    ) -> Result<String, ServiceError> {
        let p = path.as_ref().to_string_lossy().to_string();
        self.run_and_get_stdout(["subvolume", "list", &p])
    }

    /// Run `btrfs receive` asynchronously, reading from the provided stream.
    ///
    /// This method spawns `btrfs receive -e <path>` and pipes data from
    /// `input_stream` to its stdin. Returns the received subvolume name
    /// parsed from stderr (line like "At subvol subvolname"), or None if not found.
    /// Returns an error if the process fails to spawn or exits with a non-zero status.
    pub async fn receive<R, P>(
        &self,
        path: P,
        mut input_stream: R,
    ) -> Result<Option<String>, ServiceError>
    where
        R: AsyncRead + Unpin + Send + 'static,
        P: AsRef<std::path::Path>,
    {
        use tokio::io::AsyncWriteExt;

        let lossy_path = path.as_ref().as_os_str().to_string_lossy();
        let command = format!("btrfs receive {lossy_path} -e 1>&2");
        let mut btrfs_proc = TokioCommand::new("bash")
            .arg("-c")
            .arg(command)
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .inspect_err(|e| error!("Error in btrfs receive command spawn: {e}"))
            .map_err(ServiceError::IOError)?;

        let mut btrfs_stdin = btrfs_proc.stdin.take().ok_or_else(|| {
            ServiceError::IOError(std::io::Error::other(
                "Failed to open stdin for btrfs receive",
            ))
        })?;

        // btrfs receive outputs to stderr (via 1>&2 redirection in command)
        // We must read stderr concurrently to prevent the process from blocking
        let btrfs_stderr = btrfs_proc.stderr.take().ok_or_else(|| {
            ServiceError::IOError(std::io::Error::other(
                "Failed to open stderr for btrfs receive",
            ))
        })?;

        // Create the btrfs stderr reader (for parsing subvolume name)
        let btrfs_stderr_reader = BufReader::new(btrfs_stderr);

        // Pipe input stream -> btrfs stdin
        let pipe_task = tokio::spawn(async move {
            let Ok(copy_res) = tokio::io::copy(&mut input_stream, &mut btrfs_stdin)
                .await
                .inspect_err(|e| error!("Error piping data to btrfs receive: {e}"))
            else {
                return;
            };

            let Ok(_) = btrfs_stdin
                .shutdown()
                .await
                .inspect_err(|e| error!("Error closing btrfs receive: {e}"))
            else {
                return;
            };

            info!("Piping data from xz to to btrfs receive succeeded: {copy_res} bytes copied");
        });

        // Read stderr concurrently to capture the subvolume name and prevent blocking
        let stderr_task = tokio::spawn(async move {
            let mut subvol_name: Option<String> = None;
            let mut lines = btrfs_stderr_reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // Parse line like "At subvol subvolname"
                if let Some(name) = line.strip_prefix("At subvol ") {
                    subvol_name = Some(name.to_string());
                    break;
                }
            }
            subvol_name
        });

        // Wait for all tasks: pipe data, wait for process, and read stderr
        let (copy_task_res, btrfs_task_res, stderr_task_res) =
            tokio::join!(pipe_task, btrfs_proc.wait(), stderr_task);

        let Ok(_) = copy_task_res
            .as_ref()
            .inspect_err(|e| error!("btrfs copy stream join error: {e}"))
        else {
            return Err(ServiceError::IOError(std::io::Error::other(
                "joining to stream to btrfs receive failed".to_string(),
            )));
        };

        let Ok(btrfs_status) = btrfs_task_res
            .as_ref()
            .inspect_err(|e| error!("btrfs receive join error: {e}"))
        else {
            return Err(ServiceError::IOError(std::io::Error::other(
                "joining of btrfs receive failed".to_string(),
            )));
        };

        // Get the subvolume name from stderr (may be None if not found)
        let subvol_name = match stderr_task_res {
            Ok(name) => name,
            Err(e) => {
                error!("stderr read task join error: {e}");
                return Err(ServiceError::IOError(std::io::Error::other(
                    "reading stderr from btrfs receive failed".to_string(),
                )));
            }
        };

        if !btrfs_status.success() {
            return Err(ServiceError::IOError(std::io::Error::other(format!(
                "btrfs receive failed with status: {btrfs_status}",
            ))));
        }

        Ok(subvol_name)
    }

    /// Check if the given directory is a btrfs subvolume.
    ///
    /// This method verifies two conditions:
    /// 1. The directory is on a btrfs filesystem
    /// 2. The directory's inode number is 2 or 256 (characteristic of btrfs subvolumes)
    ///
    /// Returns `true` if both conditions are met, `false` otherwise.
    /// Returns an error if the directory doesn't exist or cannot be accessed.
    pub fn is_btrfs_subvolume<P: AsRef<std::path::Path>>(
        &self,
        path: P,
    ) -> Result<bool, ServiceError> {
        use nix::sys::statfs::statfs;

        const BTRFS_SUPER_MAGIC: i64 = 0x9123683e;

        let path_ref = path.as_ref();

        // Check if the filesystem type is btrfs
        let fs_stat = statfs(path_ref).map_err(|e| {
            let path_str = path.as_ref().as_os_str().to_str().unwrap_or("<error_path>");
            ServiceError::IOError(std::io::Error::other(format!(
                "Failed to get filesystem info for {path_str}: {e}"
            )))
        })?;

        if fs_stat.filesystem_type().0 != BTRFS_SUPER_MAGIC {
            return Ok(false);
        }

        // Get the inode number
        let metadata = std::fs::metadata(path_ref)?;
        let inode = metadata.ino();

        // Btrfs subvolumes have inode number 2 or 256
        Ok(inode == 2 || inode == 256)
    }

    /// Get the btrfs subvolume ID of the given subvolume path.
    ///
    /// This method first verifies that the path is a btrfs subvolume,
    /// then runs `btrfs subvolume show` to retrieve the subvolume ID.
    ///
    /// Returns the subvolume ID as a `u64` on success.
    /// Returns an error if the path is not a btrfs subvolume or if
    /// the ID cannot be retrieved or parsed.
    pub fn btrfs_subvol_get_id<P: AsRef<std::path::Path>>(
        &self,
        path: P,
    ) -> Result<u64, ServiceError> {
        let path_ref = path.as_ref();

        // First check if it's a btrfs subvolume
        if !self.is_btrfs_subvolume(path_ref)? {
            return Err(ServiceError::BtrfsError(format!(
                "{:?} is not a valid btrfs subvolume",
                path_ref
            )));
        }

        // Run btrfs subvolume show
        let output = self.run_and_get_stdout(["subvolume", "show", &path_ref.to_string_lossy()])?;

        // Parse the output to find "Subvolume ID:" line
        for line in output.lines() {
            let trimmed = line.trim_start();
            if let Some(id_part) = trimmed.strip_prefix("Subvolume ID:") {
                let subvol_id = id_part.trim().parse::<u64>().map_err(|e| {
                    ServiceError::BtrfsError(format!(
                        "Failed to parse subvolume ID from '{}': {}",
                        id_part.trim(),
                        e
                    ))
                })?;
                return Ok(subvol_id);
            }
        }

        Err(ServiceError::BtrfsError(format!(
            "Could not find 'Subvolume ID:' in output for {:?}",
            path_ref
        )))
    }

    /// Set a subvolume to read-write state.
    ///
    /// This method ensures the subvolume is in RW state by:
    /// 1. Verifying it's a btrfs subvolume
    /// 2. Checking the current read-only property
    /// 3. Setting ro=false if needed
    /// 4. Verifying the property was changed successfully
    ///
    /// Returns `Ok(())` on success or if already in RW state.
    /// Returns an error if the path is not a btrfs subvolume or if
    /// the property cannot be changed.
    pub fn subvolume_set_rw<P: AsRef<std::path::Path>>(&self, path: P) -> Result<(), ServiceError> {
        let path_ref = path.as_ref();

        // Check if it's a btrfs subvolume
        if !self.is_btrfs_subvolume(path_ref)? {
            return Err(ServiceError::BtrfsError(format!(
                "The given path {:?} is not a btrfs subvolume",
                path_ref
            )));
        }

        // Get current read-only property
        let property_state =
            self.run_and_get_stdout(["property", "get", "-fts", &path_ref.to_string_lossy()])?;

        // Check if currently read-only
        if property_state.contains("ro=true") {
            // Set to read-write
            self.run_and_get_stdout([
                "property",
                "set",
                "-fts",
                &path_ref.to_string_lossy(),
                "ro",
                "false",
            ])?;

            // Verify the change
            let property_state_after =
                self.run_and_get_stdout(["property", "get", "-fts", &path_ref.to_string_lossy()])?;

            if !property_state_after.contains("ro=false") {
                return Err(ServiceError::BtrfsError(format!(
                    "The subvolume {:?} is still read-only after attempting to set it read-write",
                    path_ref
                )));
            }
        }

        Ok(())
    }

    /// Set a subvolume to read-only state.
    ///
    /// This method ensures the subvolume is in RO state by:
    /// 1. Verifying it's a btrfs subvolume
    /// 2. Checking the current read-only property
    /// 3. Setting ro=true if needed
    /// 4. Verifying the property was changed successfully
    ///
    /// Returns `Ok(())` on success or if already in RO state.
    /// Returns an error if the path is not a btrfs subvolume or if
    /// the property cannot be changed.
    pub fn subvolume_set_ro<P: AsRef<std::path::Path>>(&self, path: P) -> Result<(), ServiceError> {
        let path_ref = path.as_ref();

        // Check if it's a btrfs subvolume
        if !self.is_btrfs_subvolume(path_ref)? {
            return Err(ServiceError::BtrfsError(format!(
                "The given path {:?} is not a btrfs subvolume",
                path_ref
            )));
        }

        // Get current read-only property
        let property_state =
            self.run_and_get_stdout(["property", "get", "-fts", &path_ref.to_string_lossy()])?;

        // Check if currently read-write
        if property_state.contains("ro=false") {
            // Set to read-only
            self.run_and_get_stdout([
                "property",
                "set",
                "-fts",
                &path_ref.to_string_lossy(),
                "ro",
                "true",
            ])?;

            // Verify the change
            let property_state_after =
                self.run_and_get_stdout(["property", "get", "-fts", &path_ref.to_string_lossy()])?;

            if !property_state_after.contains("ro=true") {
                return Err(ServiceError::BtrfsError(format!(
                    "The subvolume {:?} is still read-write after attempting to set it read-only",
                    path_ref
                )));
            }
        }

        Ok(())
    }

    /// Set the default subvolume for a btrfs filesystem.
    ///
    /// This method sets which subvolume will be mounted by default when
    /// the btrfs filesystem is mounted without specifying a subvolume.
    ///
    /// # Arguments
    ///
    /// * `subvol_id` - The subvolume ID to set as default
    /// * `rootfs` - Path to the btrfs filesystem root
    ///
    /// Returns `Ok(())` on success.
    /// Returns an error if the command fails.
    pub fn subvolume_set_default<P: AsRef<std::path::Path>>(
        &self,
        subvol_id: u64,
        rootfs: P,
    ) -> Result<(), ServiceError> {
        let rootfs_ref = rootfs.as_ref();

        self.run_and_get_stdout([
            "subvolume",
            "set-default",
            &subvol_id.to_string(),
            &rootfs_ref.to_string_lossy(),
        ])?;

        Ok(())
    }

    /// Get the default subvolume ID for a btrfs filesystem.
    ///
    /// This method returns the ID of the subvolume that will be mounted
    /// by default when the btrfs filesystem is mounted without specifying
    /// a subvolume.
    ///
    /// # Arguments
    ///
    /// * `rootfs` - Path to the btrfs filesystem root
    ///
    /// Returns the default subvolume ID as a `u64` on success.
    /// Returns an error if the command fails or the ID cannot be parsed.
    pub fn subvolume_get_default<P: AsRef<std::path::Path>>(
        &self,
        rootfs: P,
    ) -> Result<u64, ServiceError> {
        let rootfs_ref = rootfs.as_ref();

        let output =
            self.run_and_get_stdout(["subvolume", "get-default", &rootfs_ref.to_string_lossy()])?;

        // Parse output like "ID 256 gen 123 top level 5 path deployments/current"
        // or "ID 5 (FS_TREE)"
        for word in output.split_whitespace() {
            if let Ok(id) = word.parse::<u64>() {
                return Ok(id);
            }
        }

        Err(ServiceError::BtrfsError(format!(
            "Could not parse default subvolume ID from output: {}",
            output
        )))
    }

    /// Create a btrfs subvolume.
    ///
    /// This method creates the specified subvolume.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the subvolume to delete
    ///
    /// Returns `Ok(())` on success.
    /// Returns an error if the command fails.
    pub fn subvolume_create<P: AsRef<std::path::Path>>(
        &self,
        path: P,
    ) -> Result<String, ServiceError> {
        let path_ref = path.as_ref();

        self.run_and_get_stdout(["subvolume", "create", &path_ref.to_string_lossy()])
    }

    /// Delete a btrfs subvolume.
    ///
    /// This method deletes the specified subvolume. The subvolume must
    /// not be currently mounted and must not be the default subvolume.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the subvolume to delete
    ///
    /// Returns `Ok(())` on success.
    /// Returns an error if the command fails.
    pub fn subvolume_delete<P: AsRef<std::path::Path>>(
        &self,
        path: P,
    ) -> Result<String, ServiceError> {
        let path_ref = path.as_ref();
        self.run_and_get_stdout(["subvolume", "delete", &path_ref.to_string_lossy()])
    }

    /// List all deployment subvolumes in the deployments directory.
    ///
    /// This method reads the deployments directory and returns a list
    /// of all entries that are btrfs subvolumes.
    ///
    /// # Arguments
    ///
    /// * `deployments_dir` - Path to the deployments directory
    ///
    /// Returns a vector of tuples containing (subvolume_name, subvolume_id, full_path)
    /// Returns an error if the directory cannot be read.
    pub fn list_deployment_subvolumes<P: AsRef<std::path::Path>>(
        &self,
        deployments_dir: P,
    ) -> Result<Vec<(String, u64, std::path::PathBuf)>, ServiceError> {
        let deployments_ref = deployments_dir.as_ref();

        let mut result = Vec::new();

        let entries = std::fs::read_dir(deployments_ref)?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            // Check if it's a directory and a btrfs subvolume
            if path.is_dir() && self.is_btrfs_subvolume(&path)? {
                let name = entry.file_name().to_string_lossy().to_string();
                let id = self.btrfs_subvol_get_id(&path)?;
                result.push((name, id, path));
            }
        }

        Ok(result)
    }
}
