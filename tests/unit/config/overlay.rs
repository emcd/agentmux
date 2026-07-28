//! Configuration overlay resolution: `[root/overlay, root]`.
//!
//! One lookup serves every configuration file, so override reachability cannot
//! vary per file. Each overridable file previously carried its own bespoke
//! lookup, which is how one override came to be honored in release builds while
//! its sibling in the same directory was silently inert.

use std::fs;

use tempfile::TempDir;

use agentmux::configuration::{
    effective_bundle_definitions, effective_configuration_path, load_bundle_configuration,
    load_bundle_group_memberships, load_ui_configuration,
};
use agentmux::relay::load_relay_runtime_configuration;

use super::helpers::*;

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

#[test]
fn overlay_file_shadows_the_base_file() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        ACP_CODER,
        &bundle_body(&temporary, "base"),
    );
    let overlay_bundles = root.join("overlay/bundles");
    fs::create_dir_all(&overlay_bundles).expect("create overlay bundles");
    fs::write(
        overlay_bundles.join("alpha.toml"),
        bundle_body(&temporary, "overlaid"),
    )
    .expect("write overlay bundle");

    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    assert_eq!(loaded.members[0].id, "overlaid");
}

#[test]
fn falls_through_to_base_when_overlay_lacks_the_file() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        ACP_CODER,
        &bundle_body(&temporary, "base"),
    );
    fs::create_dir_all(root.join("overlay/bundles")).expect("create empty overlay");

    let loaded = load_bundle_configuration(&root, "alpha").expect("load configuration");
    assert_eq!(loaded.members[0].id, "base");
}

#[test]
fn malformed_overlay_file_does_not_fall_through_to_base() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        ACP_CODER,
        &bundle_body(&temporary, "base"),
    );
    let overlay_bundles = root.join("overlay/bundles");
    fs::create_dir_all(&overlay_bundles).expect("create overlay bundles");
    fs::write(
        overlay_bundles.join("alpha.toml"),
        "format-version = 1\nnot-a-key = [\n",
    )
    .expect("write malformed overlay bundle");

    // Falling through would silently apply configuration the operator believes
    // they had overridden.
    load_bundle_configuration(&root, "alpha")
        .expect_err("a malformed overlay file must fault rather than fall through");
}

#[test]
fn bundle_directories_union_by_identifier() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        ACP_CODER,
        &bundle_body(&temporary, "base"),
    );
    write_config(
        &temporary,
        "beta",
        ACP_CODER,
        &bundle_body(&temporary, "base"),
    );
    let overlay_bundles = root.join("overlay/bundles");
    fs::create_dir_all(&overlay_bundles).expect("create overlay bundles");
    fs::write(
        overlay_bundles.join("beta.toml"),
        bundle_body(&temporary, "overlaid"),
    )
    .expect("write overlay bundle");

    // Whole-directory replacement would force an overlay redefining one bundle
    // to restate every other one.
    let definitions = effective_bundle_definitions(&root);
    let identifiers: Vec<&str> = definitions.keys().map(String::as_str).collect();
    assert_eq!(identifiers, ["alpha", "beta"]);
    assert_eq!(definitions["alpha"], root.join("bundles/alpha.toml"));
    assert_eq!(definitions["beta"], overlay_bundles.join("beta.toml"));
}

#[test]
fn enumeration_includes_an_overlay_only_bundle() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        ACP_CODER,
        &bundle_body(&temporary, "base"),
    );
    let overlay_bundles = root.join("overlay/bundles");
    fs::create_dir_all(&overlay_bundles).expect("create overlay bundles");
    fs::write(
        overlay_bundles.join("beta.toml"),
        bundle_body(&temporary, "overlaid"),
    )
    .expect("write overlay bundle");

    // Enumeration feeds relay startup/autostart, group operations, TUI choices,
    // and every all-bundle listing. Reading only the base directory would leave
    // an overlay-only bundle loadable by name but invisible to all of them.
    let names: Vec<String> = load_bundle_group_memberships(&root)
        .expect("load memberships")
        .into_iter()
        .map(|membership| membership.bundle_name)
        .collect();
    assert_eq!(names, ["alpha", "beta"]);
}

#[test]
fn enumeration_reports_a_shadowed_bundle_once() {
    let temporary = TempDir::new().expect("temporary");
    let root = write_config(
        &temporary,
        "alpha",
        ACP_CODER,
        &bundle_body(&temporary, "base"),
    );
    let overlay_bundles = root.join("overlay/bundles");
    fs::create_dir_all(&overlay_bundles).expect("create overlay bundles");
    let dir = temporary.path().display().to_string();
    fs::write(
        overlay_bundles.join("alpha.toml"),
        format!(
            r#"
format-version = 1
groups = ["overlaid"]

[[sessions]]
id = "s"
directory = "{dir}"
coder = "acp"
"#
        ),
    )
    .expect("write overlay bundle");

    let memberships = load_bundle_group_memberships(&root).expect("load memberships");
    assert_eq!(memberships.len(), 1);
    assert_eq!(memberships[0].bundle_name, "alpha");
    assert_eq!(memberships[0].groups, ["overlaid"]);
}

#[test]
fn relay_settings_resolve_through_the_overlay() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path();
    let overlay = root.join("overlay");
    fs::create_dir_all(&overlay).expect("create overlay");
    fs::write(root.join("relay.toml"), "watch-bundles = true\n").expect("write base relay.toml");
    fs::write(overlay.join("relay.toml"), "watch-bundles = false\n")
        .expect("write overlay relay.toml");

    let loaded = load_relay_runtime_configuration(root, None, None).expect("load relay settings");
    assert!(
        !loaded.watch_bundles,
        "relay.toml must resolve through the same lookup as every other file"
    );
}

#[test]
fn malformed_overlay_relay_file_does_not_fall_through_to_base() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path();
    let overlay = root.join("overlay");
    fs::create_dir_all(&overlay).expect("create overlay");
    fs::write(root.join("relay.toml"), "watch-bundles = true\n").expect("write base relay.toml");
    fs::write(overlay.join("relay.toml"), "watch-bundles = [\n")
        .expect("write malformed overlay relay.toml");

    load_relay_runtime_configuration(root, None, None)
        .expect_err("a malformed overlay file must fault rather than fall through");
}

#[test]
fn missing_file_reports_against_the_base_location() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path();

    // An operator would create the file in the base root, so that is where an
    // absence is reported — not under an overlay that may not exist.
    assert_eq!(
        effective_configuration_path(root, "ui.toml"),
        root.join("ui.toml")
    );
}

#[test]
fn every_file_kind_resolves_through_the_same_lookup() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path();
    let overlay = root.join("overlay");
    fs::create_dir_all(&overlay).expect("create overlay");
    fs::write(root.join("ui.toml"), "default-bundle = \"base-bundle\"\n")
        .expect("write base ui.toml");
    fs::write(
        overlay.join("ui.toml"),
        "default-bundle = \"overlay-bundle\"\n",
    )
    .expect("write overlay ui.toml");

    let loaded = load_ui_configuration(root)
        .expect("load ui configuration")
        .expect("expected ui configuration");
    assert_eq!(loaded.default_bundle.as_deref(), Some("overlay-bundle"));
}

#[test]
fn overlay_supplied_relative_paths_keep_their_existing_base() {
    // The overlay directory is not a resolution base. A relative member
    // directory declared in an overlay bundle resolves exactly as the identical
    // declaration in a base bundle file would.
    let temporary = TempDir::new().expect("temporary");
    let body = r#"
format-version = 1

[[sessions]]
id = "relative"
directory = "relative/workspace"
coder = "acp"
"#;
    let root = write_config(&temporary, "alpha", ACP_CODER, body);
    write_config(&temporary, "beta", ACP_CODER, body);
    let overlay_bundles = root.join("overlay/bundles");
    fs::create_dir_all(&overlay_bundles).expect("create overlay bundles");
    fs::write(overlay_bundles.join("beta.toml"), body).expect("write overlay bundle");

    let from_base = load_bundle_configuration(&root, "alpha").expect("load base bundle");
    let from_overlay = load_bundle_configuration(&root, "beta").expect("load overlay bundle");
    assert_eq!(
        from_base.members[0].working_directory, from_overlay.members[0].working_directory,
        "an overlay-supplied relative path must not rebase under the overlay"
    );
}
