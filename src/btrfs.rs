use crate::ServiceError;
use std::process::Command;

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
}
