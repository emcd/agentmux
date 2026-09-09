//! `change scope` peer-grant administration and live multi-namespace ingress:
//! - [`grammar`]: namespace-set/wildcard grammar, canonical persistence, and
//!   malformed-scope rejection.
//! - [`updates`]: in-place credential-preserving updates (update, clear,
//!   no-op), the dedicated `change.scope` authority, live narrowing/widening
//!   on one connection for Send/Raww plus both discovery operations, rotation
//!   preserving the narrowed scope, and wildcard reach.
//! - [`binding`]: drop/revocation and rotation isolation of stale bindings.
//! - [`faults`]: pre-commit failure preservation, post-rename
//!   durability-uncertainty effectiveness and retry, and rotation recovery.
//! - [`concurrency`]: shared-context serialization of concurrent updates and
//!   rotation, admitted-work survival, and admin progress during a stalled
//!   principal probe.
//!
//! Shared helpers (the scope-grant operator configuration, the admin request
//! dispatcher, store inspection, and the peer send prober) live in this hub.
//! Cluster-specific helpers live with their cluster.
//!

use std::io::BufReader;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{
    Arc, Barrier, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant};

use agentmux::configuration::ConfigurationRoots;
use agentmux::runtime::paths::BundleRuntimePaths;
use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;

/// Writes the identity-test operator configuration plus an explicit
/// `change.scope=all` grant, so scope updates authorize.
fn write_scope_configuration(temporary: &TempDir, bundle_name: &str) -> ConfigurationRoots {
    let configuration_roots = write_bundle_configuration(temporary, bundle_name);
    std::fs::write(
        configuration_roots.base_layer().join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "home"
look = "self"
send = "home"

[[policies]]
id = "operator"

[policies.controls]
list = "home"
look = "home"
send = "home"

[policies.controls.new]
peer = "all"

[policies.controls.change]
psk = "all"
scope = "all"

[policies.controls.drop]
peer = "all"
"#,
    )
    .expect("write operator policies configuration");
    write_tui_configuration(&configuration_roots, "operator", bundle_name);
    configuration_roots
}

/// Submits one `change_scope` admin request as the `@GLOBAL` operator.
fn change_scope_request(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    principal_id: &str,
    scope: Option<&str>,
) -> Value {
    let mut request = json!({"operation": "change_scope", "principal_id": principal_id});
    if let Some(scope) = scope {
        request["scope"] = Value::String(scope.to_string());
    }
    operator_request(configuration_roots, bundle_paths, bundle_name, request)
}

/// Reads one principal record from the on-disk store.
fn store_record(bundle_paths: &BundleRuntimePaths, principal_id: &str) -> Value {
    let raw =
        std::fs::read_to_string(principal_store_file(bundle_paths)).expect("read principal store");
    let store: Value = serde_json::from_str(&raw).expect("parse principal store");
    store["principals"]
        .as_array()
        .expect("principals array")
        .iter()
        .find(|record| record["principal_id"] == principal_id)
        .unwrap_or_else(|| panic!("missing record for {principal_id}: {store}"))
        .clone()
}

/// Submits one ingress Send as a peer relay over a fresh connection.
fn peer_send_response(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    peer_id: &str,
    psk: &str,
    target: &str,
) -> Value {
    let (mut client, join) = spawn_relay_connection(configuration_roots, bundle_paths);
    let read_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": peer_id,
            "identity_token": psk,
        }),
    );
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "peer-send",
            "request": {
                "operation": "send",
                "requester_session": peer_id,
                "message": "scope probe",
                "targets": [target],
                "broadcast": false,
            },
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown peer stream");
    join.join().expect("join peer relay thread");
    response
}

mod binding;
mod concurrency;
mod faults;
mod grammar;
mod updates;
