//! Coverage for `relay::preflight_bundle_configuration` — the read-only startup
//! validation path backing `agentmux check configuration`. The happy path and
//! the bundle-schema failure mirror the public configuration loaders, while the
//! invalid-policy-scope case proves the pre-flight also exercises the
//! authorization layer (which is only reachable through this relay entrypoint,
//! not the public configuration loaders).

use super::*;

use agentmux::relay::preflight_bundle_configuration;

#[test]
fn preflight_accepts_valid_bundle() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");

    preflight_bundle_configuration(&config_root, "party")
        .expect("valid bundle pre-flights cleanly");
}

#[test]
fn preflight_rejects_unknown_bundle() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");

    let error = preflight_bundle_configuration(&config_root, "absent")
        .expect_err("absent bundle should fail pre-flight");
    assert_eq!(error.code, "validation_unknown_bundle");
}

#[test]
fn preflight_reports_invalid_relay_toml_peer() {
    // `agentmux check configuration` validates the expanded `relay.toml` schema
    // through the same load path as relay startup: an invalid `[[peers]]` entry
    // (empty address) surfaces here, not only at startup.
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    std::fs::write(
        config_root.base_layer().join("relay.toml"),
        "[[peers]]\nalias = \"west\"\naddress = \"\"\nconnect-as = \"east\"\n",
    )
    .expect("write relay.toml");

    let error = preflight_bundle_configuration(&config_root, "party")
        .expect_err("invalid relay.toml peer should fail pre-flight");
    assert_eq!(error.code, "validation_invalid_arguments");
    assert_eq!(
        error
            .details
            .as_ref()
            .and_then(|details| details.get("field"))
            .and_then(|field| field.as_str()),
        Some("peers.address"),
    );
}

#[test]
fn preflight_reports_duplicate_peer_alias() {
    // A duplicate `[[peers]].alias` collapses to one routable endpoint at
    // runtime, so pre-flight must reject it before the relay is started.
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle(&temporary, "party");
    std::fs::write(
        config_root.base_layer().join("relay.toml"),
        "[[peers]]\nalias = \"west\"\naddress = \"/run/agentmux/west-a.sock\"\nconnect-as = \"east\"\n\
         [[peers]]\nalias = \"west\"\naddress = \"/run/agentmux/west-b.sock\"\nconnect-as = \"north\"\n",
    )
    .expect("write relay.toml");

    let error = preflight_bundle_configuration(&config_root, "party")
        .expect_err("duplicate peer alias should fail pre-flight");
    assert_eq!(error.code, "validation_invalid_arguments");
    assert_eq!(
        error
            .details
            .as_ref()
            .and_then(|details| details.get("field"))
            .and_then(|field| field.as_str()),
        Some("peers.alias"),
    );
}

#[test]
fn preflight_reports_unknown_field_with_path_detail() {
    // The headline incident class: a misspelled session key (`codex-session-id`
    // for `coder`). `deny_unknown_fields` rejects it at parse time, and the
    // pre-flight surfaces the offending file path plus the field-level cause.
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle_with_policy(
        &temporary,
        "party",
        r#"
format-version = 1

[[sessions]]
id = "alpha"
directory = "/tmp"
coder = "shell"
codex-session-id = "alpha"
"#,
        Some(
            r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "home"
look = "self"
send = "home"
"#,
        ),
    );

    let error = preflight_bundle_configuration(&config_root, "party")
        .expect_err("unknown field should fail pre-flight");
    let details = error.details.expect("invalid-configuration details");
    let cause = details["cause"].as_str().expect("cause detail");
    assert!(
        cause.contains("codex-session-id"),
        "cause should name the offending field: {cause}"
    );
    let path = details["path"].as_str().expect("path detail");
    assert!(
        path.ends_with("party.toml"),
        "path should point at the bundle file: {path}"
    );
}

#[test]
fn preflight_reports_invalid_policy_scope() {
    // A policy-control scope typo is only caught by the authorization layer,
    // which the configuration loaders never reach — this is exactly the coverage
    // the relay pre-flight wrapper adds over a configuration-only check.
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle_with_policy(
        &temporary,
        "party",
        r#"
format-version = 1

[[sessions]]
id = "alpha"
directory = "/tmp"
coder = "shell"
"#,
        Some(
            r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "home"
look = "bogus-scope"
send = "home"
"#,
        ),
    );

    let error = preflight_bundle_configuration(&config_root, "party")
        .expect_err("invalid policy scope should fail pre-flight");
    assert_eq!(error.code, "validation_invalid_arguments");
    let details = error.details.expect("invalid-scope details");
    assert_eq!(details["control"], "look");
    assert_eq!(details["value"], "bogus-scope");
}
