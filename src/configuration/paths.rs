use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use super::{
    ASSOCIATION_FILE, BUNDLE_EXTENSION, BUNDLES_DIRECTORY, CODERS_FILE, ConfigurationRoots,
    POLICIES_FILE, RELAY_FILE, UI_FILE, USERS_FILE,
};

/// The layer-supplied file for a path relative to the configuration roots: the
/// first layer holding it as a regular file, or `None` when no layer does.
///
/// This is the lookup proper. [`effective_configuration_path`] adds the
/// absent-file policy on top; source reporting needs the lookup without it,
/// because a path synthesized for a file no layer supplies would be reported as
/// though a layer supplied it.
#[must_use]
pub fn supplied_configuration_path(
    roots: &ConfigurationRoots,
    relative: impl AsRef<Path>,
) -> Option<PathBuf> {
    let relative = relative.as_ref();
    roots
        .layers()
        .iter()
        .map(|layer| layer.join(relative))
        .find(|candidate| candidate.is_file())
}

/// Resolves the effective file for a path relative to the configuration roots:
/// the first layer holding it as a regular file.
///
/// Every configuration file resolves through this lookup, so override
/// reachability cannot vary per file. Each overridable file previously carried
/// its own bespoke lookup, which is how one override came to be honored in
/// release builds while its sibling in the same directory was silently inert.
///
/// Falls back to the base layer when no layer holds it, so a missing file is
/// reported against the location an operator would create it rather than
/// against a layer that exists to shadow one.
#[must_use]
pub fn effective_configuration_path(
    roots: &ConfigurationRoots,
    relative: impl AsRef<Path>,
) -> PathBuf {
    let relative = relative.as_ref();
    supplied_configuration_path(roots, relative)
        .unwrap_or_else(|| roots.base_layer().join(relative))
}

/// Resolves path to shared coder definitions.
#[must_use]
pub fn coders_configuration_path(roots: &ConfigurationRoots) -> PathBuf {
    effective_configuration_path(roots, CODERS_FILE)
}

/// Every physical bundle directory, highest precedence first.
///
/// The relay watcher observes all of them, since a change in any layer can
/// alter the effective set, and pre-flight reports all of them when no bundle
/// is discoverable — naming one layer would misreport where the search looked.
#[must_use]
pub fn bundle_directory_layers(roots: &ConfigurationRoots) -> Vec<PathBuf> {
    roots
        .layers()
        .iter()
        .map(|layer| layer.join(BUNDLES_DIRECTORY))
        .collect()
}

/// Resolves path to one bundle definition file, an earlier layer shadowing a
/// later one.
#[must_use]
pub fn bundle_configuration_path(roots: &ConfigurationRoots, bundle_name: &str) -> PathBuf {
    effective_configuration_path(
        roots,
        Path::new(BUNDLES_DIRECTORY).join(format!("{bundle_name}.{BUNDLE_EXTENSION}")),
    )
}

/// Effective bundle definitions keyed by bundle identifier, with an entry in an
/// earlier layer shadowing an entry of the same identifier in a later one.
///
/// Bundle directories union rather than replace: whole-directory replacement
/// would force a layer redefining one bundle to restate every other one. An
/// unreadable directory contributes nothing, which is how the common case of a
/// layer without a bundles directory resolves.
#[must_use]
pub fn effective_bundle_definitions(roots: &ConfigurationRoots) -> BTreeMap<String, PathBuf> {
    let mut definitions = BTreeMap::new();
    // Reversed so the earliest layer is applied last and therefore wins.
    for directory in bundle_directory_layers(roots).into_iter().rev() {
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
pub fn tui_configuration_path(roots: &ConfigurationRoots) -> PathBuf {
    effective_configuration_path(roots, USERS_FILE)
}

/// Resolves path to UI-surface configuration file (`ui.toml`).
#[must_use]
pub fn ui_configuration_path(roots: &ConfigurationRoots) -> PathBuf {
    effective_configuration_path(roots, UI_FILE)
}

/// Resolves path to authorization policy presets file.
#[must_use]
pub fn policies_configuration_path(roots: &ConfigurationRoots) -> PathBuf {
    effective_configuration_path(roots, POLICIES_FILE)
}

/// Resolves path to relay settings file (`relay.toml`).
#[must_use]
pub fn relay_configuration_path(roots: &ConfigurationRoots) -> PathBuf {
    effective_configuration_path(roots, RELAY_FILE)
}

/// Every root-level configuration artifact resolved through the layer list.
///
/// This is the inventory the source report enumerates, so an artifact missing
/// from it is one whose shadowing no surface exposes. `mcp.toml` earns its place
/// on exactly that ground: it selects the bundle and session an MCP server binds
/// to, so a shadowed copy silently redirects an association.
const ROOT_CONFIGURATION_ARTIFACTS: [&str; 6] = [
    ASSOCIATION_FILE,
    CODERS_FILE,
    POLICIES_FILE,
    RELAY_FILE,
    UI_FILE,
    USERS_FILE,
];

/// The root-level configuration artifacts some layer actually supplies, each
/// paired with the layer-relative name identifying it, in a stable order.
///
/// Only supplied artifacts appear. `mcp.toml`, `users.toml`, and `ui.toml` are
/// legitimately absent in ordinary deployments, so reporting a synthesized
/// base-layer path for every name would pad the report with files that do not
/// exist and bury the ones that do. Bundle definitions are enumerated by
/// [`effective_bundle_definitions`] instead, since they union a directory rather
/// than resolve a single name.
#[must_use]
pub fn supplied_root_configuration_sources(
    roots: &ConfigurationRoots,
) -> Vec<(&'static str, PathBuf)> {
    ROOT_CONFIGURATION_ARTIFACTS
        .into_iter()
        .filter_map(|name| supplied_configuration_path(roots, name).map(|path| (name, path)))
        .collect()
}
