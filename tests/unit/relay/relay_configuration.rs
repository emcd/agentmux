//! Relay-wide `relay.toml` configuration loading, override precedence, and
//! `[[peers]]` placeholder validation.
//!
//! Exercises the normalized loader ([`load_relay_runtime_configuration`]) plus
//! the pure precedence ([`resolve_relay_bool_setting`]) and environment-value
//! ([`parse_relay_bool_env_value`]) seams the loader composes, so the precedence
//! ladder and validation are covered without mutating process environment.

use std::path::Path;

use agentmux::relay::{
    PeerConfiguration, load_relay_runtime_configuration, parse_relay_bool_env_value,
    resolve_relay_bool_setting,
};
use tempfile::TempDir;

fn write_relay_toml(directory: &Path, contents: &str) {
    std::fs::write(directory.join("relay.toml"), contents).expect("write relay.toml");
}

#[test]
fn defaults_when_relay_toml_absent() {
    let temporary = TempDir::new().expect("temporary directory");

    let configuration = load_relay_runtime_configuration(temporary.path(), None, None)
        .expect("load relay configuration");

    assert!(configuration.watch_bundles);
    assert!(!configuration.require_session_credentials);
    assert_eq!(configuration.choices_pending_max, 256);
    assert!(configuration.peers.is_empty());
}

#[test]
fn loads_explicit_relay_controls() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "watch-bundles = false\nrequire-session-credentials = true\n",
    );

    let configuration = load_relay_runtime_configuration(temporary.path(), None, None)
        .expect("load relay configuration");

    assert!(!configuration.watch_bundles);
    assert!(configuration.require_session_credentials);
}

#[test]
fn cli_override_wins_over_file_value() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(temporary.path(), "watch-bundles = true\n");

    let configuration = load_relay_runtime_configuration(temporary.path(), Some(false), None)
        .expect("load relay configuration");

    assert!(
        !configuration.watch_bundles,
        "CLI override must win over the relay.toml value",
    );
}

#[test]
fn rejects_nested_relay_table() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(temporary.path(), "[relay]\nwatch-bundles = false\n");

    let error =
        load_relay_runtime_configuration(temporary.path(), None, None).expect_err("reject nested");
    assert_eq!(error.code, "validation_invalid_arguments");
}

#[test]
fn rejects_unknown_top_level_field() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(temporary.path(), "watch-bundle = false\n");

    let error =
        load_relay_runtime_configuration(temporary.path(), None, None).expect_err("reject unknown");
    assert_eq!(error.code, "validation_invalid_arguments");
}

#[test]
fn rejects_wrong_field_type() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(temporary.path(), "watch-bundles = 'false'\n");

    let error =
        load_relay_runtime_configuration(temporary.path(), None, None).expect_err("reject type");
    assert_eq!(error.code, "validation_invalid_arguments");
}

#[test]
fn accepts_valid_peer_placeholder() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "[[peers]]\naddress = \"relay.example:9000\"\n",
    );

    let configuration = load_relay_runtime_configuration(temporary.path(), None, None)
        .expect("load relay configuration");

    assert_eq!(
        configuration.peers,
        vec![PeerConfiguration {
            address: "relay.example:9000".to_string(),
        }],
    );
}

#[test]
fn rejects_peer_without_address() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(temporary.path(), "[[peers]]\n");

    let error = load_relay_runtime_configuration(temporary.path(), None, None)
        .expect_err("reject missing address");
    assert_eq!(error.code, "validation_invalid_arguments");
}

#[test]
fn rejects_empty_peer_address() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(temporary.path(), "[[peers]]\naddress = \"\"\n");

    let error = load_relay_runtime_configuration(temporary.path(), None, None)
        .expect_err("reject empty address");
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
fn rejects_unknown_peer_field() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "[[peers]]\naddress = \"relay.example:9000\"\nbogus = true\n",
    );

    let error = load_relay_runtime_configuration(temporary.path(), None, None)
        .expect_err("reject unknown peer field");
    assert_eq!(error.code, "validation_invalid_arguments");
}

#[test]
fn rejects_choices_pending_max_out_of_range() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(temporary.path(), "[choices]\npending-max = 10000\n");

    let error = load_relay_runtime_configuration(temporary.path(), None, None)
        .expect_err("reject out-of-range choices");
    assert_eq!(error.code, "validation_invalid_arguments");
    assert_eq!(
        error
            .details
            .as_ref()
            .and_then(|details| details.get("field"))
            .and_then(|field| field.as_str()),
        Some("choices.pending-max"),
    );
}

#[test]
fn precedence_cli_wins_over_environment_and_file() {
    assert!(!resolve_relay_bool_setting(
        Some(false),
        Some(true),
        Some(true),
        true
    ));
}

#[test]
fn precedence_environment_wins_over_file() {
    assert!(resolve_relay_bool_setting(
        None,
        Some(true),
        Some(false),
        false
    ));
}

#[test]
fn precedence_file_wins_over_default() {
    assert!(!resolve_relay_bool_setting(None, None, Some(false), true));
}

#[test]
fn precedence_falls_back_to_default() {
    assert!(resolve_relay_bool_setting(None, None, None, true));
    assert!(!resolve_relay_bool_setting(None, None, None, false));
}

#[test]
fn environment_value_accepts_canonical_booleans() {
    assert!(
        parse_relay_bool_env_value("AGENTMUX_RELAY_WATCH_BUNDLES", "true").expect("accept true"),
    );
    assert!(
        !parse_relay_bool_env_value("AGENTMUX_RELAY_WATCH_BUNDLES", "false").expect("accept false"),
    );
}

#[test]
fn environment_value_rejects_non_canonical() {
    let error = parse_relay_bool_env_value("AGENTMUX_RELAY_WATCH_BUNDLES", "maybe")
        .expect_err("reject non-canonical");
    assert_eq!(error.code, "validation_invalid_arguments");
}
