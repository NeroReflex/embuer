use crate::ServiceError;
use std::process::Command;
use tokio::io::AsyncRead;
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
    /// `input_stream` to its stdin. Returns an error if the process fails
    /// to spawn or exits with a non-zero status.
    pub async fn receive<R, P>(
        &self,
        path: P,
        mut input_stream: R,
    ) -> Result<(), ServiceError>
    where
        R: AsyncRead + Unpin + Send + 'static,
        P: AsRef<std::path::Path>,
    {
        use tokio::io::AsyncWriteExt;

        let mut btrfs_proc = TokioCommand::new("btrfs")
            .arg("receive")
            .arg(path.as_ref().as_os_str())
            .arg("-e")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(ServiceError::IOError)?;

        let mut btrfs_stdin = btrfs_proc.stdin.take().ok_or_else(|| {
            ServiceError::IOError(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Failed to open stdin for btrfs receive",
            ))
        })?;

        // Pipe input stream -> btrfs stdin
        let pipe_task = tokio::spawn(async move {
            let result = tokio::io::copy(&mut input_stream, &mut btrfs_stdin).await;
            if let Err(e) = result {
                eprintln!("Error piping data to btrfs receive: {}", e);
            }
            let _ = btrfs_stdin.shutdown().await;
        });

        // Wait for piping to complete
        let _ = pipe_task.await;

        // Wait for btrfs receive to finish
        let btrfs_status = btrfs_proc.wait().await?;
        if !btrfs_status.success() {
            return Err(ServiceError::IOError(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("btrfs receive failed with status: {}", btrfs_status),
            )));
        }

        Ok(())
    }
}
