//! `authenticated_identity` on the Send response and on the delivered
//! envelope, for both store-backed and socket-trust senders.

use agentmux::configuration::ConfigurationRoots;
use std::io::BufReader;

use agentmux::runtime::paths::BundleRuntimePaths;
use serde_json::Value;
use tempfile::TempDir;

use super::*;

/// Connects an `@GLOBAL` operator as a relay-wide receiver, then connects as
/// `alpha@<bundle>` presenting `identity_token` and sends one message to the
/// operator. Returns the sender-side `send` response frame. Both connections are
/// closed before returning.
fn alpha_send_response_with_token(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    identity_token: &str,
) -> Value {
    let operator_id = global_user_id(bundle_name);
    let (mut operator_client, operator_join) =
        spawn_relay_connection(configuration_roots, bundle_paths);
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

    let (mut alpha_client, alpha_join) = spawn_relay_connection(configuration_roots, bundle_paths);
    let mut alpha_reader = BufReader::new(alpha_client.try_clone().expect("clone alpha stream"));
    send_json(
        &mut alpha_client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": format!("alpha@{bundle_name}"),
            "identity_token": identity_token,
        }),
    );
    assert_eq!(
        read_json(&mut alpha_reader)["frame"],
        "hello_ack",
        "alpha hello not acked"
    );
    send_json(
        &mut alpha_client,
        json!({
            "frame": "request",
            "request_id": "send-attribution",
            "request": {
                "operation": "send",
                "requester_session": "alpha",
                "message": "attribution probe",
                "targets": [operator_id],
                "broadcast": false,
            },
        }),
    );
    let mut response = read_json(&mut alpha_reader);
    while response["frame"] != "response" {
        response = read_json(&mut alpha_reader);
    }
    shutdown_stream(&alpha_client, "shutdown alpha stream");
    shutdown_stream(&operator_client, "shutdown operator stream");
    alpha_join.join().expect("join alpha thread");
    operator_join.join().expect("join operator thread");
    response
}

/// Connects an `@GLOBAL` operator as a relay-wide UI receiver, sends one message
/// from `alpha@<bundle>` (presenting `identity_token`) to that operator, and
/// returns the `incoming_message` event the operator receives on its stream.
/// Both connections are closed before returning.
fn operator_incoming_message_for_alpha_send(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    identity_token: &str,
) -> Value {
    let operator_id = global_user_id(bundle_name);
    let (mut operator_client, operator_join) =
        spawn_relay_connection(configuration_roots, bundle_paths);
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

    let (mut alpha_client, alpha_join) = spawn_relay_connection(configuration_roots, bundle_paths);
    let mut alpha_reader = BufReader::new(alpha_client.try_clone().expect("clone alpha stream"));
    send_json(
        &mut alpha_client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": format!("alpha@{bundle_name}"),
            "identity_token": identity_token,
        }),
    );
    assert_eq!(
        read_json(&mut alpha_reader)["frame"],
        "hello_ack",
        "alpha hello not acked"
    );
    send_json(
        &mut alpha_client,
        json!({
            "frame": "request",
            "request_id": "send-delivery-attribution",
            "request": {
                "operation": "send",
                "requester_session": "alpha",
                "message": "delivery attribution probe",
                "targets": [operator_id],
                "broadcast": false,
            },
        }),
    );
    let mut response = read_json(&mut alpha_reader);
    while response["frame"] != "response" {
        response = read_json(&mut alpha_reader);
    }
    assert_eq!(response["response"]["kind"], "send", "{response:?}");

    let event = read_until_event_type(&mut operator_reader, "incoming_message");
    shutdown_stream(&alpha_client, "shutdown alpha stream");
    shutdown_stream(&operator_client, "shutdown operator stream");
    alpha_join.join().expect("join alpha thread");
    operator_join.join().expect("join operator thread");
    event
}

// 3.5 Send response for a store-backed session carries `authenticated_identity`
// set to the verified principal_id.
#[test]
fn send_from_store_backed_session_carries_authenticated_identity() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_attr_store_backed";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );
    let response =
        alpha_send_response_with_token(&configuration_roots, &bundle_paths, bundle_name, &psk);

    assert_eq!(response["response"]["kind"], "send", "{response:?}");
    assert_eq!(
        response["response"]["authenticated_identity"], principal_id,
        "store-backed sender must be attributed to its verified principal_id"
    );
    // Reserved field is defined but never set yet.
    assert!(
        response["response"]["on_behalf_of"].is_null(),
        "on_behalf_of must be absent until its setting mechanism lands"
    );
}

// 3.6 Send response for a socket-trust session omits `authenticated_identity`.
#[test]
fn send_from_socket_trust_session_omits_authenticated_identity() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_attr_socket_trust";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response = alpha_send_response_with_token(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        "socket-trust",
    );

    assert_eq!(response["response"]["kind"], "send", "{response:?}");
    assert!(
        response["response"]["authenticated_identity"].is_null(),
        "socket-trust sender must not be attributed an authenticated_identity"
    );
    assert!(
        response["response"]["on_behalf_of"].is_null(),
        "on_behalf_of must be absent until its setting mechanism lands"
    );
}

// 3.7 Delivered envelope on the recipient stream carries `authenticated_identity`
// set to the sender's verified principal_id.
#[test]
fn delivered_envelope_on_recipient_stream_carries_authenticated_identity() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_delivery_store_backed";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );
    let event = operator_incoming_message_for_alpha_send(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &psk,
    );

    assert_eq!(
        event["event"]["event_type"], "incoming_message",
        "{event:?}"
    );
    assert_eq!(
        event["event"]["payload"]["authenticated_identity"], principal_id,
        "delivered envelope must carry the sender's verified principal_id"
    );
    // Sanity: the existing sender_session attribution is unchanged.
    assert_eq!(event["event"]["payload"]["sender_session"], principal_id);
}

// Companion to 3.7: a socket-trust sender's delivered envelope omits
// `authenticated_identity`, mirroring the Send response.
#[test]
fn delivered_envelope_omits_authenticated_identity_for_socket_trust_sender() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_delivery_socket_trust";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let event = operator_incoming_message_for_alpha_send(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        "socket-trust",
    );

    assert_eq!(
        event["event"]["event_type"], "incoming_message",
        "{event:?}"
    );
    assert!(
        event["event"]["payload"]["authenticated_identity"].is_null(),
        "socket-trust sender must not be attributed in the delivered envelope"
    );
}
