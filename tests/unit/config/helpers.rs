use std::{fs, path::PathBuf};

use tempfile::TempDir;

pub(super) fn write_config(
    temporary: &TempDir,
    bundle_name: &str,
    coders_toml: &str,
    bundle_toml: &str,
) -> PathBuf {
    let root = temporary.path().join("config");
    let bundles = root.join("bundles");
    fs::create_dir_all(&bundles).expect("create directories");
    fs::write(root.join("coders.toml"), coders_toml).expect("write coders");
    fs::write(bundles.join(format!("{bundle_name}.toml")), bundle_toml).expect("write bundle");
    root
}
