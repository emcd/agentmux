//! Rejection of the reserved `EXTERNAL`/`RELAY` namespace selectors on the
//! `List` operation, and the `validation_missing_routing_namespace` rejection
//! for a relay-wide `List` that omits the namespace.

use std::io::BufReader;

use tempfile::TempDir;

use super::*;

#[test]
fn request_namespace_external_is_rejected_as_reserved() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_namespace_external";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response = bundle_session_list_with_namespace(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        "EXTERNAL",
    );

    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_unsupported_namespace"
    );
    assert_eq!(
        response["response"]["error"]["details"]["namespace"],
        "EXTERNAL"
    );
}

#[test]
fn request_namespace_relay_is_rejected_as_reserved() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_namespace_relay";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response = bundle_session_list_with_namespace(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        "RELAY",
    );

    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_unsupported_namespace"
    );
    assert_eq!(
        response["response"]["error"]["details"]["namespace"],
        "RELAY"
    );
}

#[test]
fn relay_wide_principal_without_namespace_is_rejected() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_namespace_missing";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let (mut client, join) = spawn_relay_connection(&configuration_root, &bundle_paths);
    let mut reader = BufReader::new(client.try_clone().expect("clone stream"));
    let operator = global_user_id(bundle_name);
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": operator,
            "identity_token": "socket-trust",
        }),
    );
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");

    // A relay-wide principal carries no connection bundle binding, so a `List`
    // that omits the routing namespace leaves nothing to route against.
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "req-1",
            "request": {"operation": "list", "requester_session": "alpha"},
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown operator stream");
    join.join().expect("join relay thread");

    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_missing_routing_namespace"
    );
}
