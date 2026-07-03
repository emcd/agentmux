//! `change psk` rotation, revocation of the credential-holding session, and
//! fan-out of `identity.revoked` to trusted hosts.
//!
//! Tasks 1.20, 2.7, 2.11.

use std::io::BufReader;

use agentmux::runtime::paths::BundleRuntimePaths;
use serde_json::json;
use tempfile::TempDir;

use super::*;

// 1.20 `change psk` updates the store; the new PSK is accepted at Hello and the
// old PSK is rejected.
#[test]
fn change_psk_rotates_credential() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_change_psk";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let original_psk = register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );

    let rotation = operator_request(
        &configuration_root,
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
        &configuration_root,
        &bundle_paths,
        &principal_id,
        &rotated_psk,
        true,
    );
    assert_eq!(accepted["frame"], "hello_ack", "new psk: {accepted:?}");
    assert_eq!(accepted["principal_id"], principal_id);

    let rejected = hello_first_frame(
        &configuration_root,
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

// 2.11 `change psk` on a principal with a live, store-backed session ->
// the session receives a `runtime_identity_revoked` error frame and its
// connection is then closed.
#[test]
fn change_psk_revokes_live_session_holding_old_credential() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_revoke_live";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let psk = register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );

    // Bring up a live, store-backed session and keep its connection open.
    let (mut alpha_client, alpha_join) = spawn_relay_connection(&configuration_root, &bundle_paths);
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
        &configuration_root,
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

// 2.7 change psk on an in-scope principal fans out an identity.revoked event to
// every connected trusted host whose scope covers the revoked principal.
#[test]
fn change_psk_fans_out_identity_revoked_to_trusted_hosts() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_revoked_fanout";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let target_principal_id = format!("alpha@{bundle_name}");
    let app_principal_id = format!("engine_{bundle_name}@EXTERNAL");

    // An in-scope session principal to revoke, plus a trusted host scoped to the
    // whole bundle.
    register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &target_principal_id,
        None,
    );
    let app_psk = register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &app_principal_id,
        Some(bundle_name),
    );

    // Connect the trusted host and keep its connection open; it receives the
    // connect-time identity.snapshot first, which the revoked-event read skips.
    let (mut app_client, app_join) = spawn_relay_connection(&configuration_root, &bundle_paths);
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
        &configuration_root,
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
