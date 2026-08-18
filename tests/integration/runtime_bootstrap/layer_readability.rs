//! Layer lookup and bundle enumeration distinguishing absence from failure.
//!
//! The defect these cover is silent by construction. A higher-precedence layer
//! exists to shadow a lower one, so losing it produces the lower layer's value —
//! which is what a correctly resolved single-root deployment looks like. Every
//! assertion here is therefore paired: the fault is reported, *and* the value
//! from the layer beneath is not the answer.

use std::{fs, path::Path};

use agentmux::configuration::{
    ConfigurationRoots, effective_bundle_definitions, effective_configuration_path,
    supplied_configuration_path,
};
use tempfile::TempDir;

use crate::support::permissions::{deny_directory_access, report_permission_fixture_skip};

const RELAY_BODY: &str = "watch-bundles = true\n";

/// Creates a layer directory beneath `temporary`.
fn layer(temporary: &TempDir, name: &str) -> std::path::PathBuf {
    let directory = temporary.path().join(name);
    fs::create_dir_all(&directory).expect("create layer directory");
    directory
}

/// The two-layer list, override first.
fn layered(override_layer: &Path, base: &Path) -> ConfigurationRoots {
    ConfigurationRoots::from_elements([override_layer.to_path_buf(), base.to_path_buf()])
        .expect("layer list")
}

fn write_bundle_file(layer: &Path, bundle_name: &str) {
    let bundles = layer.join("bundles");
    fs::create_dir_all(&bundles).expect("create bundles directory");
    fs::write(
        bundles.join(format!("{bundle_name}.toml")),
        "format-version = 1\n",
    )
    .expect("write bundle");
}

#[test]
fn an_absent_file_is_absence_rather_than_a_fault() {
    // The control for every fault case below. If this stopped holding, the
    // others would all still pass while optional artifacts had become
    // unloadable everywhere.
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");

    let supplied = supplied_configuration_path(&layered(&override_layer, &base), "relay.toml")
        .expect("an absent file is not a fault");
    assert_eq!(supplied, None, "no layer supplies the file");
}

#[test]
fn a_readable_file_resolves_from_the_earlier_layer() {
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");
    fs::write(base.join("relay.toml"), RELAY_BODY).expect("write base relay.toml");
    fs::write(override_layer.join("relay.toml"), RELAY_BODY).expect("write override relay.toml");

    let supplied = supplied_configuration_path(&layered(&override_layer, &base), "relay.toml")
        .expect("lookup")
        .expect("a layer supplies the file");
    assert_eq!(supplied, override_layer.join("relay.toml"));
}

#[test]
fn an_unreadable_earlier_layer_faults_rather_than_falling_through() {
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");
    // Present in both, so falling through resolves successfully — the failure
    // mode is a clean answer from the wrong layer, not an error.
    fs::write(base.join("relay.toml"), RELAY_BODY).expect("write base relay.toml");
    fs::write(override_layer.join("relay.toml"), RELAY_BODY).expect("write override relay.toml");

    let Some(_restore) = deny_directory_access(&override_layer) else {
        report_permission_fixture_skip(
            "an_unreadable_earlier_layer_faults_rather_than_falling_through",
        );
        return;
    };

    let error = supplied_configuration_path(&layered(&override_layer, &base), "relay.toml")
        .expect_err("an unreadable earlier layer must fault");
    let rendered = error.to_string();
    assert!(
        rendered.contains(&override_layer.display().to_string()),
        "the fault must name the layer at fault, got: {rendered}"
    );
    assert!(
        !rendered.contains(&base.join("relay.toml").display().to_string()),
        "the fault must not report the layer it was shadowing, got: {rendered}"
    );
}

#[test]
fn an_unreadable_layer_faults_for_an_optional_artifact_absent_below() {
    // The case the optional-artifact semantics make dangerous: `ui.toml` is
    // legitimately absent everywhere, so an unreadable layer that gets read as
    // absence produces exactly the answer a correct empty deployment produces.
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");
    fs::write(
        override_layer.join("ui.toml"),
        "default-bundle = \"alpha\"\n",
    )
    .expect("write override ui.toml");

    let Some(_restore) = deny_directory_access(&override_layer) else {
        report_permission_fixture_skip(
            "an_unreadable_layer_faults_for_an_optional_artifact_absent_below",
        );
        return;
    };

    supplied_configuration_path(&layered(&override_layer, &base), "ui.toml")
        .expect_err("an unreadable layer must fault rather than report the artifact absent");
}

#[test]
fn a_directory_occupying_an_artifact_path_faults() {
    // No permission fixture, so this runs everywhere the suite does, including
    // as root. Deterministic and reproducible, and still invisible in practice:
    // nothing prompts an operator to look at a layer that resolves successfully
    // from underneath.
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");
    fs::write(base.join("relay.toml"), RELAY_BODY).expect("write base relay.toml");
    fs::create_dir_all(override_layer.join("relay.toml")).expect("occupy the artifact path");

    let error = supplied_configuration_path(&layered(&override_layer, &base), "relay.toml")
        .expect_err("a non-file at the artifact path must fault");
    let rendered = error.to_string();
    assert!(
        rendered.contains(&override_layer.join("relay.toml").display().to_string()),
        "the fault must name the occupied path, got: {rendered}"
    );
}

#[test]
fn a_non_directory_layer_component_faults() {
    // `bundles` is a regular file in the earlier layer, so the relative path
    // beneath it cannot resolve. Layer-list validation does not cover this: it
    // proves each supplied layer *root* is a directory and says nothing about
    // what is underneath.
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");
    write_bundle_file(&base, "alpha");
    fs::write(override_layer.join("bundles"), "not a directory\n").expect("occupy bundles");

    effective_configuration_path(&layered(&override_layer, &base), "bundles/alpha.toml")
        .expect_err("a non-directory path component must fault");
}

#[test]
fn a_layer_without_a_bundles_directory_contributes_nothing() {
    // The ordinary case the enumeration fault must not swallow: a layer
    // overriding only root-level artifacts.
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");
    write_bundle_file(&base, "alpha");

    let definitions = effective_bundle_definitions(&layered(&override_layer, &base))
        .expect("a layer with no bundles directory is not a fault");
    let identifiers: Vec<&str> = definitions.keys().map(String::as_str).collect();
    assert_eq!(identifiers, ["alpha"]);
}

#[test]
fn an_unreadable_bundles_directory_faults_rather_than_enumerating_empty() {
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");
    write_bundle_file(&base, "alpha");
    write_bundle_file(&override_layer, "beta");

    let override_bundles = override_layer.join("bundles");
    let Some(_restore) = deny_directory_access(&override_bundles) else {
        report_permission_fixture_skip(
            "an_unreadable_bundles_directory_faults_rather_than_enumerating_empty",
        );
        return;
    };

    let error = effective_bundle_definitions(&layered(&override_layer, &base))
        .expect_err("an unreadable bundles directory must fault");
    assert!(
        error
            .to_string()
            .contains(&override_bundles.display().to_string()),
        "the fault must name the directory at fault, got: {error}"
    );
}

#[test]
fn a_bundle_shaped_directory_entry_faults() {
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");
    write_bundle_file(&base, "alpha");
    fs::create_dir_all(override_layer.join("bundles").join("alpha.toml"))
        .expect("occupy the bundle path");

    // Falling through would resolve `alpha` from the base layer, which is the
    // definition the earlier layer was placed there to replace.
    effective_bundle_definitions(&layered(&override_layer, &base))
        .expect_err("a directory named like a bundle must fault");
}

#[test]
fn an_unrelated_entry_under_a_bundles_directory_is_ignored() {
    // The companion to the test above: only bundle-shaped names are held to
    // being regular files, so an ordinary subdirectory stays ordinary.
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");
    write_bundle_file(&base, "alpha");
    let bundles = override_layer.join("bundles");
    fs::create_dir_all(bundles.join("archive")).expect("create an unrelated subdirectory");
    fs::write(bundles.join("notes.md"), "not a bundle\n").expect("write an unrelated file");

    let definitions = effective_bundle_definitions(&layered(&override_layer, &base))
        .expect("unrelated entries must not fault");
    let identifiers: Vec<&str> = definitions.keys().map(String::as_str).collect();
    assert_eq!(identifiers, ["alpha"]);
}
