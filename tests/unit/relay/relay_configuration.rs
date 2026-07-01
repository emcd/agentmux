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
    assert!(configuration.relay_id.is_none());
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

fn peer_field(error: &agentmux::relay::RelayError) -> Option<&str> {
    error
        .details
        .as_ref()
        .and_then(|details| details.get("field"))
        .and_then(|field| field.as_str())
}

#[test]
fn accepts_valid_outbound_peer() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "relay-id = \"this-relay\"\n[[peers]]\nid = \"peer-relay@RELAY\"\naddress = \"/run/agentmux/peer-relay.sock\"\n",
    );

    let configuration = load_relay_runtime_configuration(temporary.path(), None, None)
        .expect("load relay configuration");

    assert_eq!(configuration.relay_id.as_deref(), Some("this-relay"));
    assert_eq!(
        configuration.peers,
        vec![PeerConfiguration {
            id: "peer-relay@RELAY".to_string(),
            address: "/run/agentmux/peer-relay.sock".to_string(),
        }],
    );
    assert_eq!(configuration.peers[0].alias(), "peer-relay");
}

#[test]
fn rejects_peer_without_id() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "relay-id = \"this-relay\"\n[[peers]]\naddress = \"/run/agentmux/peer.sock\"\n",
    );

    let error = load_relay_runtime_configuration(temporary.path(), None, None)
        .expect_err("reject missing id");
    assert_eq!(error.code, "validation_invalid_arguments");
}

#[test]
fn rejects_peer_id_outside_relay_namespace() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "relay-id = \"this-relay\"\n[[peers]]\nid = \"peer-relay@GLOBAL\"\naddress = \"/run/agentmux/peer.sock\"\n",
    );

    let error = load_relay_runtime_configuration(temporary.path(), None, None)
        .expect_err("reject non-RELAY id");
    assert_eq!(error.code, "validation_invalid_arguments");
    assert_eq!(peer_field(&error), Some("peers.id"));
}

#[test]
fn rejects_peer_without_address() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "relay-id = \"this-relay\"\n[[peers]]\nid = \"peer-relay@RELAY\"\n",
    );

    let error = load_relay_runtime_configuration(temporary.path(), None, None)
        .expect_err("reject missing address");
    assert_eq!(error.code, "validation_invalid_arguments");
}

#[test]
fn rejects_empty_peer_address() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "relay-id = \"this-relay\"\n[[peers]]\nid = \"peer-relay@RELAY\"\naddress = \"\"\n",
    );

    let error = load_relay_runtime_configuration(temporary.path(), None, None)
        .expect_err("reject empty address");
    assert_eq!(error.code, "validation_invalid_arguments");
    assert_eq!(peer_field(&error), Some("peers.address"));
}

#[test]
fn rejects_non_absolute_peer_address() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "relay-id = \"this-relay\"\n[[peers]]\nid = \"peer-relay@RELAY\"\naddress = \"relay.example:9000\"\n",
    );

    let error = load_relay_runtime_configuration(temporary.path(), None, None)
        .expect_err("reject host:port address");
    assert_eq!(error.code, "validation_invalid_arguments");
    assert_eq!(peer_field(&error), Some("peers.address"));
}

#[test]
fn rejects_scope_on_peer_entry() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "relay-id = \"this-relay\"\n[[peers]]\nid = \"peer-relay@RELAY\"\naddress = \"/run/agentmux/peer.sock\"\nscope = \"myapp\"\n",
    );

    let error = load_relay_runtime_configuration(temporary.path(), None, None)
        .expect_err("reject scope on outbound peer");
    assert_eq!(error.code, "validation_invalid_arguments");
}

#[test]
fn rejects_unknown_peer_field() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "relay-id = \"this-relay\"\n[[peers]]\nid = \"peer-relay@RELAY\"\naddress = \"/run/agentmux/peer.sock\"\nbogus = true\n",
    );

    let error = load_relay_runtime_configuration(temporary.path(), None, None)
        .expect_err("reject unknown peer field");
    assert_eq!(error.code, "validation_invalid_arguments");
}

#[test]
fn rejects_missing_relay_id_when_peers_present() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "[[peers]]\nid = \"peer-relay@RELAY\"\naddress = \"/run/agentmux/peer.sock\"\n",
    );

    let error = load_relay_runtime_configuration(temporary.path(), None, None)
        .expect_err("reject peers without relay-id");
    assert_eq!(error.code, "validation_invalid_arguments");
    assert_eq!(peer_field(&error), Some("relay-id"));
}

#[test]
fn rejects_qualified_relay_id() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(temporary.path(), "relay-id = \"this-relay@RELAY\"\n");

    let error = load_relay_runtime_configuration(temporary.path(), None, None)
        .expect_err("reject qualified relay-id");
    assert_eq!(error.code, "validation_invalid_arguments");
    assert_eq!(peer_field(&error), Some("relay-id"));
}

#[test]
fn rejects_relay_id_with_bang() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(temporary.path(), "relay-id = \"this!relay\"\n");

    let error = load_relay_runtime_configuration(temporary.path(), None, None)
        .expect_err("reject relay-id with bang");
    assert_eq!(error.code, "validation_invalid_arguments");
}

#[test]
fn rejects_blank_relay_id() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(temporary.path(), "relay-id = \"   \"\n");

    let error = load_relay_runtime_configuration(temporary.path(), None, None)
        .expect_err("reject blank relay-id");
    assert_eq!(error.code, "validation_invalid_arguments");
}

#[test]
fn accepts_relay_id_without_peers() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(temporary.path(), "relay-id = \"this-relay\"\n");

    let configuration = load_relay_runtime_configuration(temporary.path(), None, None)
        .expect("load relay configuration");

    assert_eq!(configuration.relay_id.as_deref(), Some("this-relay"));
    assert!(configuration.peers.is_empty());
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
