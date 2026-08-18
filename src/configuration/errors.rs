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
    /// A configuration layer holds the path but cannot answer for it: the read
    /// failed, or something other than a regular file occupies it.
    ///
    /// Distinct from [`Io`](Self::Io) because consumers *match* on this
    /// condition to choose a policy — the configuration report renders it and
    /// continues, the relay watcher holds its last reconciliation — and a
    /// formatted context string is not something a policy can be keyed on.
    UnreadableConfigurationLayer {
        path: PathBuf,
        source: io::Error,
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

    pub(super) fn unreadable_layer(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::UnreadableConfigurationLayer {
            path: path.into(),
            source,
        }
    }

    /// The fault for a path that exists and is not a regular file.
    ///
    /// Synthesizes an [`io::Error`] rather than splitting the variant: the
    /// condition is the same one from a consumer's side — this layer holds the
    /// path and cannot supply the file — and the cause is what differs, which
    /// is exactly what the source carries.
    pub(super) fn layer_path_not_a_file(path: impl Into<PathBuf>) -> Self {
        Self::unreadable_layer(
            path,
            io::Error::other("path exists but is not a regular file"),
        )
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
            Self::UnreadableConfigurationLayer { path, source } => write!(
                formatter,
                "configuration layer cannot supply {}: {}",
                path.display(),
                source
            ),
            Self::Io { context, source } => write!(formatter, "{context}: {source}"),
        }
    }
}

impl Error for ConfigurationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } | Self::UnreadableConfigurationLayer { source, .. } => {
                Some(source)
            }
            _ => None,
        }
    }
}
