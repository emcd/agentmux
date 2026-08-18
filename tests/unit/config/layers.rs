//! Configuration layer resolution over an operator-declared list of roots.
//!
//! One lookup serves every configuration file, so override reachability cannot
//! vary per file. Each overridable file previously carried its own bespoke
//! lookup, which is how one override came to be honored in release builds while
//! its sibling in the same directory was silently inert.

use std::{
    fs,
    path::{Path, PathBuf},
};

use tempfile::TempDir;

use agentmux::configuration::{
    ConfigurationRoots, effective_bundle_definitions, effective_configuration_path,
    load_bundle_configuration, load_bundle_group_memberships, load_ui_configuration,
};
use agentmux::relay::load_relay_runtime_configuration;

const ACP_CODER: &str = r#"
format-version = 1

[[coders]]
id = "acp"

[coders.acp]
channel = "stdio"
command = "opencode acp"
"#;

fn bundle_body(temporary: &TempDir, session_id: &str) -> String {
    let dir = temporary.path().display().to_string();
    format!(
        r#"
format-version = 1

[[sessions]]
id = "{session_id}"
directory = "{dir}"
coder = "acp"
"#
    )
}

/// Creates one layer directory beneath `temporary`.
fn layer(temporary: &TempDir, name: &str) -> PathBuf {
    let directory = temporary.path().join(name);
    fs::create_dir_all(&directory).expect("create layer directory");
    directory
}

/// Writes a bundle definition, and the coder it references, into `layer`.
fn write_bundle(layer: &Path, bundle_name: &str, body: &str) {
    let bundles = layer.join("bundles");
    fs::create_dir_all(&bundles).expect("create bundles directory");
    fs::write(layer.join("coders.toml"), ACP_CODER).expect("write coders");
    fs::write(bundles.join(format!("{bundle_name}.toml")), body).expect("write bundle");
}

/// The two-layer list every shadowing test uses, override first.
fn layered(override_layer: &Path, base: &Path) -> ConfigurationRoots {
    ConfigurationRoots::from_elements([override_layer.to_path_buf(), base.to_path_buf()])
        .expect("layer list")
}

#[test]
fn an_earlier_layer_shadows_a_later_one() {
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");
    write_bundle(&base, "alpha", &bundle_body(&temporary, "base"));
    write_bundle(
        &override_layer,
        "alpha",
        &bundle_body(&temporary, "overridden"),
    );

    let loaded =
        load_bundle_configuration(&layered(&override_layer, &base), "alpha").expect("load bundle");
    assert_eq!(loaded.members[0].id, "overridden");
}

#[test]
fn a_file_only_a_later_layer_holds_is_reached() {
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");
    write_bundle(&base, "alpha", &bundle_body(&temporary, "base"));
    fs::create_dir_all(override_layer.join("bundles")).expect("create empty override bundles");

    let loaded =
        load_bundle_configuration(&layered(&override_layer, &base), "alpha").expect("load bundle");
    assert_eq!(loaded.members[0].id, "base");
}

#[test]
fn the_first_layer_wins_across_three_layers() {
    // The ordering rule is a list property, not a property of the pair the
    // previous fixed arrangement could express.
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let middle = layer(&temporary, "middle");
    let first = layer(&temporary, "first");
    write_bundle(&base, "alpha", &bundle_body(&temporary, "base"));
    write_bundle(&middle, "alpha", &bundle_body(&temporary, "middle"));
    write_bundle(&first, "alpha", &bundle_body(&temporary, "first"));

    let roots = ConfigurationRoots::from_elements([first, middle, base]).expect("layer list");
    let loaded = load_bundle_configuration(&roots, "alpha").expect("load bundle");
    assert_eq!(loaded.members[0].id, "first");
}

#[test]
fn a_malformed_file_does_not_fall_through_to_a_later_layer() {
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");
    write_bundle(&base, "alpha", &bundle_body(&temporary, "base"));
    write_bundle(
        &override_layer,
        "alpha",
        "format-version = 1\nnot-a-key = [\n",
    );

    // Falling through would silently apply configuration the operator believes
    // they had overridden.
    load_bundle_configuration(&layered(&override_layer, &base), "alpha")
        .expect_err("a malformed file must fault rather than fall through");
}

#[test]
fn bundle_directories_union_by_identifier() {
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");
    write_bundle(&base, "alpha", &bundle_body(&temporary, "base"));
    write_bundle(&base, "beta", &bundle_body(&temporary, "base"));
    write_bundle(
        &override_layer,
        "beta",
        &bundle_body(&temporary, "overridden"),
    );

    // Whole-directory replacement would force a layer redefining one bundle to
    // restate every other one.
    let definitions =
        effective_bundle_definitions(&layered(&override_layer, &base)).expect("enumeration");
    let identifiers: Vec<&str> = definitions.keys().map(String::as_str).collect();
    assert_eq!(identifiers, ["alpha", "beta"]);
    assert_eq!(definitions["alpha"], base.join("bundles/alpha.toml"));
    assert_eq!(
        definitions["beta"],
        override_layer.join("bundles/beta.toml")
    );
}

#[test]
fn enumeration_includes_a_bundle_only_one_layer_defines() {
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");
    write_bundle(&base, "alpha", &bundle_body(&temporary, "base"));
    write_bundle(
        &override_layer,
        "beta",
        &bundle_body(&temporary, "overridden"),
    );

    // Enumeration feeds relay startup/autostart, group operations, TUI choices,
    // and every all-bundle listing. Reading only one directory would leave a
    // bundle loadable by name but invisible to all of them.
    let names: Vec<String> = load_bundle_group_memberships(&layered(&override_layer, &base))
        .expect("load memberships")
        .into_iter()
        .map(|membership| membership.bundle_name)
        .collect();
    assert_eq!(names, ["alpha", "beta"]);
}

#[test]
fn enumeration_reports_a_shadowed_bundle_once() {
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");
    write_bundle(&base, "alpha", &bundle_body(&temporary, "base"));
    let dir = temporary.path().display().to_string();
    write_bundle(
        &override_layer,
        "alpha",
        &format!(
            r#"
format-version = 1
groups = ["overridden"]

[[sessions]]
id = "s"
directory = "{dir}"
coder = "acp"
"#
        ),
    );

    let memberships =
        load_bundle_group_memberships(&layered(&override_layer, &base)).expect("load memberships");
    assert_eq!(memberships.len(), 1);
    assert_eq!(memberships[0].bundle_name, "alpha");
    assert_eq!(memberships[0].groups, ["overridden"]);
}

#[test]
fn relay_settings_resolve_through_the_layer_list() {
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");
    fs::write(base.join("relay.toml"), "watch-bundles = true\n").expect("write base relay.toml");
    fs::write(override_layer.join("relay.toml"), "watch-bundles = false\n")
        .expect("write override relay.toml");

    let loaded = load_relay_runtime_configuration(&layered(&override_layer, &base), None, None)
        .expect("load relay settings");
    assert!(
        !loaded.watch_bundles,
        "relay.toml must resolve through the same lookup as every other file"
    );
}

#[test]
fn a_malformed_relay_file_does_not_fall_through_to_a_later_layer() {
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");
    fs::write(base.join("relay.toml"), "watch-bundles = true\n").expect("write base relay.toml");
    fs::write(override_layer.join("relay.toml"), "watch-bundles = [\n")
        .expect("write malformed override relay.toml");

    load_relay_runtime_configuration(&layered(&override_layer, &base), None, None)
        .expect_err("a malformed file must fault rather than fall through");
}

#[test]
fn a_missing_file_reports_against_the_base_layer() {
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");

    // The base is the shared layer an override layer overrides, so it is the
    // creation site a reader would infer; naming the override layer would
    // suggest the file must be created in the layer that exists to shadow one.
    assert_eq!(
        effective_configuration_path(&layered(&override_layer, &base), "ui.toml").expect("lookup"),
        base.join("ui.toml")
    );
}

#[test]
fn every_file_kind_resolves_through_the_same_lookup() {
    let temporary = TempDir::new().expect("temporary");
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");
    fs::write(base.join("ui.toml"), "default-bundle = \"base-bundle\"\n")
        .expect("write base ui.toml");
    fs::write(
        override_layer.join("ui.toml"),
        "default-bundle = \"override-bundle\"\n",
    )
    .expect("write override ui.toml");

    let loaded = load_ui_configuration(&layered(&override_layer, &base))
        .expect("load ui configuration")
        .expect("expected ui configuration");
    assert_eq!(loaded.default_bundle.as_deref(), Some("override-bundle"));
}

#[test]
fn a_relative_path_resolves_identically_whichever_layer_supplies_it() {
    // No layer is a resolution base. A relative member directory declared in an
    // override layer resolves exactly as the identical declaration in the base
    // layer would.
    let temporary = TempDir::new().expect("temporary");
    let body = r#"
format-version = 1

[[sessions]]
id = "relative"
directory = "relative/workspace"
coder = "acp"
"#;
    let base = layer(&temporary, "base");
    let override_layer = layer(&temporary, "rnd");
    write_bundle(&base, "alpha", body);
    write_bundle(&override_layer, "beta", body);

    let roots = layered(&override_layer, &base);
    let from_base = load_bundle_configuration(&roots, "alpha").expect("load base bundle");
    let from_override = load_bundle_configuration(&roots, "beta").expect("load override bundle");
    assert_eq!(
        from_base.members[0].working_directory, from_override.members[0].working_directory,
        "a layer must not become a resolution base for the paths it supplies"
    );
}
