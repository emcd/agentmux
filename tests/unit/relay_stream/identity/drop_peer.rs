//! `drop peer`: store deletion, revocation of the dropped principal's live
//! session, the self-drop refusal and its ordering against authorization, and
//! the session-only credential-path report.

use std::io::BufReader;

use agentmux::runtime::paths::{BundleRuntimePaths, session_identity_psk_path};
use serde_json::json;
use tempfile::TempDir;

use super::*;

/// Writes an operator configuration granting `new.peer` and `change.psk` at
/// `all` but **not** `drop.peer`, so an otherwise fully-authorized operator is
/// ungranted for exactly one action.
fn write_configuration_without_drop_grant(
    temporary: &TempDir,
    bundle_name: &str,
) -> ConfigurationRoots {
    let configuration_roots = write_bundle_configuration(temporary, bundle_name);
    std::fs::write(
        configuration_roots.base_layer().join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "home"
look = "self"
send = "home"

[[policies]]
id = "operator"

[policies.controls]
find = "self"
list = "home"
look = "home"
send = "home"

[policies.controls.new]
peer = "all"

[policies.controls.change]
psk = "all"
"#,
    )
    .expect("write policies configuration without drop grant");
    write_tui_configuration(&configuration_roots, "operator", bundle_name);
    configuration_roots
}

// Dropping a registered principal deletes its store record: the credential that
// authenticated a moment ago no longer does.
#[test]
fn drop_peer_deletes_the_record_and_its_credential_stops_authenticating() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_drop_deletes";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );
    let accepted = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        &principal_id,
        &psk,
        true,
    );
    assert_eq!(
        accepted["frame"], "hello_ack",
        "credential must authenticate before the drop: {accepted:?}"
    );

    let dropped = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "drop_peer", "principal_id": principal_id}),
    );
    assert_eq!(
        dropped["response"]["kind"], "drop_peer",
        "drop peer rejected: {dropped:?}"
    );
    assert_eq!(dropped["response"]["principal_id"], principal_id);
    assert_eq!(dropped["response"]["principal_type"], "session");

    let rejected = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        &principal_id,
        &psk,
        true,
    );
    assert_eq!(
        rejected["response"]["error"]["code"], "validation_unrecognized_credential",
        "the dropped credential must stop authenticating: {rejected:?}"
    );
}

// A record that no longer exists must not keep authenticating a live
// connection, so the dropped principal's session is torn down with the typed
// revocation frame ahead of EOF.
#[test]
fn drop_peer_revokes_the_live_session_it_deletes() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_drop_revokes";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );

    let (mut alpha_client, alpha_join) =
        spawn_relay_connection(&configuration_roots, &bundle_paths);
    let mut alpha_reader = BufReader::new(alpha_client.try_clone().expect("clone alpha stream"));
    send_json(
        &mut alpha_client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": principal_id,
            "identity_token": psk,
        }),
    );
    assert_eq!(
        read_json(&mut alpha_reader)["frame"],
        "hello_ack",
        "alpha hello not acked"
    );

    let dropped = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "drop_peer", "principal_id": principal_id}),
    );
    assert_eq!(
        dropped["response"]["kind"], "drop_peer",
        "drop peer rejected: {dropped:?}"
    );

    let revoked = read_json_skipping_hello_ack(&mut alpha_reader);
    assert_eq!(revoked["frame"], "response", "revoked frame: {revoked:?}");
    assert_eq!(
        revoked["response"]["error"]["code"], "runtime_identity_revoked",
        "revoked frame: {revoked:?}"
    );
    assert_eq!(
        revoked["response"]["error"]["details"]["principal_id"], principal_id,
        "revoked frame must name the dropped principal: {revoked:?}"
    );

    use std::io::BufRead;
    let mut trailing = String::new();
    let read = alpha_reader
        .read_line(&mut trailing)
        .expect("read after revocation");
    assert_eq!(
        read, 0,
        "connection must close after revocation; got {trailing:?}"
    );

    shutdown_stream(&alpha_client, "shutdown alpha stream");
    alpha_join.join().expect("join alpha relay thread");
}

// A self-drop would revoke the connection carrying its own response, leaving
// the operator unable to tell a committed drop from a failed one.
#[test]
fn drop_peer_refuses_to_drop_the_requester_itself() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_drop_self";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let operator_id = global_user_id(bundle_name);

    let refused = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "drop_peer", "principal_id": operator_id}),
    );
    assert_eq!(
        refused["response"]["kind"], "error",
        "self-drop must be refused: {refused:?}"
    );
    assert_eq!(
        refused["response"]["error"]["code"], "validation_self_drop_forbidden",
        "self-drop refusal code: {refused:?}"
    );
}

// The self-drop check reads nothing privileged, so it answers ahead of the
// authorization gate: a caller dropping their own id learns that rather than
// learning their grant is missing.
#[test]
fn self_drop_is_refused_ahead_of_an_authorization_denial() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_drop_self_ungranted";
    let configuration_roots = write_configuration_without_drop_grant(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let operator_id = global_user_id(bundle_name);

    let refused = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "drop_peer", "principal_id": operator_id}),
    );
    assert_eq!(
        refused["response"]["error"]["code"], "validation_self_drop_forbidden",
        "a locally decidable validation precedes the authorization denial: {refused:?}"
    );
}

// The unknown-principal check is store-backed, so it stays behind the gate:
// answering it for an ungranted caller would disclose whether an arbitrary
// principal exists.
#[test]
fn an_ungranted_caller_is_denied_rather_than_told_the_principal_is_absent() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_drop_nondisclosure";
    let configuration_roots = write_configuration_without_drop_grant(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let denied = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "drop_peer", "principal_id": format!("ghost@{bundle_name}")}),
    );
    assert_eq!(
        denied["response"]["error"]["code"], "authorization_forbidden",
        "an ungranted caller must not learn whether the principal exists: {denied:?}"
    );
}

// A `new.peer`/`change.psk` grant confers no ability to drop: the control is
// distinct and a policy file without it fails closed.
#[test]
fn new_and_change_grants_do_not_confer_drop() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_drop_distinct_grant";
    let configuration_roots = write_configuration_without_drop_grant(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    // The same operator can mint, which pins that only the drop control is
    // missing rather than the whole operator preset being unauthorized.
    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );

    let denied = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "drop_peer", "principal_id": principal_id}),
    );
    assert_eq!(
        denied["response"]["error"]["code"], "authorization_forbidden",
        "drop must require its own grant: {denied:?}"
    );

    // And the principal survives the denial.
    let accepted = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        &principal_id,
        &psk,
        true,
    );
    assert_eq!(
        accepted["frame"], "hello_ack",
        "a denied drop must leave the principal registered: {accepted:?}"
    );
}

// Dropping an unregistered principal is not success.
#[test]
fn drop_peer_rejects_an_unregistered_principal() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_drop_unknown";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let rejected = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "drop_peer", "principal_id": format!("ghost@{bundle_name}")}),
    );
    assert_eq!(
        rejected["response"]["error"]["code"], "validation_unknown_principal",
        "dropping an absent principal must not report success: {rejected:?}"
    );
}

// A peer relay's credential lives under the *connecting* relay's state root, so
// the dropping relay reports no path rather than one derived from its own.
#[test]
fn drop_peer_reports_no_credential_path_for_a_peer_relay() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_drop_relay_path";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = "rnd-main@RELAY";

    register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        principal_id,
        Some(bundle_name),
    );

    let dropped = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "drop_peer", "principal_id": principal_id}),
    );
    assert_eq!(
        dropped["response"]["kind"], "drop_peer",
        "drop peer rejected: {dropped:?}"
    );
    assert_eq!(dropped["response"]["principal_type"], "relay");
    assert!(
        dropped["response"]["credential_path"].is_null(),
        "no path may be reported for a credential this relay cannot see: {dropped:?}"
    );
}

// A session principal is the one type whose credential location the relay owns,
// so its path is reported for the operator to clean up.
#[test]
fn drop_peer_reports_the_relay_owned_credential_path_for_a_session() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_drop_session_path";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );

    let dropped = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "drop_peer", "principal_id": principal_id}),
    );
    let reported = dropped["response"]["credential_path"]
        .as_str()
        .unwrap_or_else(|| panic!("credential_path for a session principal: {dropped:?}"));
    let expected = session_identity_psk_path(&state_root, bundle_name, "alpha");
    assert_eq!(
        reported,
        expected.display().to_string(),
        "the reported path must be the relay-owned canonical location"
    );
}

// The credential file is reported, never deleted: once the record is gone the
// file authenticates nothing, and the relay cannot know where the operator
// distributed it.
#[test]
fn drop_peer_leaves_the_credential_file_on_disk() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_drop_keeps_file";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");
    let credential_path = state_root.join("dropped-alpha.psk");
    std::fs::create_dir_all(&state_root).expect("create state root for the credential destination");

    let registration = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({
            "operation": "new_peer",
            "principal_id": principal_id,
            "destination": {"kind": "path", "path": credential_path.to_string_lossy()},
        }),
    );
    assert_eq!(
        registration["response"]["kind"], "new_peer",
        "new peer rejected: {registration:?}"
    );
    assert!(
        credential_path.exists(),
        "credential file must exist before the drop"
    );

    let dropped = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "drop_peer", "principal_id": principal_id}),
    );
    assert_eq!(
        dropped["response"]["kind"], "drop_peer",
        "drop peer rejected: {dropped:?}"
    );
    assert!(
        credential_path.exists(),
        "the credential file must be left in place for the operator to remove"
    );
}
