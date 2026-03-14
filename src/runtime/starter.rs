//! Starter configuration scaffolding helpers.

use std::{
    fs,
    io::{ErrorKind, Write},
    path::Path,
};

use super::error::RuntimeError;

const BUNDLES_DIRECTORY: &str = "bundles";
const CODERS_FILE: &str = "coders.toml";
const POLICIES_FILE: &str = "policies.toml";
const EXAMPLE_BUNDLE_FILE: &str = "example.toml";

const CODERS_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/data/configuration/coders.toml"
));

const BUNDLE_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/data/configuration/bundle.toml"
));
const POLICIES_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/data/configuration/policies.toml"
));

/// Ensures starter configuration files exist without overwriting user config.
///
/// # Errors
///
/// Returns `RuntimeError` when directories or template files cannot be created.
pub fn ensure_starter_configuration_layout(configuration_root: &Path) -> Result<(), RuntimeError> {
    ensure_directory(configuration_root)?;
    let bundles_directory = configuration_root.join(BUNDLES_DIRECTORY);
    ensure_directory(&bundles_directory)?;
    ensure_template_file(&configuration_root.join(CODERS_FILE), CODERS_TEMPLATE)?;
    ensure_template_file(&configuration_root.join(POLICIES_FILE), POLICIES_TEMPLATE)?;
    ensure_template_file(
        &bundles_directory.join(EXAMPLE_BUNDLE_FILE),
        BUNDLE_TEMPLATE,
    )?;
    Ok(())
}

fn ensure_directory(path: &Path) -> Result<(), RuntimeError> {
    fs::create_dir_all(path)
        .map_err(|source| RuntimeError::io(format!("create directory {}", path.display()), source))
}

fn ensure_template_file(path: &Path, contents: &str) -> Result<(), RuntimeError> {
    if path.exists() {
        return Ok(());
    }
    let mut file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(source) if source.kind() == ErrorKind::AlreadyExists => return Ok(()),
        Err(source) => {
            return Err(RuntimeError::io(
                format!("create starter configuration file {}", path.display()),
                source,
            ));
        }
    };
    file.write_all(contents.as_bytes())
        .map_err(|source| RuntimeError::io(format!("write {}", path.display()), source))?;
    file.write_all(b"\n")
        .map_err(|source| RuntimeError::io(format!("write {}", path.display()), source))?;
    file.flush()
        .map_err(|source| RuntimeError::io(format!("flush {}", path.display()), source))?;
    Ok(())
}
