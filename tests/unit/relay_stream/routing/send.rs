//! Send-time routing: relay-wide `Send` to a bundle target by suffix, the
//! `home`-vs-`all` send-scope matrix, the cross-bundle fan-out, the
//! mixed-target regression, and the `unknown_bundle` / reserved-`@EXTERNAL`
//! rejections.

use agentmux::configuration::ConfigurationRoots;
use std::io::BufReader;

use agentmux::runtime::paths::BundleRuntimePaths;
use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;

/// Connects as the bundle's `@GLOBAL` operator (a relay-wide principal with no
/// bound bundle), sends one `send` request with the given target list, and
/// returns the response frame.
fn relay_wide_operator_send(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    targets: Value,
) -> Value {
    let (mut client, join) = spawn_relay_connection(configuration_roots, bundle_paths);
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
                "requester_session": operator_id,
                "message": "operator to bundle",
                "targets": targets,
                "broadcast": false,
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

/// Task 3.1: a bundle-bound session targets the operator by its `@GLOBAL`
/// principal id (no `namespace` field), and the message is delivered to the
/// registered relay-wide operator stream.
#[test]
fn send_to_global_target_is_delivered_to_registered_operator() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_send_global_target";
    let configuration_roots = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_roots, "default", bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let operator_id = global_user_id(bundle_name);

    // The operator registers as a relay-wide session and stays connected.
    let (mut operator_client, operator_join) =
        spawn_relay_connection(&configuration_roots, &bundle_paths);
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
    let (mut alpha_client, alpha_join) =
        spawn_relay_connection(&configuration_roots, &bundle_paths);
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
                "requester_session": "alpha",
                "message": "hello operator",
                "targets": [operator_id],
                "broadcast": false,
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
///
/// A relay-wide operator's home namespace is `GLOBAL`, so reaching into a bundle
/// is cross-namespace and requires `all` send scope.
#[test]
fn relay_wide_send_routes_to_bundle_target_by_suffix() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_relay_wide_to_bundle";
    let configuration_roots = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_roots, "default", bundle_name);
    write_policies_with_send(&configuration_roots, "all");
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response = relay_wide_operator_send(
        &configuration_roots,
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

/// A relay-wide operator reaching into a bundle under only `home` send scope
/// is denied: `home` confers authority only within the operator's own
/// (`GLOBAL`) namespace, not into bundle namespaces.
#[test]
fn relay_wide_send_into_bundle_denied_under_home_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_relay_wide_home_denied";
    let configuration_roots = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_roots, "default", bundle_name);
    write_policies_with_send(&configuration_roots, "home");
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response = relay_wide_operator_send(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!([format!("alpha@{bundle_name}")]),
    );

    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );
    assert_eq!(
        response["response"]["error"]["details"]["capability"],
        "send.deliver"
    );
}

/// Task 3.3: a relay-wide principal whose targets are all bare (no `@<namespace>`
/// suffix) is rejected — the relay requires fully-qualified targets and the
/// client never filled them in.
#[test]
fn relay_wide_send_with_bare_target_is_rejected() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_relay_wide_bare";
    let configuration_roots = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_roots, "default", bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response = relay_wide_operator_send(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!(["alpha"]),
    );

    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_unqualified_target"
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
    let configuration_roots = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_roots, "default", bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let operator_id = global_user_id(bundle_name);

    let (mut client, join) = spawn_relay_connection(&configuration_roots, &bundle_paths);
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
                "requester_session": "alpha",
                "message": "mixed targets",
                "targets": [operator_id, format!("bravo@{bundle_name}")],
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
    let configuration_roots = write_bundle_configuration(&temporary, bundle_a);
    write_tui_configuration(&configuration_roots, "default", bundle_a);
    write_ui_bundle(&configuration_roots, bundle_b);
    // The fan-out crosses into peer bundle-b, which now requires `all` send
    // scope (the uniform cross-bundle threshold).
    write_policies_with_send(&configuration_roots, "all");
    let state_root = temporary.path().join("state");
    let paths_a = BundleRuntimePaths::resolve(&state_root, bundle_a).expect("bundle-a paths");
    let paths_b = BundleRuntimePaths::resolve(&state_root, bundle_b).expect("bundle-b paths");
    let catalog = multi_bundle_catalog(&[paths_a.clone(), paths_b.clone()]);
    let operator_id = global_user_id(bundle_a);

    // The relay-wide operator registers and stays connected.
    let (mut operator_client, operator_join) =
        spawn_relay_connection_with_catalog(&configuration_roots, &state_root, catalog.clone());
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
        spawn_relay_connection_with_catalog(&configuration_roots, &state_root, catalog.clone());
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
        spawn_relay_connection_with_catalog(&configuration_roots, &state_root, catalog.clone());
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
                "requester_session": "alpha",
                "message": "fan out",
                "targets": [format!("agent@{bundle_b}"), operator_id],
                "broadcast": false,
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
    assert_eq!(
        agent_event["event"]["target_session"],
        format!("agent@{bundle_b}")
    );
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

/// A cross-bundle `Send` is denied when the sender's `send` scope is only
/// `home`. This is the uniform cross-bundle threshold (`all`) correcting
/// the prior permit-all stance; same-bundle delivery under `home` is
/// unaffected. **BREAKING** for callers that relied on permit-all cross-bundle
/// send.
#[test]
fn cross_bundle_send_denied_under_home_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_a = "party_send_home_a";
    let bundle_b = "party_send_home_b";
    let configuration_roots = write_bundle_configuration(&temporary, bundle_a);
    write_ui_bundle(&configuration_roots, bundle_b);
    write_policies_with_send(&configuration_roots, "home");
    let state_root = temporary.path().join("state");
    let paths_a = BundleRuntimePaths::resolve(&state_root, bundle_a).expect("bundle-a paths");
    let paths_b = BundleRuntimePaths::resolve(&state_root, bundle_b).expect("bundle-b paths");
    let catalog = multi_bundle_catalog(&[paths_a, paths_b]);

    let (mut client, join) =
        spawn_relay_connection_with_catalog(&configuration_roots, &state_root, catalog);
    let mut reader = BufReader::new(client.try_clone().expect("clone stream"));
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": format!("alpha@{bundle_a}"),
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
                "requester_session": "alpha",
                "message": "across the boundary",
                "targets": [format!("agent@{bundle_b}")],
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
        "authorization_forbidden"
    );
    assert_eq!(
        response["response"]["error"]["details"]["capability"],
        "send.deliver"
    );
}

/// Same-bundle delivery is unaffected by the cross-bundle threshold: a session
/// sends to a peer in its own bundle under `home` and the message is queued.
#[test]
fn same_bundle_send_permitted_under_home_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_send_same_home";
    let configuration_roots = write_bundle_configuration(&temporary, bundle_name);
    write_policies_with_send(&configuration_roots, "home");
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let (mut client, join) = spawn_relay_connection(&configuration_roots, &bundle_paths);
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
                "requester_session": "alpha",
                "message": "same bundle",
                "targets": [format!("bravo@{bundle_name}")],
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

    assert_eq!(response["response"]["kind"], "send");
    let results = response["response"]["results"]
        .as_array()
        .expect("results array");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["outcome"], "queued");
}

/// Task 2.2: a `Send` to a target qualified with an unconfigured bundle is
/// rejected as an unknown target.
#[test]
fn send_to_unknown_bundle_target_is_rejected() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_unknown_bundle";
    let configuration_roots = write_bundle_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let (mut client, join) = spawn_relay_connection(&configuration_roots, &bundle_paths);
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
                "requester_session": "alpha",
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
    let configuration_roots = write_bundle_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let (mut client, join) = spawn_relay_connection(&configuration_roots, &bundle_paths);
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
                "requester_session": "alpha",
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
