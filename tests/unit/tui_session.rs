use std::fs;

use agentmux::{
    configuration::TuiConfiguration,
    runtime::tui_session::{
        load_active_tui_configuration, resolve_tui_launch_identity, resolve_tui_session_identity,
    },
};
use tempfile::TempDir;

fn write_policies(configuration_root: &std::path::Path, policy_ids: &[&str]) {
    let mut body = String::from("format-version = 1\n");
    for policy_id in policy_ids {
        body.push_str(
            format!(
                "\n[[policies]]\nid = \"{}\"\n\n[policies.controls]\nfind = \"self\"\nlist = \"home\"\nlook = \"home\"\nsend = \"home\"\n",
                policy_id
            )
            .as_str(),
        );
    }
    fs::write(configuration_root.join("policies.toml"), body).expect("write policies.toml");
}

fn write_users(configuration_root: &std::path::Path, body: &str) {
    fs::write(configuration_root.join("users.toml"), body).expect("write users.toml");
}

fn write_ui(configuration_root: &std::path::Path, default_bundle: &str) {
    fs::write(
        configuration_root.join("ui.toml"),
        format!("default-bundle = \"{default_bundle}\"\n"),
    )
    .expect("write ui.toml");
}

#[test]
fn resolves_explicit_bundle_and_session_selector() {
    let temporary = TempDir::new().expect("temporary");
    write_policies(temporary.path(), &["default"]);
    write_users(
        temporary.path(),
        r#"
default-session = "user@GLOBAL"

[[sessions]]
id = "user@GLOBAL"
policy = "default"

[sessions.ui]
"#,
    );

    let resolved =
        resolve_tui_session_identity(temporary.path(), Some("agentmux"), Some("user@GLOBAL"))
            .expect("resolve explicit session");
    assert_eq!(resolved.namespace, "agentmux");
    assert_eq!(resolved.session_selector, "user@GLOBAL");
    assert_eq!(resolved.session_id, "user@GLOBAL");
    assert_eq!(resolved.policy, "default");
}

#[test]
fn resolves_default_bundle_from_ui_toml_when_selectors_are_omitted() {
    let temporary = TempDir::new().expect("temporary");
    write_policies(temporary.path(), &["default"]);
    write_ui(temporary.path(), "agentmux");
    write_users(
        temporary.path(),
        r#"
default-session = "user@GLOBAL"

[[sessions]]
id = "user@GLOBAL"
policy = "default"

[sessions.ui]
"#,
    );

    let resolved =
        resolve_tui_session_identity(temporary.path(), None, None).expect("resolve defaults");
    assert_eq!(resolved.namespace, "agentmux");
    assert_eq!(resolved.session_selector, "user@GLOBAL");
}

#[test]
fn rejects_missing_default_bundle_when_bundle_is_omitted() {
    let temporary = TempDir::new().expect("temporary");
    write_policies(temporary.path(), &["default"]);
    write_users(
        temporary.path(),
        r#"
default-session = "user@GLOBAL"

[[sessions]]
id = "user@GLOBAL"
policy = "default"

[sessions.ui]
"#,
    );

    let error = resolve_tui_session_identity(temporary.path(), None, None)
        .expect_err("missing default bundle should fail");
    let rendered = error.to_string();
    assert!(rendered.contains("validation_unknown_bundle"));
    assert!(
        rendered.contains("ui.toml default-bundle"),
        "strict error should name ui.toml as the default-bundle source: {rendered}"
    );
}

#[test]
fn launch_without_default_bundle_seeds_from_fallback() {
    let temporary = TempDir::new().expect("temporary");
    write_policies(temporary.path(), &["default"]);
    write_users(
        temporary.path(),
        r#"
default-session = "user@GLOBAL"

[[sessions]]
id = "user@GLOBAL"
policy = "default"

[sessions.ui]
"#,
    );

    let resolved =
        resolve_tui_launch_identity(temporary.path(), None, None, Some("first-available"))
            .expect("launch without default bundle should succeed");
    assert_eq!(resolved.namespace, "first-available");
    assert_eq!(resolved.session_id, "user@GLOBAL");
    assert_eq!(resolved.policy, "default");
}

#[test]
fn launch_without_default_bundle_or_fallback_is_empty_not_error() {
    let temporary = TempDir::new().expect("temporary");
    write_policies(temporary.path(), &["default"]);
    write_users(
        temporary.path(),
        r#"
default-session = "user@GLOBAL"

[[sessions]]
id = "user@GLOBAL"
policy = "default"

[sessions.ui]
"#,
    );

    let resolved = resolve_tui_launch_identity(temporary.path(), None, None, None)
        .expect("launch with no bundle context should still succeed");
    assert_eq!(resolved.namespace, "");
    assert_eq!(resolved.session_id, "user@GLOBAL");
}

#[test]
fn launch_prefers_ui_default_bundle_over_fallback() {
    let temporary = TempDir::new().expect("temporary");
    write_policies(temporary.path(), &["default"]);
    write_ui(temporary.path(), "configured");
    write_users(
        temporary.path(),
        r#"
default-session = "user@GLOBAL"

[[sessions]]
id = "user@GLOBAL"
policy = "default"

[sessions.ui]
"#,
    );

    let resolved =
        resolve_tui_launch_identity(temporary.path(), None, None, Some("first-available"))
            .expect("configured default bundle should win");
    assert_eq!(resolved.namespace, "configured");
}

#[test]
fn launch_still_requires_a_session() {
    let temporary = TempDir::new().expect("temporary");
    write_policies(temporary.path(), &["default"]);
    write_users(
        temporary.path(),
        r#"
[[sessions]]
id = "user@GLOBAL"
policy = "default"

[sessions.ui]
"#,
    );

    let error = resolve_tui_launch_identity(temporary.path(), None, None, None)
        .expect_err("missing default session should still fail");
    assert!(error.to_string().contains("validation_unknown_session"));
}

#[test]
fn rejects_unknown_session_selector() {
    let temporary = TempDir::new().expect("temporary");
    write_policies(temporary.path(), &["default"]);
    write_users(
        temporary.path(),
        r#"
[[sessions]]
id = "user@GLOBAL"
policy = "default"

[sessions.ui]
"#,
    );

    let error = resolve_tui_session_identity(temporary.path(), Some("agentmux"), Some("ghost"))
        .expect_err("unknown session should fail");
    assert!(error.to_string().contains("validation_unknown_session"));
}

#[test]
fn rejects_session_with_unknown_policy_reference() {
    let temporary = TempDir::new().expect("temporary");
    write_policies(temporary.path(), &["default"]);
    write_ui(temporary.path(), "agentmux");
    write_users(
        temporary.path(),
        r#"
default-session = "user@GLOBAL"

[[sessions]]
id = "user@GLOBAL"
policy = "missing"

[sessions.ui]
"#,
    );

    let error = resolve_tui_session_identity(temporary.path(), None, None)
        .expect_err("unknown policy should fail");
    assert!(error.to_string().contains("validation_unknown_policy"));
}

#[test]
fn overlay_users_file_shadows_the_base_in_every_build_profile() {
    let temporary = TempDir::new().expect("temporary");
    write_users(
        temporary.path(),
        r#"
default-session = "normal@GLOBAL"

[[sessions]]
id = "normal@GLOBAL"
policy = "default"

[sessions.ui]
"#,
    );
    let overlay_directory = temporary.path().join("overlay");
    fs::create_dir_all(&overlay_directory).expect("create overlay directory");
    fs::write(
        overlay_directory.join("users.toml"),
        r#"
default-session = "override@GLOBAL"

[[sessions]]
id = "override@GLOBAL"
policy = "default"

[sessions.ui]
"#,
    )
    .expect("write override file");

    let loaded = load_active_tui_configuration(temporary.path()).expect("load config");
    let Some(TuiConfiguration {
        default_session, ..
    }) = loaded
    else {
        panic!("expected active tui configuration");
    };
    // Honored regardless of build profile: the overlay is the one mechanism for
    // per-tree divergence, and gating it on optimization level misclassified a
    // release binary run from a checkout.
    assert_eq!(default_session.as_deref(), Some("override@GLOBAL"));
}

/// An overlay `users.toml` swaps identity (`default-session`) only. `ui.toml`
/// resolves through its own overlay lookup independently, so overlaying one file
/// does not silently pull the other from the overlay layer.
#[test]
fn override_swaps_identity_but_not_the_ui_default_bundle() {
    let temporary = TempDir::new().expect("temporary");
    write_policies(temporary.path(), &["default"]);
    write_ui(temporary.path(), "root-bundle");
    write_users(
        temporary.path(),
        r#"
default-session = "normal@GLOBAL"

[[sessions]]
id = "normal@GLOBAL"
policy = "default"

[sessions.ui]
"#,
    );
    let overlay_directory = temporary.path().join("overlay");
    fs::create_dir_all(&overlay_directory).expect("create overlay directory");
    fs::write(
        overlay_directory.join("users.toml"),
        r#"
default-session = "override@GLOBAL"

[[sessions]]
id = "override@GLOBAL"
policy = "default"

[sessions.ui]
"#,
    )
    .expect("write override file");

    let resolved = resolve_tui_launch_identity(temporary.path(), None, None, None)
        .expect("resolve launch identity");
    // The overlay defines no ui.toml, so the browsing bundle still comes from
    // the base file.
    assert_eq!(resolved.namespace, "root-bundle");
    assert_eq!(resolved.session_id, "override@GLOBAL");
}
