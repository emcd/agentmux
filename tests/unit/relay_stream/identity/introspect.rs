//! `IdentityIntrospect` dispatch gate and the connect-time `identity.snapshot`
//! for application principals.

use agentmux::configuration::ConfigurationRoots;
use std::io::BufReader;

use agentmux::runtime::paths::BundleRuntimePaths;
use tempfile::TempDir;

use super::*;

/// Connects with `principal_id` + `identity_token`, issues one
/// `IdentityIntrospect` for `target_session` (a qualified principal id), and
/// returns the response frame. The connection is closed before returning.
fn introspect_request(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    principal_id: &str,
    identity_token: &str,
    target_session: &str,
) -> Value {
    let (mut client, join) = spawn_relay_connection(configuration_roots, bundle_paths);
    let mut reader = BufReader::new(client.try_clone().expect("clone introspect stream"));
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": principal_id,
            "identity_token": identity_token,
        }),
    );
    assert_eq!(
        read_json(&mut reader)["frame"],
        "hello_ack",
        "introspect client hello not acked"
    );
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "introspect-1",
            "request": {
                "operation": "identity_introspect",
                "target_session": target_session,
            },
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown introspect stream");
    join.join().expect("join introspect relay thread");
    response
}

// An application principal introspects an in-scope session and receives the
// target's principal_id, expires_at, and verified: true.
#[test]
fn application_principal_introspects_active_session() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_introspect_ok";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let target_principal_id = format!("alpha@{bundle_name}");
    // Unique application id per test: the relay-wide registry keys on the
    // `principal_id`, so a shared `@EXTERNAL` id collides across parallel runs.
    let app_principal_id = format!("engine_{bundle_name}@EXTERNAL");

    // Register the introspection target as a session principal in the store.
    register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &target_principal_id,
        None,
    );
    // Register the application principal scoped to the whole bundle (bare-bundle
    // scope covers every session in that namespace).
    let app_psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &app_principal_id,
        Some(bundle_name),
    );

    let response = introspect_request(
        &configuration_roots,
        &bundle_paths,
        &app_principal_id,
        &app_psk,
        target_principal_id.as_str(),
    );

    assert_eq!(
        response["response"]["kind"], "identity_introspect",
        "expected introspect response: {response:?}"
    );
    assert_eq!(
        response["response"]["principal_id"], target_principal_id,
        "introspection must surface the target's stable principal_id"
    );
    assert_eq!(
        response["response"]["verified"], true,
        "an unexpired principal must verify"
    );
    // The target was registered without an expiry, so `expires_at` is absent
    // rather than a placeholder timestamp (it is an optional ISO 8601 field).
    assert!(
        response["response"]["expires_at"].is_null(),
        "expires_at must be absent for a principal that never expires: {response:?}"
    );
    assert!(
        response["response"]["on_behalf_of"].is_null(),
        "on_behalf_of must be absent until its setting mechanism lands"
    );
}

// 2.10 A session principal issuing IdentityIntrospect is denied: it carries no
// introspection rights, so the gate returns an authorization denial and no
// identity data.
#[test]
fn session_principal_introspect_is_denied() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_introspect_denied";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    // A socket-trust session principal: accepted at Hello (enforcement off) but
    // granted no introspection rights.
    let response = introspect_request(
        &configuration_roots,
        &bundle_paths,
        &format!("alpha@{bundle_name}"),
        "socket-trust",
        &format!("alpha@{bundle_name}"),
    );

    assert_eq!(
        response["response"]["kind"], "error",
        "session introspect must be rejected: {response:?}"
    );
    assert_eq!(
        response["response"]["error"]["code"], "authorization_forbidden",
        "session introspect must be an authorization denial: {response:?}"
    );
    assert!(
        response["response"]["principal_id"].is_null(),
        "a denied introspection must not leak identity data"
    );
}

// A bare (unqualified) target_session is a field-format error, rejected before
// the introspect_rights gate: even a connection with no rights sees
// validation_invalid_params rather than an authorization denial.
#[test]
fn introspect_rejects_bare_target_session_before_authorization() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_introspect_bare";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response = introspect_request(
        &configuration_roots,
        &bundle_paths,
        &format!("alpha@{bundle_name}"),
        "socket-trust",
        "alpha",
    );

    assert_eq!(
        response["response"]["kind"], "error",
        "bare introspect target must be rejected: {response:?}"
    );
    assert_eq!(
        response["response"]["error"]["code"], "validation_invalid_params",
        "bare target is a field-format error, not an authorization denial: {response:?}"
    );
    assert_eq!(
        response["response"]["error"]["details"]["field"], "target_session",
        "rejection details must cite the offending field: {response:?}"
    );
}

// 2.6 A trusted-host (application principal) connection receives an
// identity.snapshot event immediately after Hello, carrying the active
// principal records within its registered scope.
#[test]
fn application_principal_receives_identity_snapshot_on_connect() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_snapshot_connect";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let target_principal_id = format!("alpha@{bundle_name}");
    // Unique application id per test: the relay-wide registry keys on the
    // `principal_id`, so a shared `@EXTERNAL` id collides across parallel runs.
    let app_principal_id = format!("engine_{bundle_name}@EXTERNAL");

    // An in-scope session principal in the store, plus the application principal
    // scoped to the whole bundle (bare-bundle scope covers every session in it).
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

    let (mut client, join) = spawn_relay_connection(&configuration_roots, &bundle_paths);
    let mut reader = BufReader::new(client.try_clone().expect("clone app stream"));
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": app_principal_id.as_str(),
            "identity_token": app_psk,
        }),
    );
    assert_eq!(
        read_json(&mut reader)["frame"],
        "hello_ack",
        "app hello not acked"
    );

    // The snapshot is the very next frame after the ack (no choices snapshot
    // is emitted for a non-UI application principal).
    let frame = read_json(&mut reader);
    shutdown_stream(&client, "shutdown app stream");
    join.join().expect("join app relay thread");

    assert_eq!(
        frame["frame"], "event",
        "expected snapshot event: {frame:?}"
    );
    assert_eq!(
        frame["event"]["event_type"], "identity.snapshot",
        "{frame:?}"
    );
    assert_eq!(frame["event"]["target_session"], app_principal_id.as_str());
    let principals = frame["event"]["payload"]["principals"]
        .as_array()
        .expect("principals array in snapshot payload");
    let ids: Vec<&str> = principals
        .iter()
        .filter_map(|entry| entry["principal_id"].as_str())
        .collect();
    assert!(
        ids.contains(&target_principal_id.as_str()),
        "snapshot must carry the in-scope target: {frame:?}"
    );
    // The host's own @EXTERNAL record is outside the bundle scope and omitted.
    assert!(
        !ids.contains(&app_principal_id.as_str()),
        "out-of-scope principals must not appear in the snapshot: {frame:?}"
    );
    let target_entry = principals
        .iter()
        .find(|entry| entry["principal_id"] == json!(target_principal_id))
        .expect("target entry in snapshot");
    assert_eq!(
        target_entry["verified"], true,
        "an active principal is verified in the snapshot"
    );
    assert!(
        target_entry["expires_at"].is_null(),
        "a never-expiring principal omits expires_at: {frame:?}"
    );
}
