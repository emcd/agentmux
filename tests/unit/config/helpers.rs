use std::fs;

use agentmux::configuration::ConfigurationRoots;
use tempfile::TempDir;

/// Writes a single-layer configuration root and returns it as a layer list.
///
/// Most loader tests are indifferent to layering, so they take the
/// single-layer form and the multi-layer cases are exercised where the
/// shadowing rule itself is under test.
pub(super) fn write_config(
    temporary: &TempDir,
    bundle_name: &str,
    coders_toml: &str,
    bundle_toml: &str,
) -> ConfigurationRoots {
    let root = temporary.path().join("config");
    let bundles = root.join("bundles");
    fs::create_dir_all(&bundles).expect("create directories");
    fs::write(root.join("coders.toml"), coders_toml).expect("write coders");
    fs::write(bundles.join(format!("{bundle_name}.toml")), bundle_toml).expect("write bundle");
    ConfigurationRoots::single(root)
}
