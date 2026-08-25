//! Relay identity integration tests covering the integrated
//! Hello → credential-verification → register path and the relay-wide
//! `new peer` / `change psk` administration path through
//! `serve_connection`, using the shared relay-stream harness.
//!
//! The cluster files partition the 27 tests by concern:
//! - [`hello`]: Hello with valid / mismatched / unrecognized credentials,
//!   including the socket-trust paths and the application-principal hello.
//! - [`psk_lifecycle`]: `change psk` rotation, revocation of the
//!   credential-holding session, and fan-out of `identity.revoked` to
//!   trusted hosts.
//! - [`introspect`]: `IdentityIntrospect` dispatch gate and the
//!   connect-time `identity.snapshot` for application principals.
//! - [`expires`]: credential expiry teardown at Hello and the typed
//!   identity-lifecycle error codes.
//! - [`send_attribution`]: `authenticated_identity` on the Send response
//!   and on the delivered envelope, for both store-backed and socket-trust
//!   senders.
//! - [`new_peer`]: `new peer --output` credential-file writer
//!   (path validation, O_NOFOLLOW, mode 0600, end-to-end auth).
//! - [`second_claim`]: stale vs live prior-writer semantics for
//!   `register_stream`'s identity-claim conflict decision.
//!
//! Shared helpers (every cluster shares the per-bundle operator
//! configuration writer, the principal-store path, the operator-request
//! dispatcher, and the `register_peer` / `hello_first_frame` pairs)
//! live in this hub. Cluster-specific helpers live with their cluster.

use agentmux::configuration::ConfigurationRoots;
use std::{io::BufReader, path::PathBuf};

use agentmux::runtime::paths::BundleRuntimePaths;
use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;

mod concurrency;
mod drop_peer;
mod expires;
mod hello;
mod introspect;
mod new_peer;
mod psk_lifecycle;
mod second_claim;
mod send_attribution;

/// Writes a configuration whose operator preset grants
/// `new.peer`/`change.psk`/`drop.peer` at `all`, with a `@GLOBAL` operator
/// declared in the TUI configuration so relay-wide credential administration
/// authorizes.
fn write_identity_configuration(temporary: &TempDir, bundle_name: &str) -> ConfigurationRoots {
    let configuration_roots = write_bundle_configuration(temporary, bundle_name);
    std::fs::write(
        configuration_roots.base_layer().join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "home"
look = "self"
send = "home"

[[policies]]
id = "operator"

[policies.controls]
find = "self"
list = "home"
look = "home"
send = "home"

[policies.controls.new]
peer = "all"

[policies.controls.change]
psk = "all"

[policies.controls.drop]
peer = "all"
"#,
    )
    .expect("write operator policies configuration");
    write_tui_configuration(&configuration_roots, "operator", bundle_name);
    configuration_roots
}

/// Path to the relay-level principal store under a bundle's state root.
fn principal_store_file(bundle_paths: &BundleRuntimePaths) -> PathBuf {
    bundle_paths
        .state_root
        .join("identity")
        .join("principals.json")
}

/// Connects as the `@GLOBAL` operator, submits one relay-admin request, and
/// returns the response frame. The connection is closed before returning.
fn operator_request(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    request: Value,
) -> Value {
    let (mut client, join) = spawn_relay_connection(configuration_roots, bundle_paths);
    let read_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);
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
    let ack = read_json(&mut reader);
    assert_eq!(
        ack["frame"], "hello_ack",
        "operator hello not acked: {ack:?}"
    );
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "admin-1",
            "request": request,
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown operator stream");
    join.join().expect("join operator relay thread");
    response
}

/// Connects as `operator` over a connection served by `context`, submits one
/// relay-admin request, and returns the response frame. Unlike
/// [`operator_request`], every call shares `context` (and thus the relay-scope
/// identity-admin serialization lock), so concurrent callers exercise the real
/// cross-connection transaction ordering. Each concurrent caller must present a
/// distinct `operator` id, or the second Hello collides on the identity claim.
fn operator_request_on_context(
    context: &std::sync::Arc<agentmux::relay::ConnectionServeContext>,
    operator: &str,
    request: Value,
) -> Value {
    let (mut client, join) = spawn_relay_connection_on_context(std::sync::Arc::clone(context));
    let read_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": operator,
            "identity_token": "socket-trust",
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
            "request_id": "admin-1",
            "request": request,
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown operator stream");
    join.join().expect("join operator relay thread");
    response
}

/// Registers `principal_id` via `new peer` and returns the issued PSK.
fn register_peer(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    principal_id: &str,
    scope: Option<&str>,
) -> String {
    let mut request = json!({"operation": "new_peer", "principal_id": principal_id});
    if let Some(scope) = scope {
        request["scope"] = Value::String(scope.to_string());
    }
    let response = operator_request(configuration_roots, bundle_paths, bundle_name, request);
    assert_eq!(
        response["response"]["kind"], "new_peer",
        "new peer rejected: {response:?}"
    );
    response["response"]["psk"]
        .as_str()
        .expect("psk in new peer response")
        .to_string()
}

/// Sends one Hello over a fresh connection and returns the first server frame
/// (a `hello_ack` on success, or an error `response` on rejection). The
/// connection is closed before returning.
fn hello_first_frame(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    principal_id: &str,
    identity_token: &str,
    require_session_credentials: bool,
) -> Value {
    let (mut client, join) = spawn_relay_connection_with_enforcement(
        configuration_roots,
        bundle_paths,
        require_session_credentials,
    );
    let read_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);
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
    shutdown_stream(&client, "shutdown hello client stream");
    join.join().expect("join hello relay thread");
    frame
}
