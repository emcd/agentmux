//! The peer-alias invariant: a `[[peers]]` alias names an identity this relay
//! issued.
//!
//! Binding the alias to the issued identity is what lets a relay name an
//! inbound peer from the principal it just authenticated, with nothing to look
//! up and nothing to keep in agreement. These exercise the load-time check that
//! holds the binding, over a hand-written principal store.

use std::path::{Path, PathBuf};

use agentmux::relay::{PeerConfiguration, RelayError, validate_peer_aliases};
use agentmux::runtime::paths::principal_store_path;
use tempfile::TempDir;

/// Writes a principal store holding one record per supplied
/// `(principal_id, principal_type)` pair, and returns the state root.
fn write_store(temporary: &TempDir, records: &[(&str, &str)]) -> PathBuf {
    let state_root = temporary.path().join("state");
    let store_path = principal_store_path(&state_root);
    std::fs::create_dir_all(store_path.parent().expect("store parent"))
        .expect("create identity directory");
    let principals: Vec<String> = records
        .iter()
        .enumerate()
        .map(|(index, (principal_id, principal_type))| {
            format!(
                "    {{\n      \"principal_id\": \"{principal_id}\",\n      \
                 \"principal_type\": \"{principal_type}\",\n      \
                 \"credential_hash\": \"{index:064x}\"\n    }}"
            )
        })
        .collect();
    std::fs::write(
        &store_path,
        format!(
            "{{\n  \"format_version\": 1,\n  \"principals\": [\n{}\n  ]\n}}",
            principals.join(",\n")
        ),
    )
    .expect("write principal store");
    state_root
}

/// A peer entry whose alias and `connect-as` deliberately differ, so no test
/// passes by the two happening to coincide.
fn peer(alias: &str) -> PeerConfiguration {
    PeerConfiguration {
        alias: alias.to_string(),
        address: "/run/agentmux/west.sock".to_string(),
        connect_as: "east".to_string(),
    }
}

fn alias_of(error: &RelayError) -> Option<String> {
    error.details.as_ref()?["value"]
        .as_str()
        .map(str::to_string)
}

#[test]
fn an_alias_naming_an_issued_relay_identity_is_accepted() {
    // The alias and `connect-as` differ here, which is the ordinary case rather
    // than an exotic one: they name opposite directions of the relationship and
    // each side chooses its own independently.
    let temporary = TempDir::new().expect("temporary");
    let state_root = write_store(&temporary, &[("west@RELAY", "relay")]);

    validate_peer_aliases(&[peer("west")], &state_root).expect("a registered alias is accepted");
}

#[test]
fn an_alias_naming_no_issued_identity_is_rejected() {
    // This is also the dial-only case. A peer this relay never hears from needs
    // no name, so its alias could in principle be free — but an absent record is
    // equally consistent with a typo, a stale alias after re-registration, or a
    // row copied between deployments, and nothing local separates them. Excusing
    // absence would accept all four, so none is excused.
    let temporary = TempDir::new().expect("temporary");
    let state_root = write_store(&temporary, &[("east@RELAY", "relay")]);

    let error = validate_peer_aliases(&[peer("west")], &state_root)
        .expect_err("an unregistered alias must be rejected");
    assert_eq!(error.code, "validation_peer_alias_unregistered");
    assert_eq!(
        alias_of(&error).as_deref(),
        Some("west"),
        "the failure names the offending alias, not the principal it looked for"
    );
}

#[test]
fn an_alias_naming_a_non_relay_principal_is_rejected() {
    // A registered name is not enough: the record must be a peer relay. A
    // session or user principal that happens to share the alias would otherwise
    // satisfy the check while naming something that can never connect as a peer.
    let temporary = TempDir::new().expect("temporary");
    let state_root = write_store(&temporary, &[("west@RELAY", "session")]);

    let error = validate_peer_aliases(&[peer("west")], &state_root)
        .expect_err("a non-relay principal must be rejected");
    assert_eq!(error.code, "validation_peer_alias_not_relay_principal");
    assert_eq!(alias_of(&error).as_deref(), Some("west"));
}

#[test]
fn the_offending_alias_is_named_when_an_earlier_peer_is_valid() {
    // Guards the loop rather than the predicate: a check that reported the first
    // entry regardless, or stopped at the first success, would pass every
    // single-peer test above.
    let temporary = TempDir::new().expect("temporary");
    let state_root = write_store(&temporary, &[("west@RELAY", "relay")]);

    let error = validate_peer_aliases(&[peer("west"), peer("north")], &state_root)
        .expect_err("a later unregistered alias must still be rejected");
    assert_eq!(
        alias_of(&error).as_deref(),
        Some("north"),
        "the second entry is the one that fails"
    );
}

#[test]
fn a_relay_with_no_peers_needs_no_principal_store() {
    // A relay that dials nobody must start on a state root that has never had a
    // principal registered, so the check cannot require the store to exist.
    let temporary = TempDir::new().expect("temporary");
    let state_root = temporary.path().join("empty-state");

    validate_peer_aliases(&[], &state_root).expect("no peers needs no store");
    assert!(
        !Path::new(&principal_store_path(&state_root)).exists(),
        "the check must not create a store as a side effect"
    );
}
