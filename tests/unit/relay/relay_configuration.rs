//! Relay-wide `relay.toml` configuration loading, override precedence, and
//! `[[peers]]` placeholder validation.
//!
//! Exercises the normalized loader ([`load_relay_runtime_configuration`]) plus
//! the pure precedence ([`resolve_relay_bool_setting`]) and environment-value
//! ([`parse_relay_bool_env_value`]) seams the loader composes, so the precedence
//! ladder and validation are covered without mutating process environment.

use agentmux::configuration::ConfigurationRoots;
use std::path::Path;

use agentmux::relay::{
    DeliveryConfiguration, PeerConfiguration, load_relay_runtime_configuration,
    parse_relay_bool_env_value, resolve_relay_bool_setting,
};
use tempfile::TempDir;

fn write_relay_toml(directory: &Path, contents: &str) {
    std::fs::write(directory.join("relay.toml"), contents).expect("write relay.toml");
}

#[test]
fn defaults_when_relay_toml_absent() {
    let temporary = TempDir::new().expect("temporary directory");

    let configuration =
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
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

    let configuration =
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
            .expect("load relay configuration");

    assert!(!configuration.watch_bundles);
    assert!(configuration.require_session_credentials);
}

#[test]
fn cli_override_wins_over_file_value() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(temporary.path(), "watch-bundles = true\n");

    let configuration = load_relay_runtime_configuration(
        &ConfigurationRoots::single(temporary.path()),
        Some(false),
        None,
    )
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
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
            .expect_err("reject nested");
    assert_eq!(error.code, "validation_invalid_arguments");
}

#[test]
fn rejects_unknown_top_level_field() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(temporary.path(), "watch-bundle = false\n");

    let error =
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
            .expect_err("reject unknown");
    assert_eq!(error.code, "validation_invalid_arguments");
}

#[test]
fn rejects_wrong_field_type() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(temporary.path(), "watch-bundles = 'false'\n");

    let error =
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
            .expect_err("reject type");
    assert_eq!(error.code, "validation_invalid_arguments");
}

fn peer_field(error: &agentmux::relay::RelayError) -> Option<&str> {
    error
        .details
        .as_ref()
        .and_then(|details| details.get("field"))
        .and_then(|field| field.as_str())
}

const VALID_PEER: &str =
    "[[peers]]\nalias = \"west\"\naddress = \"/run/agentmux/west.sock\"\nconnect-as = \"east\"\n";

#[test]
fn accepts_valid_outbound_peer() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(temporary.path(), VALID_PEER);

    let configuration =
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
            .expect("load relay configuration");

    assert_eq!(
        configuration.peers,
        vec![PeerConfiguration {
            alias: "west".to_string(),
            address: "/run/agentmux/west.sock".to_string(),
            connect_as: "east".to_string(),
        }],
    );
    assert_eq!(configuration.peers[0].alias, "west");
    assert_eq!(configuration.peers[0].connect_as, "east");
}

#[test]
fn rejects_peer_without_alias() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "[[peers]]\naddress = \"/run/agentmux/west.sock\"\nconnect-as = \"east\"\n",
    );

    let error =
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
            .expect_err("reject missing alias");
    assert_eq!(error.code, "validation_invalid_arguments");
}

#[test]
fn rejects_peer_without_connect_as() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "[[peers]]\nalias = \"west\"\naddress = \"/run/agentmux/west.sock\"\n",
    );

    let error =
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
            .expect_err("reject missing connect-as");
    assert_eq!(error.code, "validation_invalid_arguments");
}

#[test]
fn rejects_peer_without_address() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "[[peers]]\nalias = \"west\"\nconnect-as = \"east\"\n",
    );

    let error =
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
            .expect_err("reject missing address");
    assert_eq!(error.code, "validation_invalid_arguments");
}

#[test]
fn rejects_empty_peer_address() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "[[peers]]\nalias = \"west\"\naddress = \"\"\nconnect-as = \"east\"\n",
    );

    let error =
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
            .expect_err("reject empty address");
    assert_eq!(error.code, "validation_invalid_arguments");
    assert_eq!(peer_field(&error), Some("peers.address"));
}

#[test]
fn rejects_non_absolute_peer_address() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "[[peers]]\nalias = \"west\"\naddress = \"relay.example:9000\"\nconnect-as = \"east\"\n",
    );

    let error =
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
            .expect_err("reject host:port address");
    assert_eq!(error.code, "validation_invalid_arguments");
    assert_eq!(peer_field(&error), Some("peers.address"));
}

#[test]
fn rejects_qualified_connect_as() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "[[peers]]\nalias = \"west\"\naddress = \"/run/agentmux/west.sock\"\nconnect-as = \"east@RELAY\"\n",
    );

    let error =
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
            .expect_err("reject qualified connect-as");
    assert_eq!(error.code, "validation_invalid_arguments");
    assert_eq!(peer_field(&error), Some("peers.connect-as"));
}

#[test]
fn rejects_alias_with_bang() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "[[peers]]\nalias = \"we!st\"\naddress = \"/run/agentmux/west.sock\"\nconnect-as = \"east\"\n",
    );

    let error =
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
            .expect_err("reject alias with bang");
    assert_eq!(error.code, "validation_invalid_arguments");
    assert_eq!(peer_field(&error), Some("peers.alias"));
}

#[test]
fn rejects_blank_alias() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "[[peers]]\nalias = \"   \"\naddress = \"/run/agentmux/west.sock\"\nconnect-as = \"east\"\n",
    );

    let error =
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
            .expect_err("reject blank alias");
    assert_eq!(error.code, "validation_invalid_arguments");
    assert_eq!(peer_field(&error), Some("peers.alias"));
}

#[test]
fn rejects_duplicate_peer_alias() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "[[peers]]\nalias = \"west\"\naddress = \"/run/agentmux/west-a.sock\"\nconnect-as = \"east\"\n\
         [[peers]]\nalias = \"west\"\naddress = \"/run/agentmux/west-b.sock\"\nconnect-as = \"north\"\n",
    );

    let error =
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
            .expect_err("reject duplicate alias");
    assert_eq!(error.code, "validation_invalid_arguments");
    assert_eq!(peer_field(&error), Some("peers.alias"));
}

#[test]
fn accepts_duplicate_connect_as_across_peers() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "[[peers]]\nalias = \"west\"\naddress = \"/run/agentmux/west.sock\"\nconnect-as = \"east\"\n\
         [[peers]]\nalias = \"north\"\naddress = \"/run/agentmux/north.sock\"\nconnect-as = \"east\"\n",
    );

    let configuration =
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
            .expect("distinct aliases with a shared receiver-issued connect-as are valid");
    assert_eq!(configuration.peers.len(), 2);
    assert_eq!(configuration.peers[0].connect_as, "east");
    assert_eq!(configuration.peers[1].connect_as, "east");
}

#[test]
fn rejects_scope_on_peer_entry() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "[[peers]]\nalias = \"west\"\naddress = \"/run/agentmux/west.sock\"\nconnect-as = \"east\"\nscope = \"myapp\"\n",
    );

    let error =
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
            .expect_err("reject scope on outbound peer");
    assert_eq!(error.code, "validation_invalid_arguments");
}

#[test]
fn rejects_unknown_peer_field() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(
        temporary.path(),
        "[[peers]]\nalias = \"west\"\naddress = \"/run/agentmux/west.sock\"\nconnect-as = \"east\"\nbogus = true\n",
    );

    let error =
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
            .expect_err("reject unknown peer field");
    assert_eq!(error.code, "validation_invalid_arguments");
}

#[test]
fn accepts_relay_without_peers() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(temporary.path(), "watch-bundles = true\n");

    let configuration =
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
            .expect("load relay configuration");

    assert!(configuration.peers.is_empty());
}

#[test]
fn rejects_choices_pending_max_out_of_range() {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(temporary.path(), "[choices]\npending-max = 10000\n");

    let error =
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
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

/// Reads one detail field off a structured loader rejection, so a test can name
/// the exact key and bound the loader reported rather than only its error code.
fn detail<'a>(error: &'a agentmux::relay::RelayError, key: &str) -> Option<&'a serde_json::Value> {
    error.details.as_ref().and_then(|details| details.get(key))
}

fn load_delivery(contents: &str) -> Result<DeliveryConfiguration, agentmux::relay::RelayError> {
    let temporary = TempDir::new().expect("temporary directory");
    write_relay_toml(temporary.path(), contents);
    load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
        .map(|configuration| configuration.delivery)
}

#[test]
fn delivery_defaults_when_relay_toml_absent() {
    let temporary = TempDir::new().expect("temporary directory");

    let delivery =
        load_relay_runtime_configuration(&ConfigurationRoots::single(temporary.path()), None, None)
            .expect("load relay configuration")
            .delivery;

    assert_eq!(delivery.submission_timeout_ms, 5_000);
    assert_eq!(delivery.fence_observation_timeout_ms, 5_000);
    assert_eq!(delivery.queued_envelopes_max, 10_000);
    assert_eq!(delivery.queued_bytes_max, 268_435_456);
    assert_eq!(delivery.queued_envelopes_per_target_max, 1_000);
    assert_eq!(delivery.queued_bytes_per_target_max, 33_554_432);
    assert_eq!(delivery.undelivered_warning_ms, 1_800_000);
    assert_eq!(delivery.undelivered_report_interval_ms, 300_000);
}

#[test]
fn loads_explicit_delivery_settings() {
    let delivery = load_delivery(
        "[delivery]\n\
         submission-timeout-ms = 1500\n\
         fence-observation-timeout-ms = 2500\n\
         queued-envelopes-max = 5000\n\
         queued-bytes-max = 134217728\n\
         queued-envelopes-per-target-max = 500\n\
         queued-bytes-per-target-max = 16777216\n\
         undelivered-warning-ms = 900000\n\
         undelivered-report-interval-ms = 60000\n",
    )
    .expect("load delivery configuration");

    assert_eq!(delivery.submission_timeout_ms, 1_500);
    assert_eq!(delivery.fence_observation_timeout_ms, 2_500);
    assert_eq!(delivery.queued_envelopes_max, 5_000);
    assert_eq!(delivery.queued_bytes_max, 134_217_728);
    assert_eq!(delivery.queued_envelopes_per_target_max, 500);
    assert_eq!(delivery.queued_bytes_per_target_max, 16_777_216);
    assert_eq!(delivery.undelivered_warning_ms, 900_000);
    assert_eq!(delivery.undelivered_report_interval_ms, 60_000);
}

/// A partial `[delivery]` table takes the documented default for every key it
/// omits, rather than zeroing them.
#[test]
fn partial_delivery_table_defaults_the_rest() {
    let delivery = load_delivery("[delivery]\nsubmission-timeout-ms = 750\n")
        .expect("load delivery configuration");

    assert_eq!(delivery.submission_timeout_ms, 750);
    assert_eq!(delivery.queued_envelopes_max, 10_000);
    assert_eq!(delivery.undelivered_warning_ms, 1_800_000);
}

#[test]
fn rejects_out_of_range_undelivered_warning() {
    let error = load_delivery("[delivery]\nundelivered-warning-ms = 59999\n")
        .expect_err("reject warning below the permitted minimum");

    assert_eq!(error.code, "validation_invalid_arguments");
    assert_eq!(
        detail(&error, "field").and_then(serde_json::Value::as_str),
        Some("delivery.undelivered-warning-ms"),
    );
    assert_eq!(
        detail(&error, "value").and_then(serde_json::Value::as_u64),
        Some(59_999),
    );
    assert_eq!(
        detail(&error, "minimum").and_then(serde_json::Value::as_u64),
        Some(60_000),
    );
    assert_eq!(
        detail(&error, "maximum").and_then(serde_json::Value::as_u64),
        Some(86_400_000),
    );
}

/// Zero is out of range for every `[delivery]` key and carries no "unlimited"
/// meaning; it is rejected with the same structured range error as any other
/// out-of-range value rather than a bespoke one.
#[test]
fn rejects_zero_as_a_quota() {
    let error =
        load_delivery("[delivery]\nqueued-envelopes-max = 0\n").expect_err("reject a zero quota");

    assert_eq!(error.code, "validation_invalid_arguments");
    assert_eq!(
        detail(&error, "field").and_then(serde_json::Value::as_str),
        Some("delivery.queued-envelopes-max"),
    );
    assert_eq!(
        detail(&error, "minimum").and_then(serde_json::Value::as_u64),
        Some(1),
    );
}

#[test]
fn rejects_per_target_envelope_quota_above_global() {
    let error = load_delivery(
        "[delivery]\nqueued-envelopes-max = 100\nqueued-envelopes-per-target-max = 101\n",
    )
    .expect_err("reject unreachable per-target quota");

    assert_eq!(error.code, "validation_invalid_arguments");
    assert_eq!(
        detail(&error, "field").and_then(serde_json::Value::as_str),
        Some("delivery.queued-envelopes-per-target-max"),
    );
    assert_eq!(
        detail(&error, "value").and_then(serde_json::Value::as_u64),
        Some(101),
    );
    assert_eq!(
        detail(&error, "global_field").and_then(serde_json::Value::as_str),
        Some("delivery.queued-envelopes-max"),
    );
    assert_eq!(
        detail(&error, "global_value").and_then(serde_json::Value::as_u64),
        Some(100),
    );
}

#[test]
fn rejects_per_target_byte_quota_above_global() {
    let error = load_delivery(
        "[delivery]\nqueued-bytes-max = 2097152\nqueued-bytes-per-target-max = 4194304\n",
    )
    .expect_err("reject unreachable per-target byte quota");

    assert_eq!(
        detail(&error, "field").and_then(serde_json::Value::as_str),
        Some("delivery.queued-bytes-per-target-max"),
    );
    assert_eq!(
        detail(&error, "global_field").and_then(serde_json::Value::as_str),
        Some("delivery.queued-bytes-max"),
    );
}

/// Per-target quota equal to the relay-global quota is reachable and therefore
/// permitted; only a strictly larger one is the mistake.
#[test]
fn accepts_per_target_quota_equal_to_global() {
    let delivery = load_delivery(
        "[delivery]\nqueued-envelopes-max = 100\nqueued-envelopes-per-target-max = 100\n",
    )
    .expect("accept per-target quota equal to global");

    assert_eq!(delivery.queued_envelopes_per_target_max, 100);
}

#[test]
fn rejects_unknown_delivery_field() {
    let error = load_delivery("[delivery]\nreadiness-timeout-ms = 1000\n")
        .expect_err("reject unknown delivery key");

    assert_eq!(error.code, "validation_invalid_arguments");
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
