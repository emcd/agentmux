//! Cross-relay (bang-path) Send forwarding and ingress filter:
//! - The 4 send tests exercise the outbound fan-out end to end: a bundle
//!   member on the origin relay addresses a cross-relay bang-path target, the
//!   origin dials a stub peer relay (PSK Hello), forwards the Send, and
//!   folds the peer's response (or a transport failure) into the merged
//!   Send result.
//! - The 6 ingress tests exercise the receiving side: a client authenticates
//!   as a peer relay principal (`<id>@RELAY`) whose store record carries a
//!   registered ingress scope, then forwards a Send to a local target. The
//!   receiving relay gates each target against the peer's scope
//!   (deny-by-default), preserving existence-before-authorization ordering.

use std::io::BufReader;

use agentmux::runtime::paths::BundleRuntimePaths;
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;

use super::*;

#[test]
fn cross_relay_send_propagates_peer_delivery_outcome() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_cross_relay_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    write_peer_credential(&bundle_paths.state_root, "peer", "peer-secret");

    let peer_socket = temporary.path().join("peer.sock");
    // The peer answers with a queued delivery result for its local target; the
    // origin re-labels it to the bang-path the requester addressed.
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

    let (results, forwarded) = forward_cross_relay_send(
        &configuration_root,
        &bundle_paths,
        bundle_name.as_str(),
        &peer_socket,
        observed,
    );

    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["target_session"], "bravo@other!peer");
    assert_eq!(results[0]["outcome"], "queued");

    // The peer received a Send presenting this relay's `<relay-id>@RELAY` identity
    // and its plain local target (not the bang-path).
    assert_eq!(forwarded["request"]["operation"], "send");
    assert_eq!(
        forwarded["request"]["requester_session"],
        "origin-relay@RELAY"
    );
    assert_eq!(forwarded["request"]["targets"][0], "bravo@other");
    // The origin is socket-trust (no verified identity), so it stamps no
    // `on_behalf_of` origin attribution onto the forwarded Send. The field is
    // omitted from the wire request entirely, not serialized as an explicit null.
    assert!(forwarded["request"].get("on_behalf_of").is_none());
}

#[test]
fn cross_relay_send_reports_ingress_denied_as_failed() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_cross_relay_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    write_peer_credential(&bundle_paths.state_root, "peer", "peer-secret");

    let peer_socket = temporary.path().join("peer.sock");
    // The peer rejects the forwarded target at its ingress filter; the origin
    // folds that error into a `failed` result carrying the peer's reason code.
    let peer_response = json!({
        "kind": "error",
        "error": {
            "code": "authorization_forbidden",
            "message": "ingress scope does not cover this target",
        },
    });
    let observed = spawn_answering_peer(&peer_socket, peer_response);

    let (results, _forwarded) = forward_cross_relay_send(
        &configuration_root,
        &bundle_paths,
        bundle_name.as_str(),
        &peer_socket,
        observed,
    );

    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["target_session"], "bravo@other!peer");
    assert_eq!(results[0]["outcome"], "failed");
    assert_eq!(results[0]["reason_code"], "authorization_forbidden");
}

#[test]
fn cross_relay_send_reports_peer_unavailable_when_unreachable() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_cross_relay_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    write_peer_credential(&bundle_paths.state_root, "peer", "peer-secret");

    // No listener is bound at this path: the peer is unreachable. The origin still
    // serves (an unreachable peer never blocks boot — the connection is lazy), and
    // the first delivery to it yields the distinct `peer_unavailable` outcome.
    let peer_socket = temporary.path().join("nonexistent-peer.sock");
    let (mut client, handle) = spawn_relay_stream_with_peer(
        &configuration_root,
        &bundle_paths,
        "peer",
        "origin-relay",
        &peer_socket,
    );
    let reader_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(reader_stream);
    send_json(&mut client, hello_payload(bundle_name.as_str(), "alpha"));
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");

    let request_id = format!("req-{}", Uuid::new_v4().simple());
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": request_id,
            "request": {
                "operation": "send",
                "requester_session": "alpha",
                "message": "cross-relay hello",
                "targets": ["bravo@other!peer"],
                "broadcast": false,
            },
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(response["response"]["kind"], "send");
    let results = response["response"]["results"]
        .as_array()
        .expect("results array");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["target_session"], "bravo@other!peer");
    assert_eq!(results[0]["outcome"], "peer_unavailable");
    assert_eq!(results[0]["reason_code"], "runtime_peer_unavailable");

    client.shutdown(std::net::Shutdown::Both).ok();
    handle.join().expect("join relay stream");
}

#[test]
fn cross_relay_send_requires_all_tier() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    // The default policy grants alpha `send = home`, so a cross-relay (always
    // all-tier) target is denied at authorization before any peer is dialed.
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    write_peer_credential(&bundle_paths.state_root, "peer", "peer-secret");

    // A peer is configured, so the request is not reported as unavailable; the
    // socket is never dialed because authorization fails first (no listener bound).
    let peer_socket = temporary.path().join("unused-peer.sock");
    let (mut client, handle) = spawn_relay_stream_with_peer(
        &configuration_root,
        &bundle_paths,
        "peer",
        "origin-relay",
        &peer_socket,
    );
    let reader_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(reader_stream);
    send_json(&mut client, hello_payload(bundle_name.as_str(), "alpha"));
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");

    let request_id = format!("req-{}", Uuid::new_v4().simple());
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": request_id,
            "request": {
                "operation": "send",
                "requester_session": "alpha",
                "message": "cross-relay hello",
                "targets": ["bravo@other!peer"],
                "broadcast": false,
            },
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );

    client.shutdown(std::net::Shutdown::Both).ok();
    handle.join().expect("join relay stream");
}

#[test]
fn cross_relay_ingress_accepts_in_scope_target() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration_with_ui_member(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    // A bundle-wide scope covers every session in the target bundle.
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some(bundle_name.as_str()),
    );

    let response = ingress_send_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        format!("display@{bundle_name}").as_str(),
    );
    assert_eq!(response["response"]["kind"], "send");
    let results = response["response"]["results"]
        .as_array()
        .expect("results array");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["outcome"], "queued");
    assert_eq!(
        results[0]["target_session"],
        format!("display@{bundle_name}")
    );
}

#[test]
fn cross_relay_ingress_denies_out_of_scope_target() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration_with_ui_member(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    // The scope names a different bundle, so the target is out of scope.
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some("some-other-bundle"),
    );

    let response = ingress_send_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        format!("display@{bundle_name}").as_str(),
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
fn cross_relay_ingress_denies_absent_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration_with_ui_member(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    // A peer registered without a scope covers nothing (deny-by-default).
    write_ingress_peer_store(&bundle_paths.state_root, relay_principal_id.as_str(), None);

    let response = ingress_send_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        format!("display@{bundle_name}").as_str(),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );
}

#[test]
fn cross_relay_ingress_unknown_target_sorts_before_authorization() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration_with_ui_member(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    // Out-of-scope peer targeting a non-member: existence is validated by the
    // spine's prepare stage before the ingress gate, so an unknown target sorts
    // as `validation_unknown_target` rather than `authorization_forbidden`.
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some("some-other-bundle"),
    );

    let response = ingress_send_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        format!("ghost@{bundle_name}").as_str(),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_unknown_target"
    );
}

#[test]
fn cross_relay_ingress_rejects_bang_path_send_before_forwarding() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration_with_ui_member(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    // In-scope for the local bundle, but the target carries a bang-path onward
    // relay id. A peer must not chain through this relay to a third relay, so the
    // request is rejected before any forwarding — the receiving relay here has no
    // peer manager configured, proving the safety does not depend on a third peer.
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some(bundle_name.as_str()),
    );

    let response = ingress_request_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        json!({
            "operation": "send",
            "requester_session": relay_principal_id,
            "message": "chain attempt",
            "targets": [format!("display@{bundle_name}!third")],
            "broadcast": false,
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
fn cross_relay_ingress_rejects_bang_path_raww_before_forwarding() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration_with_ui_member(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some(bundle_name.as_str()),
    );

    // Raww's cross-relay branch runs before the local spine; an ingress bang-path
    // target must be rejected there, not forwarded onward under this relay's
    // identity.
    let response = ingress_request_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        json!({
            "operation": "raww",
            "requester_session": relay_principal_id,
            "target_session": format!("display@{bundle_name}!third"),
            "text": "chain attempt",
            "no_enter": false,
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
