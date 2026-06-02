//! Routing-namespace tests for suffix-based target routing and the `GLOBAL`
//! namespace list (add-global-namespace-routing tasks 1.1-1.5 / 3.1-3.6).
//!
//! These exercise routing through `serve_connection`:
//!
//! - the reserved relay-wide specifiers `EXTERNAL`/`RELAY` remain rejected on
//!   the wire `namespace` selector;
//! - a relay-wide principal that omits the namespace on a `List` is rejected;
//! - `List` with `namespace = "GLOBAL"` enumerates registered relay-wide
//!   sessions (and excludes bundle sessions);
//! - `Send` infers each target's registry from its `@<namespace>` suffix: a
//!   bundle-bound session reaches an `@GLOBAL` operator, a relay-wide sender
//!   routes to a bundle via a bundle-qualified target, and a relay-wide sender
//!   with a bare target is rejected;
//! - a single `Send` fans out across namespaces (cross-bundle `@<bundle>` plus
//!   relay-wide `@GLOBAL`), unknown bundles and reserved `@EXTERNAL`/`@RELAY`
//!   targets are rejected, and mixing `@GLOBAL` with `@<bundle>` targets is no
//!   longer rejected (add-cross-namespace-routing);
//! - the retired `validation_namespace_routing_unavailable` stub no longer
//!   appears.

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

/// Connects as the bundle's `@GLOBAL` operator (a relay-wide principal with no
/// bound bundle), sends one `send` request with the given target list, and
/// returns the response frame.
fn relay_wide_operator_send(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    targets: Value,
) -> Value {
    let (mut client, join) = spawn_relay_connection(configuration_root, bundle_paths);
    let mut reader = BufReader::new(client.try_clone().expect("clone stream"));
    let operator_id = global_user_id(bundle_name);
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": operator_id,
            "identity_token": "socket-trust",
        }),
    );
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "req-1",
            "request": {
                "operation": "send",
                "sender_session": operator_id,
                "message": "operator to bundle",
                "targets": targets,
                "broadcast": false,
                "quiescence_timeout_ms": 1000,
            },
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown operator stream");
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

/// Task 3.1: a bundle-bound session targets the operator by its `@GLOBAL`
/// principal id (no `namespace` field), and the message is delivered to the
/// registered relay-wide operator stream.
#[test]
fn send_to_global_target_is_delivered_to_registered_operator() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_send_global_target";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let operator_id = global_user_id(bundle_name);

    // The operator registers as a relay-wide session and stays connected.
    let (mut operator_client, operator_join) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let mut operator_reader =
        BufReader::new(operator_client.try_clone().expect("clone operator stream"));
    send_json(
        &mut operator_client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": operator_id,
            "identity_token": "socket-trust",
        }),
    );
    assert_eq!(read_json(&mut operator_reader)["frame"], "hello_ack");

    // A bundle-bound session sends to the operator by its `@GLOBAL` id.
    let (mut alpha_client, alpha_join) = spawn_relay_connection(&configuration_root, &bundle_paths);
    let mut alpha_reader = BufReader::new(alpha_client.try_clone().expect("clone alpha stream"));
    send_json(
        &mut alpha_client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": format!("alpha@{bundle_name}"),
            "identity_token": "socket-trust",
        }),
    );
    assert_eq!(read_json(&mut alpha_reader)["frame"], "hello_ack");
    send_json(
        &mut alpha_client,
        json!({
            "frame": "request",
            "request_id": "req-1",
            "request": {
                "operation": "send",
                "sender_session": "alpha",
                "message": "hello operator",
                "targets": [operator_id],
                "broadcast": false,
                "quiescence_timeout_ms": 2000,
            },
        }),
    );
    let mut response = read_json(&mut alpha_reader);
    while response["frame"] != "response" {
        response = read_json(&mut alpha_reader);
    }
    assert_eq!(response["response"]["kind"], "send");

    // The operator observes the delivered message on its relay-wide stream.
    let event = read_until_event_type(&mut operator_reader, "incoming_message");
    assert_eq!(event["event"]["target_session"], operator_id);
    assert_eq!(
        event["event"]["payload"]["sender_session"],
        format!("alpha@{bundle_name}")
    );

    shutdown_stream(&alpha_client, "shutdown alpha stream");
    shutdown_stream(&operator_client, "shutdown operator stream");
    alpha_join.join().expect("join alpha thread");
    operator_join.join().expect("join operator thread");
}

/// Task 3.2: a relay-wide principal sends to a bundle session named by its
/// `@<bundle>` suffix; the relay infers the bundle and resolves the target.
#[test]
fn relay_wide_send_routes_to_bundle_target_by_suffix() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_relay_wide_to_bundle";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response = relay_wide_operator_send(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        json!([format!("alpha@{bundle_name}")]),
    );

    assert_eq!(response["response"]["kind"], "send");
    let results = response["response"]["results"]
        .as_array()
        .expect("results array");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["target_session"], format!("alpha@{bundle_name}"));
    assert_eq!(results[0]["outcome"], "queued");
}

/// Task 3.3: a relay-wide principal whose targets are all bare (no suffix) has
/// no routing context and is rejected.
#[test]
fn relay_wide_send_with_bare_target_is_rejected() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_relay_wide_bare";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response = relay_wide_operator_send(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        json!(["alpha"]),
    );

    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_missing_routing_namespace"
    );
}

/// Task 2.4 (regression): a single `Send` that mixes a relay-wide (`@GLOBAL`)
/// target with a bundle-session target is no longer rejected — it fans out and
/// returns a per-target result for each. `validation_conflicting_namespaces` is
/// retired.
#[test]
fn send_mixing_relay_wide_and_session_targets_fans_out() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_mixed_targets";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let operator_id = global_user_id(bundle_name);

    let (mut client, join) = spawn_relay_connection(&configuration_root, &bundle_paths);
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
            "request": {
                "operation": "send",
                "sender_session": "alpha",
                "message": "mixed targets",
                "targets": [operator_id, "bravo"],
                "broadcast": false,
                "quiescence_timeout_ms": 200,
            },
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown client stream");
    join.join().expect("join relay thread");

    assert_eq!(response["response"]["kind"], "send");
    assert_ne!(
        response["response"]["error"]["code"],
        "validation_conflicting_namespaces"
    );
    let results = response["response"]["results"]
        .as_array()
        .expect("results array");
    assert_eq!(results.len(), 2, "both targets receive a per-target result");
}

/// Task 2.1: a single `Send` from a bundle-a session targets both a peer-bundle
/// session (`agent@bundle-b`) and the relay-wide operator (`operator@GLOBAL`);
/// the relay fans out and delivers to each in its own namespace.
#[test]
fn send_fans_out_across_bundle_and_global_namespaces() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_a = "party_fanout_a";
    let bundle_b = "party_fanout_b";
    let configuration_root = write_bundle_configuration(&temporary, bundle_a);
    write_tui_configuration(&configuration_root, "default", bundle_a);
    write_ui_bundle(&configuration_root, bundle_b);
    let state_root = temporary.path().join("state");
    let paths_a = BundleRuntimePaths::resolve(&state_root, bundle_a).expect("bundle-a paths");
    let paths_b = BundleRuntimePaths::resolve(&state_root, bundle_b).expect("bundle-b paths");
    let catalog = multi_bundle_catalog(&[paths_a.clone(), paths_b.clone()]);
    let operator_id = global_user_id(bundle_a);

    // The relay-wide operator registers and stays connected.
    let (mut operator_client, operator_join) =
        spawn_relay_connection_with_catalog(&configuration_root, &state_root, catalog.clone());
    let mut operator_reader =
        BufReader::new(operator_client.try_clone().expect("clone operator stream"));
    send_json(
        &mut operator_client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": operator_id,
            "identity_token": "socket-trust",
        }),
    );
    assert_eq!(read_json(&mut operator_reader)["frame"], "hello_ack");

    // The `agent` UI session in bundle-b registers and stays connected.
    let (mut agent_client, agent_join) =
        spawn_relay_connection_with_catalog(&configuration_root, &state_root, catalog.clone());
    let mut agent_reader = BufReader::new(agent_client.try_clone().expect("clone agent stream"));
    send_json(
        &mut agent_client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": format!("agent@{bundle_b}"),
            "identity_token": "socket-trust",
        }),
    );
    assert_eq!(read_json(&mut agent_reader)["frame"], "hello_ack");

    // A bundle-a session fans one Send out to both namespaces.
    let (mut alpha_client, alpha_join) =
        spawn_relay_connection_with_catalog(&configuration_root, &state_root, catalog.clone());
    let mut alpha_reader = BufReader::new(alpha_client.try_clone().expect("clone alpha stream"));
    send_json(
        &mut alpha_client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": format!("alpha@{bundle_a}"),
            "identity_token": "socket-trust",
        }),
    );
    assert_eq!(read_json(&mut alpha_reader)["frame"], "hello_ack");
    send_json(
        &mut alpha_client,
        json!({
            "frame": "request",
            "request_id": "req-1",
            "request": {
                "operation": "send",
                "sender_session": "alpha",
                "message": "fan out",
                "targets": [format!("agent@{bundle_b}"), operator_id],
                "broadcast": false,
                "quiescence_timeout_ms": 2000,
            },
        }),
    );
    let mut response = read_json(&mut alpha_reader);
    while response["frame"] != "response" {
        response = read_json(&mut alpha_reader);
    }
    assert_eq!(response["response"]["kind"], "send");
    let results = response["response"]["results"]
        .as_array()
        .expect("results array");
    assert_eq!(results.len(), 2);
    assert!(
        results
            .iter()
            .any(|result| result["target_session"] == format!("agent@{bundle_b}")),
        "cross-bundle target resolved in its own bundle"
    );
    assert!(
        results
            .iter()
            .any(|result| result["target_session"] == operator_id),
        "relay-wide target resolved"
    );

    // Both recipients observe the delivered message on their own streams.
    let agent_event = read_until_event_type(&mut agent_reader, "incoming_message");
    assert_eq!(agent_event["event"]["target_session"], "agent");
    assert_eq!(
        agent_event["event"]["payload"]["sender_session"],
        format!("alpha@{bundle_a}")
    );
    let operator_event = read_until_event_type(&mut operator_reader, "incoming_message");
    assert_eq!(operator_event["event"]["target_session"], operator_id);

    shutdown_stream(&alpha_client, "shutdown alpha stream");
    shutdown_stream(&agent_client, "shutdown agent stream");
    shutdown_stream(&operator_client, "shutdown operator stream");
    alpha_join.join().expect("join alpha thread");
    agent_join.join().expect("join agent thread");
    operator_join.join().expect("join operator thread");
}

/// Task 2.2: a `Send` to a target qualified with an unconfigured bundle is
/// rejected as an unknown target.
#[test]
fn send_to_unknown_bundle_target_is_rejected() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_unknown_bundle";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let (mut client, join) = spawn_relay_connection(&configuration_root, &bundle_paths);
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
            "request": {
                "operation": "send",
                "sender_session": "alpha",
                "message": "to nowhere",
                "targets": ["agent@no_such_bundle"],
                "broadcast": false,
            },
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown client stream");
    join.join().expect("join relay thread");

    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_unknown_target"
    );
}

/// Task 2.3: a `Send` to a target in the reserved `@EXTERNAL` namespace is
/// rejected as an unsupported namespace.
#[test]
fn send_to_external_namespace_target_is_rejected() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_external_target";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let (mut client, join) = spawn_relay_connection(&configuration_root, &bundle_paths);
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
            "request": {
                "operation": "send",
                "sender_session": "alpha",
                "message": "to external",
                "targets": ["service@EXTERNAL"],
                "broadcast": false,
            },
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown client stream");
    join.join().expect("join relay thread");

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

/// Task 3.4: `List` with `namespace = "GLOBAL"` returns the registered
/// relay-wide sessions, including the requesting operator's own session.
#[test]
fn list_global_namespace_returns_registered_relay_wide_sessions() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_global_list_present";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let operator_id = global_user_id(bundle_name);

    let (mut client, join) = spawn_relay_connection(&configuration_root, &bundle_paths);
    let mut reader = BufReader::new(client.try_clone().expect("clone stream"));
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": operator_id,
            "identity_token": "socket-trust",
        }),
    );
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "req-1",
            "namespace": "GLOBAL",
            "request": {"operation": "list", "sender_session": operator_id},
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown operator stream");
    join.join().expect("join relay thread");

    assert_eq!(response["response"]["kind"], "list");
    assert_eq!(response["response"]["bundle"]["id"], "GLOBAL");
    let sessions = response["response"]["bundle"]["sessions"]
        .as_array()
        .expect("sessions array");
    let operator = sessions
        .iter()
        .find(|session| session["id"] == operator_id)
        .expect("registered operator present in GLOBAL list");
    assert_eq!(operator["transport"], "ui");
}

/// Task 3.4 (companion): the `GLOBAL` list contains only relay-wide sessions;
/// bundle sessions never appear. Robust under the process-wide registry shared
/// across parallel tests because it asserts the absence of a session-keyed id.
#[test]
fn list_global_namespace_excludes_bundle_sessions() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_global_list_excludes";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response = bundle_session_list_with_namespace(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        "GLOBAL",
    );

    assert_eq!(response["response"]["kind"], "list");
    assert_eq!(response["response"]["bundle"]["id"], "GLOBAL");
    let sessions = response["response"]["bundle"]["sessions"]
        .as_array()
        .expect("sessions array");
    assert!(
        !sessions.iter().any(|session| {
            session["id"] == "alpha" || session["id"] == format!("alpha@{bundle_name}")
        }),
        "GLOBAL list must not include bundle sessions"
    );
}

/// Task 3.6: the retired `validation_namespace_routing_unavailable` stub no
/// longer appears on the `GLOBAL` routing paths (`List` with `GLOBAL`, and a
/// `Send` to an `@GLOBAL` target).
#[test]
fn global_routing_no_longer_returns_unavailable_stub() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_stub_retired";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let operator_id = global_user_id(bundle_name);

    // (a) `List` with the `GLOBAL` namespace resolves to a list, not the stub.
    let list_response = bundle_session_list_with_namespace(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        "GLOBAL",
    );
    assert_eq!(list_response["response"]["kind"], "list");
    assert_ne!(
        list_response["response"]["error"]["code"],
        "validation_namespace_routing_unavailable"
    );

    // (b) A bundle-bound session sending to a configured `@GLOBAL` operator
    // resolves to a send (queued for the operator), not the stub.
    let (mut client, join) = spawn_relay_connection(&configuration_root, &bundle_paths);
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
            "request": {
                "operation": "send",
                "sender_session": "alpha",
                "message": "stub check",
                "targets": [operator_id],
                "broadcast": false,
                "quiescence_timeout_ms": 200,
            },
        }),
    );
    let mut send_response = read_json(&mut reader);
    while send_response["frame"] != "response" {
        send_response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown client stream");
    join.join().expect("join relay thread");

    assert_eq!(send_response["response"]["kind"], "send");
    assert_ne!(
        send_response["response"]["error"]["code"],
        "validation_namespace_routing_unavailable"
    );
}
