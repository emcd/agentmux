//! Origin-side foreign discovery: gating, forwarding, and peer error propagation.
//!
//! The origin gates on `all` before contacting a peer, forwards a request with
//! the `relay` selector cleared and no `on_behalf_of`, and propagates
//! peer-authored results and typed peer errors unchanged.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use super::super::{
    global_user_id, spawn_answering_peer, spawn_relay_stream_with_peer, write_peer_credential,
};
use super::{discovery_exchange, origin_fixture, spawn_origin_relay};

#[test]
fn foreign_discovery_requires_all_scope_before_peer_contact() {
    let (_temporary, bundle_name, configuration_roots, bundle_paths) = origin_fixture();
    write_peer_credential(&bundle_paths.state_root, "west", "peer-secret");
    // No listener bound: if authorization did not fail first, the forward would
    // dial this dead socket. `alpha` holds `list = home`, so the origin denies.
    let peer_socket = configuration_roots.base_layer().join("west.sock");
    let (client, handle) = spawn_relay_stream_with_peer(
        &configuration_roots,
        &bundle_paths,
        "west",
        "origin-relay",
        &peer_socket,
    );
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        "alpha",
        json!({"operation": "discover_namespaces", "relay": "west"}),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );
}

#[test]
fn foreign_namespace_discovery_forwards_without_origin_selectors() {
    let (temporary, bundle_name, configuration_roots, bundle_paths) = origin_fixture();
    write_peer_credential(&bundle_paths.state_root, "west", "peer-secret");
    let peer_socket = temporary.path().join("west.sock");
    let observed = spawn_answering_peer(
        &peer_socket,
        json!({
            "kind": "discover_namespaces",
            "schema_version": "test",
            "namespaces": ["myapp"],
        }),
    );
    let (client, handle) = spawn_relay_stream_with_peer(
        &configuration_roots,
        &bundle_paths,
        "west",
        "origin-relay",
        &peer_socket,
    );
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        global_user_id(&bundle_name).as_str(),
        json!({"operation": "discover_namespaces", "relay": "west"}),
    );
    // The peer-authored namespaces propagate unchanged.
    assert_eq!(response["response"]["kind"], "discover_namespaces");
    assert_eq!(response["response"]["namespaces"][0], "myapp");
    // The forwarded wire request carries no origin-local relay selector and no
    // on_behalf_of, and cannot trigger an onward peer lookup.
    let forwarded = observed
        .recv_timeout(Duration::from_secs(2))
        .expect("peer observed forwarded request");
    assert_eq!(forwarded["request"]["operation"], "discover_namespaces");
    assert!(forwarded["request"].get("relay").is_none());
    assert!(forwarded["request"].get("on_behalf_of").is_none());
}

#[test]
fn foreign_principal_discovery_propagates_peer_bundles_unchanged() {
    let (temporary, bundle_name, configuration_roots, bundle_paths) = origin_fixture();
    write_peer_credential(&bundle_paths.state_root, "west", "peer-secret");
    let peer_socket = temporary.path().join("west.sock");
    // A principal-scoped subset the peer authored, foreign id `myapp`.
    let observed = spawn_answering_peer(
        &peer_socket,
        json!({
            "kind": "discover_principals",
            "schema_version": "test",
            "bundles": [{
                "id": "myapp",
                "hosted": true,
                "state": "up",
                "startup_failure_count": 0,
                "recent_startup_failures": [],
                "principals": [{"id": "agent@myapp", "transport": "tmux", "ready": true}],
                "principals_partial": true,
            }],
        }),
    );
    let (client, handle) = spawn_relay_stream_with_peer(
        &configuration_roots,
        &bundle_paths,
        "west",
        "origin-relay",
        &peer_socket,
    );
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        global_user_id(&bundle_name).as_str(),
        json!({"operation": "discover_principals", "relay": "west", "namespace": "myapp"}),
    );
    assert_eq!(response["response"]["kind"], "discover_principals");
    let bundles = response["response"]["bundles"]
        .as_array()
        .expect("bundles array");
    assert_eq!(bundles.len(), 1);
    // The foreign id is not rewritten and the partial marker is preserved.
    assert_eq!(bundles[0]["id"], "myapp");
    assert_eq!(bundles[0]["principals_partial"], true);
    let forwarded = observed
        .recv_timeout(Duration::from_secs(2))
        .expect("peer observed forwarded request");
    assert_eq!(forwarded["request"]["operation"], "discover_principals");
    assert_eq!(forwarded["request"]["namespace"], "myapp");
    assert!(forwarded["request"].get("relay").is_none());
    assert!(forwarded["request"].get("on_behalf_of").is_none());
}

#[test]
fn foreign_discovery_propagates_peer_authorization_denial() {
    let (temporary, bundle_name, configuration_roots, bundle_paths) = origin_fixture();
    write_peer_credential(&bundle_paths.state_root, "west", "peer-secret");
    let peer_socket = temporary.path().join("west.sock");
    let _observed = spawn_answering_peer(
        &peer_socket,
        json!({
            "kind": "error",
            "error": {
                "code": "authorization_forbidden",
                "message": "cross-relay discovery denied by peer relay ingress scope",
            },
        }),
    );
    let (client, handle) = spawn_relay_stream_with_peer(
        &configuration_roots,
        &bundle_paths,
        "west",
        "origin-relay",
        &peer_socket,
    );
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        global_user_id(&bundle_name).as_str(),
        json!({"operation": "discover_namespaces", "relay": "west"}),
    );
    // The peer's typed denial propagates unchanged.
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );
}

#[test]
fn foreign_discovery_reports_unknown_alias_typed_error() {
    let (_temporary, bundle_name, configuration_roots, bundle_paths) = origin_fixture();
    write_peer_credential(&bundle_paths.state_root, "west", "peer-secret");
    let peer_socket = configuration_roots.base_layer().join("west.sock");
    let (client, handle) = spawn_relay_stream_with_peer(
        &configuration_roots,
        &bundle_paths,
        "west",
        "origin-relay",
        &peer_socket,
    );
    // The origin is `all`-authorized, but names an alias absent from `[[peers]]`.
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        global_user_id(&bundle_name).as_str(),
        json!({"operation": "discover_namespaces", "relay": "nonexistent"}),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_unknown_peer"
    );
}

#[test]
fn foreign_discovery_reports_unknown_peer_when_none_configured() {
    let (_temporary, bundle_name, configuration_roots, bundle_paths) = origin_fixture();
    let state_root = bundle_paths.state_root.clone();
    // No peers are configured at all. An `all`-scoped requester still gets a typed
    // unknown-peer error, distinct from the unknown-alias case (which has a peer
    // configured under a different alias).
    let (client, handle) = spawn_origin_relay(
        &configuration_roots,
        &state_root,
        vec![bundle_paths.clone()],
        &[],
    );
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        global_user_id(&bundle_name).as_str(),
        json!({"operation": "discover_namespaces", "relay": "west"}),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_unknown_peer"
    );
}

#[test]
fn foreign_discovery_reports_unreachable_on_peer_authentication_failure() {
    let (temporary, bundle_name, configuration_roots, bundle_paths) = origin_fixture();
    write_peer_credential(&bundle_paths.state_root, "west", "peer-secret");
    let peer_socket = temporary.path().join("west.sock");
    // A peer that answers the Hello with the structured credential-rejection error
    // frame a real relay emits. A rejected handshake — like an unreachable
    // endpoint — surfaces as `runtime_peer_unavailable` per the peer connection
    // classification, so authentication failure is not a distinct code.
    spawn_rejecting_peer(&peer_socket);
    let (client, handle) = spawn_relay_stream_with_peer(
        &configuration_roots,
        &bundle_paths,
        "west",
        "origin-relay",
        &peer_socket,
    );
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        global_user_id(&bundle_name).as_str(),
        json!({"operation": "discover_namespaces", "relay": "west"}),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "runtime_peer_unavailable"
    );
    drop(temporary);
}

#[test]
fn foreign_discovery_reports_missing_peer_credential_typed_error() {
    let (_temporary, bundle_name, configuration_roots, bundle_paths) = origin_fixture();
    // No peer credential is provisioned, so the forward cannot present an identity.
    let peer_socket = configuration_roots.base_layer().join("west.sock");
    let (client, handle) = spawn_relay_stream_with_peer(
        &configuration_roots,
        &bundle_paths,
        "west",
        "origin-relay",
        &peer_socket,
    );
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        global_user_id(&bundle_name).as_str(),
        json!({"operation": "discover_namespaces", "relay": "west"}),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "runtime_peer_credential_missing"
    );
}

#[test]
fn foreign_discovery_reports_unreachable_peer_typed_error() {
    let (_temporary, bundle_name, configuration_roots, bundle_paths) = origin_fixture();
    // The credential is present but no listener is bound, so the dial itself fails.
    write_peer_credential(&bundle_paths.state_root, "west", "peer-secret");
    let peer_socket = configuration_roots.base_layer().join("west.sock");
    let (client, handle) = spawn_relay_stream_with_peer(
        &configuration_roots,
        &bundle_paths,
        "west",
        "origin-relay",
        &peer_socket,
    );
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        global_user_id(&bundle_name).as_str(),
        json!({"operation": "discover_namespaces", "relay": "west"}),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "runtime_peer_unavailable"
    );
}

// A stub peer that accepts one connection, reads the dialer's Hello, then answers
// with the structured error frame a real relay emits for an unrecognized
// credential (rather than closing the socket). This drives the dialer's Response-
// error classification path (`client.rs` hello loop), not the bare-EOF path, so a
// regression parsing/classifying a real credential denial would be caught. The
// dialer surfaces the rejected handshake as `runtime_peer_unavailable`.
fn spawn_rejecting_peer(socket_path: &Path) {
    let listener = UnixListener::bind(socket_path).expect("bind rejecting peer socket");
    thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut reader = BufReader::new(stream.try_clone().expect("clone rejecting stream"));
        let mut stream = stream;
        let mut hello_line = String::new();
        if reader.read_line(&mut hello_line).is_err() {
            return;
        }
        let rejection = json!({
            "frame": "response",
            "request_id": Value::Null,
            "response": {
                "kind": "error",
                "error": {
                    "code": "validation_unrecognized_credential",
                    "message": "peer relay credential is not recognized",
                },
            },
        });
        let _ = writeln!(stream, "{rejection}");
        let _ = stream.flush();
        thread::sleep(Duration::from_millis(50));
    });
}
