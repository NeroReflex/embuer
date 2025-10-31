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
