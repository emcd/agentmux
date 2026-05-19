use std::{
    error::Error,
    fmt::{Display, Formatter},
    io,
    path::{Path, PathBuf},
};

/// Configuration load/validation failures.
#[derive(Debug)]
pub enum ConfigurationError {
    UnknownBundle {
        bundle_name: String,
        path: PathBuf,
    },
    AmbiguousSender {
        working_directory: PathBuf,
        matches: Vec<String>,
    },
    InvalidConfiguration {
        path: PathBuf,
        message: String,
    },
    InvalidGroupName {
        path: PathBuf,
        group_name: String,
    },
    ReservedGroupName {
        path: PathBuf,
        group_name: String,
    },
    Io {
        context: String,
        source: io::Error,
    },
}

impl ConfigurationError {
    pub(super) fn io(context: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    pub(super) fn invalid(path: &Path, message: impl Into<String>) -> Self {
        Self::InvalidConfiguration {
            path: path.to_path_buf(),
            message: message.into(),
        }
    }
}

impl Display for ConfigurationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownBundle { bundle_name, path } => write!(
                formatter,
                "bundle '{}' is not configured at {}",
                bundle_name,
                path.display()
            ),
            Self::AmbiguousSender {
                working_directory,
                matches,
            } => write!(
                formatter,
                "ambiguous sender for {} matched sessions: {}",
                working_directory.display(),
                matches.join(", ")
            ),
            Self::InvalidConfiguration { path, message } => {
                write!(
                    formatter,
                    "invalid bundle configuration {}: {}",
                    path.display(),
                    message
                )
            }
            Self::InvalidGroupName { path, group_name } => write!(
                formatter,
                "invalid group name '{}' in {}",
                group_name,
                path.display()
            ),
            Self::ReservedGroupName { path, group_name } => write!(
                formatter,
                "group name '{}' is reserved in {}",
                group_name,
                path.display()
            ),
            Self::Io { context, source } => write!(formatter, "{context}: {source}"),
        }
    }
}

impl Error for ConfigurationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
