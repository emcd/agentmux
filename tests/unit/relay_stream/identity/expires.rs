//! Credential expiry teardown at Hello and the typed identity-lifecycle error
//! codes.

use std::io::{BufRead, BufReader};

use agentmux::runtime::paths::BundleRuntimePaths;
use serde_json::Value;
use tempfile::TempDir;

use super::*;

/// Rewrites the principal store so `principal_id`'s record carries an
/// `expires_at` in the past, expiring its credential without disturbing the
/// credential hash. The store is loaded fresh on the next Hello, so the patched
/// expiry takes effect on the very next connection.
fn expire_principal_in_store(bundle_paths: &BundleRuntimePaths, principal_id: &str) {
    let path = principal_store_file(bundle_paths);
    let raw = std::fs::read_to_string(&path).expect("read principal store");
    let mut store: Value = serde_json::from_str(&raw).expect("parse principal store");
    let record = store["principals"]
        .as_array_mut()
        .expect("principals array in store")
        .iter_mut()
        .find(|record| record["principal_id"] == json!(principal_id))
        .expect("record for principal in store");
    record["expires_at"] = json!("2000-01-01T00:00:00Z");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&store).expect("serialize patched store"),
    )
    .expect("rewrite principal store");
}

/// Sends one Hello over a fresh connection and returns the first server frame
/// together with whether the relay then closed the connection (EOF on the next
/// read). Used to assert that a rejected Hello delivers its typed error frame
/// before the connection is torn down.
fn hello_frame_and_eof(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    principal_id: &str,
    identity_token: &str,
) -> (Value, bool) {
    let (mut client, join) = spawn_relay_connection(configuration_root, bundle_paths);
    let mut reader = BufReader::new(client.try_clone().expect("clone hello stream"));
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": principal_id,
            "identity_token": identity_token,
        }),
    );
    let frame = read_json(&mut reader);
    let mut trailing = String::new();
    let read = reader
        .read_line(&mut trailing)
        .expect("read after hello rejection");
    shutdown_stream(&client, "shutdown hello stream");
    join.join().expect("join hello relay thread");
    (frame, read == 0)
}

// 2.12 A principal whose credential has expired is rejected at Hello with a
// `runtime_identity_expired` error frame, and the relay then closes the
// connection.
#[test]
fn expired_principal_receives_runtime_identity_expired_before_close() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_expired_teardown";
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
    // Backdate the registered credential's expiry; the PSK itself is unchanged.
    expire_principal_in_store(&bundle_paths, &principal_id);

    let (frame, closed) =
        hello_frame_and_eof(&configuration_root, &bundle_paths, &principal_id, &psk);

    assert_eq!(
        frame["frame"], "response",
        "expected error frame: {frame:?}"
    );
    assert_eq!(frame["response"]["kind"], "error", "{frame:?}");
    assert_eq!(
        frame["response"]["error"]["code"], "runtime_identity_expired",
        "an expired credential must be rejected with the distinct typed code: {frame:?}"
    );
    assert_eq!(
        frame["response"]["error"]["details"]["principal_id"], principal_id,
        "the expiry error must name the expired principal: {frame:?}"
    );
    assert!(
        closed,
        "the relay must close the connection after the expiry frame"
    );
}

// 2.13 The typed identity-lifecycle error codes (`runtime_identity_expired`,
// `runtime_identity_revoked`) are distinct from one another and from the
// transport-level `relay_unavailable`, so a client can tell an expired or
// revoked credential apart from a relay outage.
#[test]
fn typed_identity_error_codes_are_distinct_from_relay_unavailable() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_typed_codes";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    // Expiry path: an expired credential is rejected at Hello.
    let expired_id = format!("alpha@{bundle_name}");
    let expired_psk = register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &expired_id,
        None,
    );
    expire_principal_in_store(&bundle_paths, &expired_id);
    let (expired_frame, _) = hello_frame_and_eof(
        &configuration_root,
        &bundle_paths,
        &expired_id,
        &expired_psk,
    );
    let expired_code = expired_frame["response"]["error"]["code"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Revocation path: rotating a live session's credential tears it down.
    let revoked_id = format!("bravo@{bundle_name}");
    let revoked_psk = register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &revoked_id,
        None,
    );
    let (mut live_client, live_join) = spawn_relay_connection(&configuration_root, &bundle_paths);
    let mut live_reader = BufReader::new(live_client.try_clone().expect("clone live stream"));
    send_json(
        &mut live_client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": revoked_id,
            "identity_token": revoked_psk,
        }),
    );
    assert_eq!(
        read_json(&mut live_reader)["frame"],
        "hello_ack",
        "live hello not acked"
    );
    let rotation = operator_request(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        json!({"operation": "change_psk", "principal_id": revoked_id}),
    );
    assert_eq!(
        rotation["response"]["kind"], "change_psk",
        "change psk rejected: {rotation:?}"
    );
    let revoked_frame = read_json_skipping_hello_ack(&mut live_reader);
    let revoked_code = revoked_frame["response"]["error"]["code"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    shutdown_stream(&live_client, "shutdown live stream");
    live_join.join().expect("join live relay thread");

    assert_eq!(
        expired_code, "runtime_identity_expired",
        "expiry frame: {expired_frame:?}"
    );
    assert_eq!(
        revoked_code, "runtime_identity_revoked",
        "revoked frame: {revoked_frame:?}"
    );
    assert_ne!(
        expired_code, "relay_unavailable",
        "an expired credential must not surface as a transport outage"
    );
    assert_ne!(
        revoked_code, "relay_unavailable",
        "a revoked credential must not surface as a transport outage"
    );
    assert_ne!(
        expired_code, revoked_code,
        "expiry and revocation must carry distinguishable typed codes"
    );
}
