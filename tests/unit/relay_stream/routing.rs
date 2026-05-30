//! Routing-namespace resolution tests for the request-envelope `namespace`
//! selector (rename-request-routing-field tasks 1.1-1.5).
//!
//! These exercise `resolve_effective_bundle` through `serve_connection`: the
//! reserved relay-wide specifiers `EXTERNAL`/`RELAY` are rejected, the
//! not-yet-routable `GLOBAL` specifier returns its distinct stub error, and a
//! relay-wide principal that omits the namespace is rejected. Bundle-name
//! routing (the unchanged path) stays covered by the broader request tests in
//! the parent harness, which now send their selector under `namespace`.

use super::*;

/// Connects as a bundle-bound `alpha` session, sends one `list` request whose
/// frame carries the given routing `namespace`, and returns the response frame.
fn bundle_session_list_with_namespace(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    namespace: &str,
) -> Value {
    let (mut client, join) = spawn_relay_connection(configuration_root, bundle_paths);
    let mut reader = BufReader::new(client.try_clone().expect("clone stream"));
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": format!("alpha@{bundle_name}"),
            "identity_token": "socket-trust",
        }),
    );
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "req-1",
            "namespace": namespace,
            "request": {"operation": "list", "sender_session": "alpha"},
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown client stream");
    join.join().expect("join relay thread");
    response
}

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
fn request_namespace_global_routing_is_unavailable() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_namespace_global";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    // An explicit `GLOBAL` selector takes precedence over the connection's bundle
    // binding; it is intended to be routable but the relay-wide delivery path is
    // not yet built, so it returns the distinct stub error rather than routing or
    // a misleading catalog miss.
    let response = bundle_session_list_with_namespace(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        "GLOBAL",
    );

    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_namespace_routing_unavailable"
    );
    assert_eq!(
        response["response"]["error"]["details"]["namespace"],
        "GLOBAL"
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

    // A relay-wide principal carries no connection bundle binding, so omitting the
    // routing namespace leaves nothing to route against.
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "req-1",
            "request": {"operation": "list", "sender_session": "alpha"},
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
