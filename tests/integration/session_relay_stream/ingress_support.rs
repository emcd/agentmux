use agentmux::{
    configuration::ConfigurationRoots,
    runtime::paths::{BundleRuntimePaths, principal_store_path},
};
use std::{io::BufReader, path::Path, time::Duration};

use serde_json::{Value, json};
use uuid::Uuid;

use super::{read_json, send_json, spawn_relay_stream};

// SHA-256 (lowercase hex) of the fixed ingress PSK below, embedded in the
// hand-written principal store so a Hello with the raw token authenticates.
pub(crate) const INGRESS_PEER_TOKEN: &str = "ingress-peer-secret";
pub(crate) const INGRESS_PEER_CREDENTIAL_HASH: &str =
    "1c9c2d8823d0f52409743bb29168008f69157c166de636fcca23631e26f8daa7";

/// Negative-assertion budget: how long to wait after `SendOutcome::Queued`
/// for the worker to attempt delivery against a not-yet-registered UI
/// stream, before the test registers the UI stream. The worker must
/// have had time to subscribe-fail-drop for the routing decision to be
/// exercised; 300ms is generous for the worker cycle on any loaded
/// CI machine. Reduce with a per-test override if a future test
/// shows the cycle is faster.
pub(crate) const UI_DELIVERY_WORKER_BUDGET: Duration =
    Duration::from_millis(300);

// A per-test-unique peer relay principal id. The process-wide stream registry is
// keyed by `principal_id`, so concurrent tests must not share one or they collide
// with an identity-claim conflict. The credential hash is fixed (tied to the one
// token), independent of the id.
pub(crate) fn unique_relay_principal_id() -> String {
    format!("origin-{}@RELAY", Uuid::new_v4().simple())
}

// Hand-writes a principal store registering `principal_id` as a peer relay with
// the fixed ingress credential and the given `scope` (`None` for a peer
// registered without a scope, which the ingress gate treats as fail-closed).
pub(crate) fn write_ingress_peer_store(
    state_root: &Path,
    principal_id: &str,
    scope: Option<&str>,
) {
    let store_path = principal_store_path(state_root);
    std::fs::create_dir_all(store_path.parent().expect("store parent"))
        .expect("create identity directory");
    let scope_field = match scope {
        Some(value) => format!(",\n      \"scope\": \"{value}\""),
        None => String::new(),
    };
    let body = format!(
        "{{\n  \"format_version\": 1,\n  \"principals\": [\n    {{\n      \"principal_id\": \"{principal_id}\",\n      \"principal_type\": \"relay\",\n      \"credential_hash\": \"{INGRESS_PEER_CREDENTIAL_HASH}\"{scope_field}\n    }}\n  ]\n}}"
    );
    std::fs::write(&store_path, body).expect("write principal store");
}

// Hand-writes a principal store registering `principal_id` as a store-backed
// (PSK-authenticated) session principal with the fixed ingress credential, so a
// Hello with `INGRESS_PEER_TOKEN` yields a verified `authenticated_identity`
// (unlike a socket-trust session, which carries none).
pub(crate) fn write_authenticated_session_store(
    state_root: &Path,
    principal_id: &str,
) {
    let store_path = principal_store_path(state_root);
    std::fs::create_dir_all(store_path.parent().expect("store parent"))
        .expect("create identity directory");
    let body = format!(
        "{{\n  \"format_version\": 1,\n  \"principals\": [\n    {{\n      \"principal_id\": \"{principal_id}\",\n      \"principal_type\": \"session\",\n      \"credential_hash\": \"{INGRESS_PEER_CREDENTIAL_HASH}\"\n    }}\n  ]\n}}"
    );
    std::fs::write(&store_path, body).expect("write principal store");
}

// Hellos as `principal_id` (a peer relay) with its registered PSK, submits one
// request, and returns the decoded response frame.
pub(crate) fn ingress_request_response(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    principal_id: &str,
    request: Value,
) -> Value {
    let (mut client, handle) =
        spawn_relay_stream(configuration_roots, bundle_paths);
    let reader_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(reader_stream);
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": principal_id,
            "identity_token": INGRESS_PEER_TOKEN,
        }),
    );
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");

    let request_id = format!("req-{}", Uuid::new_v4().simple());
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": request_id,
            "request": request,
        }),
    );
    let response = read_json(&mut reader);
    client.shutdown(std::net::Shutdown::Both).ok();
    handle.join().expect("join relay stream");
    response
}

// Convenience wrapper: forwards an ingress Send to a single `target`.
pub(crate) fn ingress_send_response(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    principal_id: &str,
    target: &str,
) -> Value {
    ingress_request_response(
        configuration_roots,
        bundle_paths,
        principal_id,
        json!({
            "operation": "send",
            "requester_session": principal_id,
            "message": "ingress hello",
            "targets": [target],
            "broadcast": false,
        }),
    )
}
