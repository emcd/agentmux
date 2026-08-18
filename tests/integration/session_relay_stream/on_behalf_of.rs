//! Cross-relay sender-attribution (`on_behalf_of`): an intermediary-asserted
//! origin subject that is advisory, single-hop, and never an authorization
//! input. The three guarantees exercised end to end are:
//!
//! - The origin relay stamps `on_behalf_of` from its verified requester
//!   identity (no self-assertion by a non-relay requester at the origin).
//! - The receiving relay honors `on_behalf_of` only from a peer-relay
//!   (ingress) requester and surfaces it in the delivered envelope.
//! - Setting `on_behalf_of` does not widen a peer's authority: a peer's
//!   scope gate is evaluated independently of the forwarded attribution,
//!   and a non-relay requester's value is dropped by the spoof gate.

use agentmux::configuration::ConfigurationRoots;
use std::io::BufReader;

use agentmux::{
    relay::{RelayRequest, RelayResponse, SendOutcome},
    runtime::paths::BundleRuntimePaths,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use uuid::Uuid;

use super::*;

// Registers a UI subscriber for `display`, runs one ingress Send from a peer
// relay principal to that target (optionally carrying an `on_behalf_of`), and
// returns the (send response frame, delivered `incoming_message` payload). The
// async delivery outlives the ingress connection on the process-global runtime,
// so the still-open UI subscriber observes it.
fn ingress_send_to_ui_display(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    relay_principal_id: &str,
    on_behalf_of: Option<&str>,
) -> (Value, Value) {
    let (mut ui_client, ui_handle) = spawn_relay_stream(configuration_roots, bundle_paths);
    let ui_read = ui_client.try_clone().expect("clone ui stream");
    let mut ui_reader = BufReader::new(ui_read);
    send_json(&mut ui_client, hello_payload(bundle_name, "display"));
    assert_eq!(read_json(&mut ui_reader)["frame"], "hello_ack");

    let mut request = json!({
        "operation": "send",
        "requester_session": relay_principal_id,
        "message": "attributed ingress hello",
        "targets": [format!("display@{bundle_name}")],
        "broadcast": false,
    });
    if let Some(value) = on_behalf_of {
        request["on_behalf_of"] = json!(value);
    }
    let response = ingress_request_response(
        configuration_roots,
        bundle_paths,
        relay_principal_id,
        request,
    );

    let display_target = format!("display@{bundle_name}");
    let events = collect_events_for_target(
        &ui_client,
        &mut ui_reader,
        display_target.as_str(),
        None,
        Duration::from_secs(5),
    );
    let payload = events
        .iter()
        .find(|value| value["event"]["event_type"] == "incoming_message")
        .expect("delivered incoming_message envelope")["event"]["payload"]
        .clone();

    ui_client.shutdown(std::net::Shutdown::Both).ok();
    ui_handle.join().expect("join ui relay stream");
    (response, payload)
}

// Registers a UI subscriber for `display`, dispatches one local Send to it with
// the given request `on_behalf_of`, and returns the delivered `incoming_message`
// payload. The in-process `dispatch_request` path carries no relay principal, so
// it models a non-relay requester for the spoof gate.
fn local_send_to_ui_display_payload(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    on_behalf_of: Option<String>,
) -> Value {
    let (mut ui_client, ui_handle) = spawn_relay_stream(configuration_roots, bundle_paths);
    let ui_read = ui_client.try_clone().expect("clone ui stream");
    let mut ui_reader = BufReader::new(ui_read);
    send_json(&mut ui_client, hello_payload(bundle_name, "display"));
    assert_eq!(read_json(&mut ui_reader)["frame"], "hello_ack");

    let display_target = format!("display@{bundle_name}");
    let response = dispatch_request(
        RelayRequest::Send {
            request_id: Some(format!("req-{}", Uuid::new_v4().simple())),
            requester_session: "alpha".to_string(),
            message: "local attributed hello".to_string(),
            targets: vec![display_target.clone()],
            broadcast: false,
            on_behalf_of,
        },
        configuration_roots,
        bundle_name,
        &bundle_paths.runtime_directory,
    )
    .expect("send response");
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results[0].outcome, SendOutcome::Queued);
    let terminal_id = results[0].message_id.clone();

    let events = collect_events_for_target(
        &ui_client,
        &mut ui_reader,
        display_target.as_str(),
        Some(terminal_id.as_str()),
        Duration::from_secs(5),
    );
    let payload = events
        .iter()
        .find(|value| value["event"]["event_type"] == "incoming_message")
        .expect("delivered incoming_message envelope")["event"]["payload"]
        .clone();

    ui_client.shutdown(std::net::Shutdown::Both).ok();
    ui_handle.join().expect("join ui relay stream");
    payload
}

#[test]
fn cross_relay_send_stamps_on_behalf_of_from_authenticated_origin() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_roots = write_cross_relay_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    write_peer_credential(&bundle_paths.state_root, "peer", "peer-secret");
    // Register the requester as a store-backed session so its Hello yields a
    // verified `authenticated_identity` the origin can stamp as on_behalf_of.
    let requester_principal = format!("alpha@{bundle_name}");
    write_authenticated_session_store(&bundle_paths.state_root, requester_principal.as_str());

    let peer_socket = temporary.path().join("peer.sock");
    let peer_response = json!({
        "kind": "send",
        "schema_version": "test",
        "requester_session": "origin-relay@RELAY",
        "results": [{
            "target_session": "bravo@other",
            "message_id": "peer-m1",
            "outcome": "queued",
        }],
    });
    let observed = spawn_answering_peer(&peer_socket, peer_response);

    let (results, forwarded) = forward_cross_relay_send_with_hello(
        &configuration_roots,
        &bundle_paths,
        &peer_socket,
        observed,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": requester_principal,
            "identity_token": INGRESS_PEER_TOKEN,
        }),
    );

    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["outcome"], "queued");
    // The forwarded Send carries the origin's verified principal as the origin
    // subject the peer is being asked to attribute the message to.
    assert_eq!(forwarded["request"]["on_behalf_of"], requester_principal);
}

#[test]
fn cross_relay_ingress_surfaces_on_behalf_of_in_delivered_envelope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_roots = write_bundle_configuration_with_ui_member(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some(bundle_name.as_str()),
    );

    let (response, payload) = ingress_send_to_ui_display(
        &configuration_roots,
        &bundle_paths,
        bundle_name.as_str(),
        relay_principal_id.as_str(),
        Some("origin-subject@remote"),
    );

    // The relay ingress requester is authorized, so the send is queued and the
    // response echoes the honored attribution.
    assert_eq!(response["response"]["kind"], "send");
    assert_eq!(
        response["response"]["on_behalf_of"],
        "origin-subject@remote"
    );
    // The delivered envelope shows both this relay's verified peer identity and
    // the forwarded origin subject.
    assert_eq!(payload["authenticated_identity"], relay_principal_id);
    assert_eq!(payload["on_behalf_of"], "origin-subject@remote");
}

#[test]
fn cross_relay_ingress_ignores_on_behalf_of_for_authorization() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_roots = write_bundle_configuration_with_ui_member(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    // The scope names a different bundle, so the target is out of scope. Setting
    // on_behalf_of must not widen the peer's authority.
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some("some-other-bundle"),
    );

    let response = ingress_request_response(
        &configuration_roots,
        &bundle_paths,
        relay_principal_id.as_str(),
        json!({
            "operation": "send",
            "requester_session": relay_principal_id,
            "message": "ingress hello",
            "targets": [format!("display@{bundle_name}")],
            "broadcast": false,
            "on_behalf_of": "some-operator@some-other-bundle",
        }),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );
    assert_eq!(
        response["response"]["error"]["details"]["capability"],
        "ingress"
    );
}

#[test]
fn local_send_drops_self_asserted_on_behalf_of() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_roots = write_bundle_configuration_with_ui_member(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");

    // A non-relay requester sets on_behalf_of directly; the spoof gate must drop
    // it so it never reaches the delivered envelope.
    let payload = local_send_to_ui_display_payload(
        &configuration_roots,
        &bundle_paths,
        bundle_name.as_str(),
        Some("victim@elsewhere".to_string()),
    );
    assert!(
        payload.get("on_behalf_of").is_none(),
        "a non-relay requester must not self-assert on_behalf_of"
    );
}

#[test]
fn local_send_omits_on_behalf_of_by_default() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_roots = write_bundle_configuration_with_ui_member(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");

    // Regression: an ordinary local delivery carries no on_behalf_of.
    let payload = local_send_to_ui_display_payload(
        &configuration_roots,
        &bundle_paths,
        bundle_name.as_str(),
        None,
    );
    assert!(payload.get("on_behalf_of").is_none());
    assert_eq!(payload["sender_session"], format!("alpha@{bundle_name}"));
}
