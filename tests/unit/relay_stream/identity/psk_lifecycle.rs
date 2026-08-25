//! `change psk` rotation, revocation of the credential-holding session, and
//! fan-out of `identity.revoked` to trusted hosts.

use std::io::BufReader;

use agentmux::runtime::paths::{BundleRuntimePaths, session_identity_psk_path};
use serde_json::json;
use tempfile::TempDir;

use super::*;

// 1.20 `change psk` updates the store; the new PSK is accepted at Hello and the
// old PSK is rejected.
#[test]
fn change_psk_rotates_credential() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_change_psk";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let original_psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );

    let rotation = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "change_psk", "principal_id": principal_id}),
    );
    assert_eq!(
        rotation["response"]["kind"], "change_psk",
        "change psk rejected: {rotation:?}"
    );
    let rotated_psk = rotation["response"]["psk"]
        .as_str()
        .expect("psk in change psk response")
        .to_string();
    assert_ne!(rotated_psk, original_psk, "rotation must mint a new psk");

    let accepted = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        &principal_id,
        &rotated_psk,
        true,
    );
    assert_eq!(accepted["frame"], "hello_ack", "new psk: {accepted:?}");
    assert_eq!(accepted["principal_id"], principal_id);

    let rejected = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        &principal_id,
        &original_psk,
        false,
    );
    assert_eq!(rejected["frame"], "response", "old psk: {rejected:?}");
    assert_eq!(rejected["response"]["kind"], "error");
    assert_eq!(
        rejected["response"]["error"]["code"],
        "validation_unrecognized_credential"
    );
}

// Layering must not reach credential administration. `--write-config` names the
// operator's intent — write the credential down rather than return it — not the
// tree it lands in: a session pre-shared key belongs under the state root.
// Giving it layer semantics would put a secret into a tree that is shared,
// layered, and plausibly committed, so this pins the destination under a
// multi-layer list rather than leaving it to the single-layer case the other
// tests exercise.
#[test]
fn change_psk_config_writes_under_the_state_root_not_a_configuration_layer() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_change_layered";
    let base = write_identity_configuration(&temporary, bundle_name);
    let override_layer = temporary.path().join("override");
    std::fs::create_dir_all(&override_layer).expect("create override layer");
    let configuration_roots = ConfigurationRoots::from_elements([
        override_layer.clone(),
        base.base_layer().to_path_buf(),
    ])
    .expect("two-layer configuration list");
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let original_psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );
    // Snapshot after registration so the comparison isolates the rotation.
    let layers_before = layer_file_inventory(&configuration_roots);

    let rotation = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({
            "operation": "change_psk",
            "principal_id": principal_id,
            "destination": {"kind": "config"},
        }),
    );
    assert_eq!(
        rotation["response"]["kind"], "change_psk",
        "change psk config rejected: {rotation:?}"
    );

    // The positive half, and the one that carries the proof: the credential
    // landed under the state root. An implementation that resolved the
    // destination through a configuration layer would fail here, not merely on
    // the inventory comparison below.
    let canonical = session_identity_psk_path(&state_root, bundle_name, "alpha");
    assert_eq!(
        rotation["response"]["written_path"],
        canonical.to_string_lossy().as_ref(),
        "a layered configuration list must not move the credential destination"
    );
    let rotated_psk = std::fs::read_to_string(&canonical).expect("read rotated credential");
    assert_ne!(rotated_psk, original_psk, "rotation must mint a new psk");

    assert_eq!(
        layer_file_inventory(&configuration_roots),
        layers_before,
        "credential administration must write nothing into any configuration layer"
    );
}

/// Every file under every configuration layer, for before/after comparison.
///
/// A whole-tree inventory rather than a search for `.psk` files: the property is
/// that credential administration writes nothing into a configuration layer at
/// all, and a check keyed to one filename would miss a credential written under
/// another.
fn layer_file_inventory(roots: &ConfigurationRoots) -> Vec<PathBuf> {
    fn collect(directory: &std::path::Path, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(&path, found);
            } else {
                found.push(path);
            }
        }
    }

    let mut found = Vec::new();
    for layer in roots.layers() {
        collect(layer, &mut found);
    }
    found.sort();
    found
}

// `change psk` with the `config` destination writes the rotated PSK to the
// session's relay-owned canonical `identity.psk` (mode 0600), omits it from the
// response, and the rotated credential authenticates while the old one is
// rejected.
#[test]
fn change_psk_config_writes_session_identity_and_omits_psk() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_change_config";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let original_psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );

    let rotation = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({
            "operation": "change_psk",
            "principal_id": principal_id,
            "destination": {"kind": "config"},
        }),
    );
    assert_eq!(
        rotation["response"]["kind"], "change_psk",
        "change psk config rejected: {rotation:?}"
    );

    let canonical = session_identity_psk_path(&state_root, bundle_name, "alpha");
    assert_eq!(
        rotation["response"]["written_path"],
        canonical.to_string_lossy().as_ref(),
        "response must report the relay-owned canonical path"
    );
    assert!(
        rotation["response"]["psk"].is_null(),
        "psk must be omitted when written to config"
    );

    let mode = std::fs::metadata(&canonical)
        .expect("stat rotated config credential")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "config credential must be owner-only (0600)");
    let rotated_psk = std::fs::read_to_string(&canonical).expect("read rotated config credential");
    assert!(
        !rotated_psk.is_empty(),
        "rotated credential must be non-empty"
    );
    assert_ne!(rotated_psk, original_psk, "rotation must mint a new psk");

    let accepted = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        &principal_id,
        &rotated_psk,
        true,
    );
    assert_eq!(
        accepted["frame"], "hello_ack",
        "rotated config credential must authenticate: {accepted:?}"
    );

    let rejected = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        &principal_id,
        &original_psk,
        false,
    );
    assert_eq!(
        rejected["response"]["error"]["code"], "validation_unrecognized_credential",
        "old credential must be rejected after rotation: {rejected:?}"
    );
}

// `change psk` with `config` is rejected for a non-session (peer relay)
// principal whose credential location is not relay-owned, and the rejected
// attempt neither rotates the stored hash nor revokes: the original credential
// still authenticates afterward.
#[test]
fn change_psk_config_rejected_for_relay_principal_leaves_credential_unrotated() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_change_config_relay";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = "peer1@RELAY";

    let original_psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        principal_id,
        Some(bundle_name),
    );

    let rejected = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({
            "operation": "change_psk",
            "principal_id": principal_id,
            "destination": {"kind": "config"},
        }),
    );
    assert_eq!(
        rejected["response"]["kind"], "error",
        "config on a relay principal must reject: {rejected:?}"
    );
    assert_eq!(
        rejected["response"]["error"]["code"],
        "validation_config_destination_unsupported"
    );

    // The rejected rotation must not have replaced the stored hash: the original
    // credential still authenticates.
    let accepted = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        principal_id,
        &original_psk,
        true,
    );
    assert_eq!(
        accepted["frame"], "hello_ack",
        "original credential must survive a rejected rotation: {accepted:?}"
    );
    assert_eq!(accepted["principal_id"], principal_id);
}

// `change psk --output` whose publishing rename fails after the store commit
// (the target path is an existing directory) rolls the store back to the prior
// record: the original credential still authenticates and no revocation fires
// (the revoke step is unreachable past the failed commit).
#[test]
fn change_psk_output_finalization_failure_leaves_credential_unrotated() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_change_finalization";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let original_psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );

    let output_path = temporary.path().join("occupied.psk");
    std::fs::create_dir_all(&output_path).expect("create directory at output path");
    let response = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({
            "operation": "change_psk",
            "principal_id": principal_id,
            "destination": {"kind": "path", "path": output_path.to_string_lossy()},
        }),
    );
    assert_eq!(
        response["response"]["kind"], "error",
        "finalization failure must surface an error: {response:?}"
    );

    // Rollback restored the prior record: the original credential still
    // authenticates, so the rotation did not take effect.
    let accepted = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        &principal_id,
        &original_psk,
        true,
    );
    assert_eq!(
        accepted["frame"], "hello_ack",
        "original credential must survive a failed rotation: {accepted:?}"
    );
    assert_eq!(accepted["principal_id"], principal_id);
}

// 2.11 `change psk` on a principal with a live, store-backed session ->
// the session receives a `runtime_identity_revoked` error frame and its
// connection is then closed.
#[test]
fn change_psk_revokes_live_session_holding_old_credential() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_revoke_live";
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

    // Bring up a live, store-backed session and keep its connection open.
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

    // Rotate the credential from the operator connection; alpha holds the old
    // credential and must be revoked. `operator_request` only returns after the
    // rotation handler has run, by which point the revocation frame is queued.
    let rotation = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "change_psk", "principal_id": principal_id}),
    );
    assert_eq!(
        rotation["response"]["kind"], "change_psk",
        "change psk rejected: {rotation:?}"
    );

    // The live session observes the revocation frame ahead of EOF.
    let revoked = read_json_skipping_hello_ack(&mut alpha_reader);
    assert_eq!(revoked["frame"], "response", "revoked frame: {revoked:?}");
    assert_eq!(revoked["response"]["kind"], "error", "{revoked:?}");
    assert_eq!(
        revoked["response"]["error"]["code"], "runtime_identity_revoked",
        "revoked frame: {revoked:?}"
    );
    assert_eq!(
        revoked["response"]["error"]["details"]["principal_id"], principal_id,
        "revoked frame must name the rotated principal: {revoked:?}"
    );

    // The connection is then closed: the next read observes EOF.
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

// A principal rotating its *own* credential must receive the rotated PSK. The
// revocation sweep matches on `authenticated_identity`, which for a self-rotation
// is the requesting connection: tearing it down discards the response carrying
// the only copy of the new secret, leaving the principal with no credential
// matching the hash the relay has already committed.
//
// The requester must be provisioned with a real PSK. A socket-trust Hello records
// no `authenticated_identity` and is never matched by the sweep, so a test
// written against the harness default cannot reach this path at all and would
// pass against unfixed code.
#[test]
fn change_psk_on_the_requesters_own_identity_returns_the_rotated_credential() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_self_rotate";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let operator = global_user_id(bundle_name);

    let operator_psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &operator,
        None,
    );

    let (mut client, join) = spawn_relay_connection(&configuration_roots, &bundle_paths);
    let mut reader = BufReader::new(client.try_clone().expect("clone stream"));
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": operator,
            "identity_token": operator_psk,
        }),
    );
    let ack = read_json(&mut reader);
    assert_eq!(
        ack["frame"], "hello_ack",
        "operator hello not acked: {ack:?}"
    );

    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "self-rotate-1",
            "request": {"operation": "change_psk", "principal_id": operator},
        }),
    );

    // The first frame decides it. Unfixed, the requester is swept and the only
    // frame it ever sees is the `runtime_identity_revoked` error ahead of EOF.
    let rotation = read_json_skipping_hello_ack(&mut reader);
    assert_eq!(
        rotation["response"]["kind"], "change_psk",
        "self-rotation must answer the requester rather than revoke it: {rotation:?}"
    );
    let rotated_psk = rotation["response"]["psk"]
        .as_str()
        .expect("psk in self-rotation response");
    assert_ne!(
        rotated_psk, operator_psk,
        "self-rotation must mint a new psk"
    );

    shutdown_stream(&client, "shutdown operator stream");
    join.join().expect("join operator relay thread");
}

// Excluding the requester from the teardown must not also exclude it from the
// trusted-host fan-out. The two mechanisms are distinct: a watching host holds a
// cached view of a credential that changed, and that view is stale no matter who
// initiated the rotation. This guards the regression where the fan-out is moved
// inside the self-rotation exclusion.
#[test]
fn self_rotation_still_fans_out_identity_revoked_to_trusted_hosts() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_self_rotate_fanout";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let operator = global_user_id(bundle_name);
    let app_principal_id = format!("engine_{bundle_name}@EXTERNAL");

    let operator_psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &operator,
        None,
    );
    // Scoped to the reserved namespace the operator lives in, so the fan-out's
    // scope check covers the rotated principal.
    let app_psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &app_principal_id,
        Some("GLOBAL"),
    );

    let (mut app_client, app_join) = spawn_relay_connection(&configuration_roots, &bundle_paths);
    let mut app_reader = BufReader::new(app_client.try_clone().expect("clone app stream"));
    send_json(
        &mut app_client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": app_principal_id.as_str(),
            "identity_token": app_psk,
        }),
    );
    assert_eq!(
        read_json(&mut app_reader)["frame"],
        "hello_ack",
        "app hello not acked"
    );

    let (mut client, join) = spawn_relay_connection(&configuration_roots, &bundle_paths);
    let mut reader = BufReader::new(client.try_clone().expect("clone stream"));
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": operator,
            "identity_token": operator_psk,
        }),
    );
    assert_eq!(
        read_json(&mut reader)["frame"],
        "hello_ack",
        "operator hello not acked"
    );
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "self-rotate-fanout-1",
            "request": {"operation": "change_psk", "principal_id": operator},
        }),
    );
    let rotation = read_json_skipping_hello_ack(&mut reader);
    assert_eq!(
        rotation["response"]["kind"], "change_psk",
        "self-rotation rejected: {rotation:?}"
    );

    let event = read_until_event_type(&mut app_reader, "identity.revoked");
    assert_eq!(
        event["event"]["payload"]["principal_id"], operator,
        "revoked event must name the self-rotated principal: {event:?}"
    );
    assert!(
        event["event"]["payload"]["revoked_at"].is_string(),
        "revoked event must carry a revoked_at timestamp: {event:?}"
    );

    shutdown_stream(&client, "shutdown operator stream");
    join.join().expect("join operator relay thread");
    shutdown_stream(&app_client, "shutdown app stream");
    app_join.join().expect("join app relay thread");
}

// 2.7 change psk on an in-scope principal fans out an identity.revoked event to
// every connected trusted host whose scope covers the revoked principal.
#[test]
fn change_psk_fans_out_identity_revoked_to_trusted_hosts() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_revoked_fanout";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let target_principal_id = format!("alpha@{bundle_name}");
    let app_principal_id = format!("engine_{bundle_name}@EXTERNAL");

    // An in-scope session principal to revoke, plus a trusted host scoped to the
    // whole bundle.
    register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &target_principal_id,
        None,
    );
    let app_psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &app_principal_id,
        Some(bundle_name),
    );

    // Connect the trusted host and keep its connection open; it receives the
    // connect-time identity.snapshot first, which the revoked-event read skips.
    let (mut app_client, app_join) = spawn_relay_connection(&configuration_roots, &bundle_paths);
    let mut app_reader = BufReader::new(app_client.try_clone().expect("clone app stream"));
    send_json(
        &mut app_client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": app_principal_id.as_str(),
            "identity_token": app_psk,
        }),
    );
    assert_eq!(
        read_json(&mut app_reader)["frame"],
        "hello_ack",
        "app hello not acked"
    );

    // Rotate the target's credential from the operator connection.
    // `operator_request` returns only after the rotation handler has run, by
    // which point the fan-out event is queued to the host's writer.
    let rotation = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "change_psk", "principal_id": target_principal_id}),
    );
    assert_eq!(
        rotation["response"]["kind"], "change_psk",
        "change psk rejected: {rotation:?}"
    );

    let event = read_until_event_type(&mut app_reader, "identity.revoked");
    shutdown_stream(&app_client, "shutdown app stream");
    app_join.join().expect("join app relay thread");

    assert_eq!(
        event["event"]["payload"]["principal_id"], target_principal_id,
        "revoked event must name the rotated principal: {event:?}"
    );
    assert!(
        event["event"]["payload"]["revoked_at"].is_string(),
        "revoked event must carry a revoked_at timestamp: {event:?}"
    );
    assert_eq!(
        event["event"]["target_session"],
        app_principal_id.as_str(),
        "revoked event must be routed to the watching host"
    );
}
