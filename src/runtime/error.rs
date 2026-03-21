//! Error types for runtime bootstrap and path resolution.

use std::{
    error::Error,
    fmt::{Display, Formatter},
    io,
    path::PathBuf,
    time::Duration,
};

/// Runtime failure emitted by bootstrap and filesystem helpers.
#[derive(Debug)]
pub enum RuntimeError {
    HomeDirectoryUnavailable,
    InvalidArgument {
        argument: String,
        message: String,
    },
    Validation {
        code: String,
        message: String,
    },
    InvalidBundleName {
        bundle_name: String,
    },
    RelayAutostartDisabled {
        relay_socket: PathBuf,
    },
    RelayStartupTimeout {
        relay_socket: PathBuf,
        startup_timeout: Duration,
    },
    RelaySpawnFailure {
        command: PathBuf,
        source: io::Error,
    },
    SecurityForeignOwned {
        path: PathBuf,
        expected_uid: u32,
        actual_uid: u32,
    },
    Io {
        context: String,
        source: io::Error,
    },
}

impl RuntimeError {
    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    pub fn validation(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Validation {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl Display for RuntimeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HomeDirectoryUnavailable => {
                write!(formatter, "home directory is unavailable; set HOME")
            }
            Self::InvalidArgument { argument, message } => {
                write!(formatter, "invalid argument {argument}: {message}")
            }
            Self::Validation { code, message } => {
                write!(formatter, "{code}: {message}")
            }
            Self::InvalidBundleName { bundle_name } => {
                write!(formatter, "invalid bundle name '{bundle_name}'")
            }
            Self::RelayAutostartDisabled { relay_socket } => write!(
                formatter,
                "relay unavailable at {}; start agentmux host relay with matching --state-directory",
                relay_socket.display()
            ),
            Self::RelayStartupTimeout {
                relay_socket,
                startup_timeout,
            } => write!(
                formatter,
                "relay startup timed out after {}ms while waiting for {}",
                startup_timeout.as_millis(),
                relay_socket.display()
            ),
            Self::RelaySpawnFailure { command, .. } => write!(
                formatter,
                "failed to spawn relay command {}",
                command.display()
            ),
            Self::SecurityForeignOwned {
                path,
                expected_uid,
                actual_uid,
            } => write!(
                formatter,
                "runtime artifact {} is owned by uid {}, expected uid {}",
                path.display(),
                actual_uid,
                expected_uid
            ),
            Self::Io { context, source } => {
                write!(formatter, "{context}: {source}")
            }
        }
    }
}

impl Error for RuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } | Self::RelaySpawnFailure { source, .. } => Some(source),
            _ => None,
        }
    }
}
