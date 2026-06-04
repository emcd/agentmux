use std::path::{Path, PathBuf};

use super::{BUNDLE_EXTENSION, BUNDLES_DIRECTORY, CODERS_FILE, POLICIES_FILE, USERS_FILE};

/// Resolves path to shared coder definitions.
pub fn coders_configuration_path(configuration_root: &Path) -> PathBuf {
    configuration_root.join(CODERS_FILE)
}

/// Resolves the directory that holds per-bundle definition files. This is the
/// directory the relay bundle file watcher observes for runtime add/remove/
/// modify of bundle configurations.
pub fn bundles_configuration_directory(configuration_root: &Path) -> PathBuf {
    configuration_root.join(BUNDLES_DIRECTORY)
}

/// Resolves path to one bundle definition file.
pub fn bundle_configuration_path(configuration_root: &Path, bundle_name: &str) -> PathBuf {
    configuration_root
        .join(BUNDLES_DIRECTORY)
        .join(format!("{bundle_name}.{BUNDLE_EXTENSION}"))
}

/// Resolves path to global user configuration file (`users.toml`).
pub fn tui_configuration_path(configuration_root: &Path) -> PathBuf {
    configuration_root.join(USERS_FILE)
}

/// Resolves path to authorization policy presets file.
pub fn policies_configuration_path(configuration_root: &Path) -> PathBuf {
    configuration_root.join(POLICIES_FILE)
}
