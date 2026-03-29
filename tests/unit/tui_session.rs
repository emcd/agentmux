use std::fs;

use agentmux::{
    configuration::TuiConfiguration,
    runtime::tui_session::{load_active_tui_configuration, resolve_tui_session_identity},
};
use tempfile::TempDir;

fn write_policies(configuration_root: &std::path::Path, policy_ids: &[&str]) {
    let mut body = String::from("format-version = 1\n");
    for policy_id in policy_ids {
        body.push_str(
            format!(
                "\n[[policies]]\nid = \"{}\"\n\n[policies.controls]\nfind = \"self\"\nlist = \"all:home\"\nlook = \"all:home\"\nsend = \"all:home\"\n",
                policy_id
            )
            .as_str(),
        );
    }
    fs::write(configuration_root.join("policies.toml"), body).expect("write policies.toml");
}

fn write_tui(configuration_root: &std::path::Path, body: &str) {
    fs::write(configuration_root.join("tui.toml"), body).expect("write tui.toml");
}

#[test]
fn resolves_explicit_bundle_and_session_selector() {
    let temporary = TempDir::new().expect("temporary");
    write_policies(temporary.path(), &["default"]);
    write_tui(
        temporary.path(),
        r#"
default-bundle = "bundle-default"
default-session = "user"

[[sessions]]
id = "user"
policy = "default"
"#,
    );

    let resolved = resolve_tui_session_identity(
        temporary.path(),
        temporary.path(),
        Some("agentmux"),
        Some("user"),
    )
    .expect("resolve explicit session");
    assert_eq!(resolved.bundle_name, "agentmux");
    assert_eq!(resolved.session_selector, "user");
    assert_eq!(resolved.session_id, "user");
    assert_eq!(resolved.policy_id, "default");
}

#[test]
fn resolves_defaults_when_selectors_are_omitted() {
    let temporary = TempDir::new().expect("temporary");
    write_policies(temporary.path(), &["default"]);
    write_tui(
        temporary.path(),
        r#"
default-bundle = "agentmux"
default-session = "user"

[[sessions]]
id = "user"
policy = "default"
"#,
    );

    let resolved = resolve_tui_session_identity(temporary.path(), temporary.path(), None, None)
        .expect("resolve defaults");
    assert_eq!(resolved.bundle_name, "agentmux");
    assert_eq!(resolved.session_selector, "user");
}

#[test]
fn rejects_missing_default_bundle_when_bundle_is_omitted() {
    let temporary = TempDir::new().expect("temporary");
    write_policies(temporary.path(), &["default"]);
    write_tui(
        temporary.path(),
        r#"
default-session = "user"

[[sessions]]
id = "user"
policy = "default"
"#,
    );

    let error = resolve_tui_session_identity(temporary.path(), temporary.path(), None, None)
        .expect_err("missing default bundle should fail");
    assert!(error.to_string().contains("validation_unknown_bundle"));
}

#[test]
fn rejects_unknown_session_selector() {
    let temporary = TempDir::new().expect("temporary");
    write_policies(temporary.path(), &["default"]);
    write_tui(
        temporary.path(),
        r#"
default-bundle = "agentmux"

[[sessions]]
id = "user"
policy = "default"
"#,
    );

    let error = resolve_tui_session_identity(
        temporary.path(),
        temporary.path(),
        Some("agentmux"),
        Some("ghost"),
    )
    .expect_err("unknown session should fail");
    assert!(error.to_string().contains("validation_unknown_session"));
}

#[test]
fn rejects_session_with_unknown_policy_reference() {
    let temporary = TempDir::new().expect("temporary");
    write_policies(temporary.path(), &["default"]);
    write_tui(
        temporary.path(),
        r#"
default-bundle = "agentmux"
default-session = "user"

[[sessions]]
id = "user"
policy = "missing"
"#,
    );

    let error = resolve_tui_session_identity(temporary.path(), temporary.path(), None, None)
        .expect_err("unknown policy should fail");
    assert!(error.to_string().contains("validation_unknown_policy"));
}

#[test]
fn loads_local_override_in_debug_builds() {
    let temporary = TempDir::new().expect("temporary");
    write_tui(
        temporary.path(),
        r#"
default-bundle = "agentmux"
default-session = "normal"

[[sessions]]
id = "normal"
policy = "default"
"#,
    );
    let override_directory = temporary
        .path()
        .join(".auxiliary/configuration/agentmux/overrides");
    fs::create_dir_all(&override_directory).expect("create override directory");
    fs::write(
        override_directory.join("tui.toml"),
        r#"
default-bundle = "agentmux"
default-session = "override"

[[sessions]]
id = "override"
policy = "default"
"#,
    )
    .expect("write override file");

    let loaded =
        load_active_tui_configuration(temporary.path(), temporary.path()).expect("load config");
    let Some(TuiConfiguration {
        default_session, ..
    }) = loaded
    else {
        panic!("expected active tui configuration");
    };
    if cfg!(debug_assertions) {
        assert_eq!(default_session.as_deref(), Some("override"));
    } else {
        assert_eq!(default_session.as_deref(), Some("normal"));
    }
}
