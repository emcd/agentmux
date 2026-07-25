//! Starter configuration scaffolding helpers.

use std::{
    fs,
    io::{ErrorKind, Write},
    path::Path,
};

use crate::configuration::{
    BUNDLES_DIRECTORY, CODERS_FILE, POLICIES_FILE, RELAY_FILE, UI_FILE, USERS_FILE,
    effective_bundle_definitions,
};

use super::error::RuntimeError;
use super::paths::RuntimeRoots;

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
const USERS_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/data/configuration/users.toml"
));
const UI_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/data/configuration/ui.toml"
));
const RELAY_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/data/configuration/relay.toml"
));

/// Ensures starter configuration files exist without overwriting user config.
///
/// Hydration applies only to a root resolved from the default tier. A root the
/// operator named — by flag, environment, or discovery — is never scaffolded:
/// answering "you named a root that is not there" with a fresh empty deployment
/// makes the mistake look like success. Taking the resolved roots rather than a
/// bare path is what keeps that gate from being bypassed at a call site.
///
/// # Errors
///
/// Returns `RuntimeError` when directories or template files cannot be created,
/// or when a non-default root does not exist.
pub fn ensure_starter_configuration_layout(roots: &RuntimeRoots) -> Result<(), RuntimeError> {
    let configuration_root = roots.configuration_root.as_path();
    if !roots.configuration_root_source.permits_hydration() {
        if !configuration_root.is_dir() {
            return Err(RuntimeError::validation(
                "validation_configuration_root_absent",
                format!(
                    "configuration root does not exist: {}",
                    configuration_root.display()
                ),
            ));
        }
        return Ok(());
    }
    ensure_directory(configuration_root)?;
    let bundles_directory = configuration_root.join(BUNDLES_DIRECTORY);
    ensure_directory(&bundles_directory)?;
    ensure_template_file(&configuration_root.join(CODERS_FILE), CODERS_TEMPLATE)?;
    ensure_template_file(&configuration_root.join(POLICIES_FILE), POLICIES_TEMPLATE)?;
    ensure_template_file(&configuration_root.join(USERS_FILE), USERS_TEMPLATE)?;
    // ui.toml scaffolds as a fully-commented all-defaults file (like relay.toml):
    // it documents the UI-surface keys (`default-bundle`) without activating any,
    // and a missing/empty ui.toml simply means no configured UI-surface defaults.
    ensure_template_file(&configuration_root.join(UI_FILE), UI_TEMPLATE)?;
    // The relay.toml template is fully commented, so it scaffolds as an
    // all-defaults (effectively empty) file: it documents the schema without
    // activating any control, and the relay loads it as the documented defaults.
    ensure_template_file(&configuration_root.join(RELAY_FILE), RELAY_TEMPLATE)?;
    // The example bundle is a first-run sample, not durable config: it seeds
    // only when the operator has supplied no bundles of their own, so deleting
    // it after setup does not re-seed it on the next start. Bundles union
    // across layers, which makes an overlay definition operator configuration
    // exactly as a base one is — seeding beside it would add a live bundle to
    // the effective set that the operator never wrote.
    if effective_bundle_definitions(configuration_root).is_empty() {
        ensure_template_file(
            &bundles_directory.join(EXAMPLE_BUNDLE_FILE),
            BUNDLE_TEMPLATE,
        )?;
    }
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
