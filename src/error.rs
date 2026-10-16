use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("WireGuard executable not found. Looked at: {0}. Install WireGuard for Windows from https://www.wireguard.com/install/ or pass --wireguard-exe.")]
    WireGuardNotFound(PathBuf),

    #[error("config file not found: {0}")]
    ConfigNotFound(PathBuf),

    #[error("could not derive a tunnel name from config path {0}; it must have a .conf extension")]
    InvalidConfigName(PathBuf),

    #[error("failed to run '{command}': {source}")]
    Spawn {
        command: String,
        #[source]
        source: std::io::Error,
    },

    #[error("'{command}' exited with status {status}: {stderr}")]
    CommandFailed {
        command: String,
        status: i32,
        stderr: String,
    },

    #[error("could not read service status for tunnel '{0}'")]
    StatusUnknown(String),

    #[error("invalid relay config '{path}': {reason}")]
    RelayConfigInvalid { path: PathBuf, reason: String },

    #[error("process '{0}' is not running")]
    ProcessNotRunning(String),

    #[error("no healthy relay available among the configured candidates")]
    NoHealthyRelay,

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, AgentError>;
