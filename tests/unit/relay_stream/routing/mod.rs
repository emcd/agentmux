//! Routing namespace tests for suffix-based target routing and the
//! `GLOBAL` namespace list.
//!
//! The cluster files partition the 23 tests by concern:
//! - [`raww`]: relay-wide and cross-bundle `raww` routing, including the
//!   bare-target and the `home`-vs-`all` raww-scope matrix.
//! - [`namespace`]: rejection of the reserved `EXTERNAL`/`RELAY` namespace
//!   selectors and the `validation_missing_routing_namespace` rejection
//!   for a relay-wide `List` that omits the namespace.
//! - [`send`]: send-time routing, including relay-wide `Send` to a bundle
//!   target by suffix, the `home`-vs-`all` send-scope matrix, the
//!   cross-bundle fan-out, the mixed-target regression, and the
//!   `unknown_bundle` / reserved-`@EXTERNAL` rejections.
//! - [`global_list`]: `List` with `namespace = "GLOBAL"`, both the
//!   registered- and excluded-bundle-sessions perspectives, the
//!   process-only registration seed, and the declared-offline
//!   relay-wide principal.
//!
//! Shared helpers (every cluster shares the
//! `bundle_session_list_with_namespace` helper used by both the
//! namespace-rejection and the `GLOBAL` list clusters) live in this hub.
//! Cluster-specific helpers live with their cluster.

use agentmux::configuration::ConfigurationRoots;
use std::io::BufReader;

use serde_json::{Value, json};

use super::*;

mod global_list;
mod namespace;
mod raww;
mod send;

/// Connects as a bundle-bound `alpha` session, sends one `list` request whose
/// frame carries the given routing `namespace`, and returns the response frame.
fn bundle_session_list_with_namespace(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    namespace: &str,
) -> Value {
    let (mut client, join) = spawn_relay_connection(configuration_roots, bundle_paths);
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
            "request": {"operation": "list", "requester_session": "alpha"},
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
