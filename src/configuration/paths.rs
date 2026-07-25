use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use super::{
    BUNDLE_EXTENSION, BUNDLES_DIRECTORY, CODERS_FILE, OVERLAY_DIRECTORY, POLICIES_FILE, RELAY_FILE,
    UI_FILE, USERS_FILE,
};

/// Physical layers a configuration root expands to, nearest first.
///
/// Every configuration file resolves through these layers, so override
/// reachability cannot vary per file. Each overridable file previously carried
/// its own bespoke lookup, which is how one override came to be honored in
/// release builds while its sibling in the same directory was silently inert.
#[must_use]
pub fn configuration_layers(configuration_root: &Path) -> [PathBuf; 2] {
    [
        configuration_root.join(OVERLAY_DIRECTORY),
        configuration_root.to_path_buf(),
    ]
}

/// Resolves the effective file for a path relative to the configuration root:
/// the first layer holding it as a regular file.
///
/// Falls back to the base path when no layer holds it, so a missing file is
/// reported against the location an operator would create it rather than
/// against the overlay.
#[must_use]
pub fn effective_configuration_path(
    configuration_root: &Path,
    relative: impl AsRef<Path>,
) -> PathBuf {
    let relative = relative.as_ref();
    for layer in configuration_layers(configuration_root) {
        let candidate = layer.join(relative);
        if candidate.is_file() {
            return candidate;
        }
    }
    configuration_root.join(relative)
}

/// Resolves path to shared coder definitions.
#[must_use]
pub fn coders_configuration_path(configuration_root: &Path) -> PathBuf {
    effective_configuration_path(configuration_root, CODERS_FILE)
}

/// Resolves the base directory holding per-bundle definition files.
///
/// This is the write location and the location reported to an operator. Reads
/// go through [`effective_bundle_definitions`], which unions both layers.
#[must_use]
pub fn bundles_configuration_directory(configuration_root: &Path) -> PathBuf {
    configuration_root.join(BUNDLES_DIRECTORY)
}

/// Both physical bundle directories, overlay first.
///
/// The relay watcher observes both, since a change in either layer can alter
/// the effective set.
#[must_use]
pub fn bundle_directory_layers(configuration_root: &Path) -> [PathBuf; 2] {
    configuration_layers(configuration_root).map(|layer| layer.join(BUNDLES_DIRECTORY))
}

/// Resolves path to one bundle definition file, overlay shadowing base.
#[must_use]
pub fn bundle_configuration_path(configuration_root: &Path, bundle_name: &str) -> PathBuf {
    effective_configuration_path(
        configuration_root,
        Path::new(BUNDLES_DIRECTORY).join(format!("{bundle_name}.{BUNDLE_EXTENSION}")),
    )
}

/// Effective bundle definitions keyed by bundle identifier, with an overlay
/// entry shadowing a base entry of the same identifier.
///
/// Bundle directories union rather than replace: whole-directory replacement
/// would force an overlay redefining one bundle to restate every other one. An
/// unreadable directory contributes nothing, which is how the common case of an
/// absent overlay directory resolves.
#[must_use]
pub fn effective_bundle_definitions(configuration_root: &Path) -> BTreeMap<String, PathBuf> {
    let mut definitions = BTreeMap::new();
    // Reversed so the overlay layer is applied last and therefore wins.
    for directory in bundle_directory_layers(configuration_root)
        .into_iter()
        .rev()
    {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file()
                || path.extension().and_then(|value| value.to_str()) != Some(BUNDLE_EXTENSION)
            {
                continue;
            }
            let Some(identifier) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            definitions.insert(identifier.to_string(), path);
        }
    }
    definitions
}

/// Resolves path to global user configuration file (`users.toml`).
#[must_use]
pub fn tui_configuration_path(configuration_root: &Path) -> PathBuf {
    effective_configuration_path(configuration_root, USERS_FILE)
}

/// Resolves path to UI-surface configuration file (`ui.toml`).
#[must_use]
pub fn ui_configuration_path(configuration_root: &Path) -> PathBuf {
    effective_configuration_path(configuration_root, UI_FILE)
}

/// Resolves path to authorization policy presets file.
#[must_use]
pub fn policies_configuration_path(configuration_root: &Path) -> PathBuf {
    effective_configuration_path(configuration_root, POLICIES_FILE)
}

/// Resolves path to relay settings file (`relay.toml`).
#[must_use]
pub fn relay_configuration_path(configuration_root: &Path) -> PathBuf {
    effective_configuration_path(configuration_root, RELAY_FILE)
}
