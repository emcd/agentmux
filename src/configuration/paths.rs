use std::{
    collections::BTreeMap,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use super::{
    ASSOCIATION_FILE, BUNDLE_EXTENSION, BUNDLES_DIRECTORY, CODERS_FILE, ConfigurationError,
    ConfigurationRoots, POLICIES_FILE, RELAY_FILE, UI_FILE, USERS_FILE,
};

/// What one layer answers for one candidate path.
enum LayerAnswer {
    Supplies,
    DoesNotSupply,
}

/// Classifies one candidate path, distinguishing a layer that does not supply
/// the file from a layer that cannot say.
///
/// Exactly one answer means the layer does not supply it: nothing exists at the
/// path. Every other answer is about a path that *is* occupied, and each of them
/// resolves to the same observable outcome if it is read as absence — a
/// lower-precedence layer's value silently takes effect, which is the
/// substitution this lookup exists to prevent. `NotADirectory` is included in
/// the fault side deliberately: layer-list validation proves each supplied layer
/// *root* is a directory and says nothing about intermediate components of a
/// relative path beneath it.
fn classify_candidate(candidate: &Path) -> Result<LayerAnswer, ConfigurationError> {
    match std::fs::metadata(candidate) {
        Ok(metadata) if metadata.is_file() => Ok(LayerAnswer::Supplies),
        Ok(_) => Err(ConfigurationError::layer_path_not_a_file(candidate)),
        // Also a dangling symlink, since `metadata` follows links exactly as the
        // `is_file` probe this replaced did.
        Err(source) if source.kind() == ErrorKind::NotFound => Ok(LayerAnswer::DoesNotSupply),
        Err(source) => Err(ConfigurationError::unreadable_layer(candidate, source)),
    }
}

/// The layer-supplied file for a path relative to the configuration roots: the
/// first layer holding it as a regular file, or `None` when no layer does.
///
/// This is the lookup proper. [`effective_configuration_path`] adds the
/// absent-file policy on top; source reporting needs the lookup without it,
/// because a path synthesized for a file no layer supplies would be reported as
/// though a layer supplied it.
///
/// `None` and `Err` are different answers on purpose. `None` means every layer
/// was asked and none holds the file, which each artifact's own absence
/// semantics then interpret. `Err` means a layer could not be asked, and the
/// search stops there rather than continuing into the layers it was shadowing.
pub fn supplied_configuration_path(
    roots: &ConfigurationRoots,
    relative: impl AsRef<Path>,
) -> Result<Option<PathBuf>, ConfigurationError> {
    let relative = relative.as_ref();
    for layer in roots.layers() {
        let candidate = layer.join(relative);
        match classify_candidate(&candidate)? {
            LayerAnswer::Supplies => return Ok(Some(candidate)),
            LayerAnswer::DoesNotSupply => {}
        }
    }
    Ok(None)
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
/// against a layer that exists to shadow one. That fallback answers genuine
/// absence only: a layer that could not be read faults instead, because
/// synthesizing a base-layer path there would report the file missing from a
/// location nobody looked at while the layer that holds it goes unmentioned.
///
/// "Genuine absence" is narrow, and deliberately so: nothing existing at the
/// candidate path, and nothing else. A permission error, an intermediate path
/// component that is not a directory, and a path occupied by something other
/// than a regular file are each a layer that could not answer rather than a
/// layer answering no. The returned path is therefore only meaningful together
/// with the `Ok`; on `Err` there is no path to report a file missing from,
/// which is the point.
pub fn effective_configuration_path(
    roots: &ConfigurationRoots,
    relative: impl AsRef<Path>,
) -> Result<PathBuf, ConfigurationError> {
    let relative = relative.as_ref();
    Ok(supplied_configuration_path(roots, relative)?
        .unwrap_or_else(|| roots.base_layer().join(relative)))
}

/// Resolves path to shared coder definitions.
pub fn coders_configuration_path(
    roots: &ConfigurationRoots,
) -> Result<PathBuf, ConfigurationError> {
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
pub fn bundle_configuration_path(
    roots: &ConfigurationRoots,
    bundle_name: &str,
) -> Result<PathBuf, ConfigurationError> {
    effective_configuration_path(
        roots,
        Path::new(BUNDLES_DIRECTORY).join(format!("{bundle_name}.{BUNDLE_EXTENSION}")),
    )
}

/// Effective bundle definitions keyed by bundle identifier, with an entry in an
/// earlier layer shadowing an entry of the same identifier in a later one.
///
/// Bundle directories union rather than replace: whole-directory replacement
/// would force a layer redefining one bundle to restate every other one. A layer
/// with no bundles directory contributes nothing, which is the ordinary case for
/// a layer overriding only root-level artifacts.
///
/// Enumeration reaches the filesystem three times per layer — opening the
/// directory, taking each iterator item, and typing each entry — and each is its
/// own chance to turn a failure into an empty result. Only an absent directory
/// is absence; everything else faults, so a layer never contributes a silently
/// short set that reads as the definitions it holds.
pub fn effective_bundle_definitions(
    roots: &ConfigurationRoots,
) -> Result<BTreeMap<String, PathBuf>, ConfigurationError> {
    let mut definitions = BTreeMap::new();
    // Reversed so the earliest layer is applied last and therefore wins.
    for directory in bundle_directory_layers(roots).into_iter().rev() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(source) if source.kind() == ErrorKind::NotFound => continue,
            Err(source) => return Err(ConfigurationError::unreadable_layer(directory, source)),
        };
        for entry in entries {
            // Not `flatten`: an item error truncates this layer's contribution
            // while enumeration still reports success, which is the same
            // substitution as an unreadable layer with less to show for it.
            let entry =
                entry.map_err(|source| ConfigurationError::unreadable_layer(&directory, source))?;
            let path = entry.path();
            // Extension first, so an ordinary subdirectory or stray file under
            // `bundles/` stays ignorable whatever its type, and only
            // bundle-shaped names are held to being regular files.
            if path.extension().and_then(|value| value.to_str()) != Some(BUNDLE_EXTENSION) {
                continue;
            }
            match std::fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => {}
                Ok(_) => return Err(ConfigurationError::layer_path_not_a_file(path)),
                // The entry was removed between enumeration and this probe. The
                // removal raises its own filesystem event, so the watcher
                // reconciles from that rather than from a fault here.
                Err(source) if source.kind() == ErrorKind::NotFound => continue,
                Err(source) => return Err(ConfigurationError::unreadable_layer(path, source)),
            }
            let Some(identifier) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            definitions.insert(identifier.to_string(), path);
        }
    }
    Ok(definitions)
}

/// Resolves path to global user configuration file (`users.toml`).
pub fn tui_configuration_path(roots: &ConfigurationRoots) -> Result<PathBuf, ConfigurationError> {
    effective_configuration_path(roots, USERS_FILE)
}

/// Resolves path to UI-surface configuration file (`ui.toml`).
pub fn ui_configuration_path(roots: &ConfigurationRoots) -> Result<PathBuf, ConfigurationError> {
    effective_configuration_path(roots, UI_FILE)
}

/// Resolves path to authorization policy presets file.
pub fn policies_configuration_path(
    roots: &ConfigurationRoots,
) -> Result<PathBuf, ConfigurationError> {
    effective_configuration_path(roots, POLICIES_FILE)
}

/// Resolves path to relay settings file (`relay.toml`).
pub fn relay_configuration_path(roots: &ConfigurationRoots) -> Result<PathBuf, ConfigurationError> {
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
pub fn supplied_root_configuration_sources(
    roots: &ConfigurationRoots,
) -> Result<Vec<(&'static str, PathBuf)>, ConfigurationError> {
    let mut sources = Vec::new();
    for name in ROOT_CONFIGURATION_ARTIFACTS {
        if let Some(path) = supplied_configuration_path(roots, name)? {
            sources.push((name, path));
        }
    }
    Ok(sources)
}
