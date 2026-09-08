//! `change scope` peer-grant administration and live multi-namespace ingress.
//!
//! Exercises the breaking peer-grant contract through the public relay-stream
//! surface: namespace-set/wildcard grammar and canonical persistence, the
//! dedicated `change.scope` authority, in-place credential-preserving updates,
//! current-record (not Hello-snapshot) ingress decisions for Send and existing
//! discovery, and credential-bound staleness after drop/re-registration.

use std::io::BufReader;
use std::os::unix::net::UnixStream;

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

#[test]
fn new_peer_canonicalizes_set_scope_in_store() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_canon";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "canon@RELAY";

    register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("  zeta , alpha,alpha ,GLOBAL "),
    );

    let record = store_record(&bundle_paths, peer_id);
    assert_eq!(
        record["scope"], "GLOBAL,alpha,zeta",
        "scope must persist trimmed, sorted, and deduplicated: {record}"
    );
}

#[test]
fn new_peer_rejects_malformed_scopes() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_malformed";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    // Each case names a distinct peer so a rejected registration cannot shadow
    // a later case; every rejection must be a scope validation failure.
    let cases = [
        ("bad-empty-item@RELAY", "alpha,,beta"),
        ("bad-mixed-wildcard@RELAY", "*,alpha"),
        ("bad-exact@RELAY", "alpha@ident_scope_malformed"),
        ("bad-relay@RELAY", "RELAY"),
        ("bad-external@RELAY", "EXTERNAL"),
        ("bad-chars@RELAY", "not a namespace"),
    ];
    for (peer_id, scope) in cases {
        let response = operator_request(
            &configuration_roots,
            &bundle_paths,
            bundle_name,
            json!({"operation": "new_peer", "principal_id": peer_id, "scope": scope}),
        );
        assert_eq!(
            response["response"]["kind"], "error",
            "malformed scope {scope:?} must not register: {response:?}"
        );
        assert_eq!(
            response["response"]["error"]["code"], "validation_invalid_params",
            "malformed scope {scope:?}: {response:?}"
        );
    }
}

#[test]
fn change_scope_updates_grant_and_returns_canonical() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_update";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "update@RELAY";

    register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some(bundle_name),
    );
    let hash_before = store_record(&bundle_paths, peer_id)["credential_hash"].clone();

    let updated = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some(" zeta,alpha "),
    );
    assert_eq!(
        updated["response"]["kind"], "change_scope",
        "change scope rejected: {updated:?}"
    );
    assert_eq!(updated["response"]["scope"], "alpha,zeta");

    let record = store_record(&bundle_paths, peer_id);
    assert_eq!(record["scope"], "alpha,zeta");
    assert_eq!(
        record["credential_hash"], hash_before,
        "scope replacement must preserve the credential: {record}"
    );
}

#[test]
fn change_scope_empty_clears_grant_and_repeats_safely() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_clear";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "clear@RELAY";

    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );

    for _ in 0..2 {
        let cleared = change_scope_request(
            &configuration_roots,
            &bundle_paths,
            bundle_name,
            peer_id,
            Some(""),
        );
        assert_eq!(
            cleared["response"]["kind"], "change_scope",
            "clear rejected: {cleared:?}"
        );
        assert_eq!(cleared["response"]["scope"], "");
    }
    assert!(
        store_record(&bundle_paths, peer_id).get("scope").is_none(),
        "cleared scope must persist absent"
    );

    let denied = peer_send_response(
        &configuration_roots,
        &bundle_paths,
        peer_id,
        &psk,
        &format!("alpha@{bundle_name}"),
    );
    assert_eq!(denied["response"]["kind"], "error");
    assert_eq!(
        denied["response"]["error"]["code"], "authorization_forbidden",
        "cleared peer must lose ingress rights: {denied:?}"
    );
}

#[test]
fn change_scope_requires_dedicated_authority() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_auth";
    // The shared identity configuration grants change.psk but not
    // change.scope: rotation rights must not confer scope administration.
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "auth@RELAY";

    register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some(bundle_name),
    );

    let denied = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );
    assert_eq!(denied["response"]["kind"], "error");
    assert_eq!(
        denied["response"]["error"]["code"], "authorization_forbidden",
        "rotation-only caller must not change scope: {denied:?}"
    );
    assert_eq!(
        denied["response"]["error"]["details"]["capability"], "change.scope",
        "denial must identify the scope capability: {denied:?}"
    );
    assert_eq!(
        store_record(&bundle_paths, peer_id)["scope"],
        bundle_name,
        "denied update must leave the record untouched"
    );
}

#[test]
fn change_scope_unknown_peer_reports_behind_authorization() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_unknown";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let unknown = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        "ghost@RELAY",
        Some("*"),
    );
    assert_eq!(unknown["response"]["kind"], "error");
    assert_eq!(
        unknown["response"]["error"]["code"], "validation_unknown_principal",
        "authorized caller sees unknown-principal: {unknown:?}"
    );

    let non_relay = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &format!("alpha@{bundle_name}"),
        Some("*"),
    );
    assert_eq!(non_relay["response"]["kind"], "error");
    assert_eq!(
        non_relay["response"]["error"]["code"], "validation_invalid_principal_id",
        "non-relay target is rejected: {non_relay:?}"
    );
}

#[test]
fn change_scope_omitted_scope_is_invalid_not_a_clear() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_omitted";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "omitted@RELAY";

    register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some(bundle_name),
    );

    let rejected = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        None,
    );
    assert_eq!(rejected["response"]["kind"], "error");
    assert_eq!(
        rejected["response"]["error"]["code"], "validation_invalid_arguments",
        "omitted scope must not clear: {rejected:?}"
    );
    assert_eq!(
        store_record(&bundle_paths, peer_id)["scope"],
        bundle_name,
        "rejected update must preserve the grant"
    );
}

#[test]
fn live_narrowing_applies_on_the_same_connection_without_reconnect() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_live";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "live@RELAY";
    let target = format!("alpha@{bundle_name}");

    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );

    let (mut client, join) = spawn_relay_connection(&configuration_roots, &bundle_paths);
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
    let mut request_id = 0;
    let mut peer_request =
        |client: &mut UnixStream, reader: &mut BufReader<UnixStream>, request: Value| -> Value {
            request_id += 1;
            send_json(
                client,
                json!({
                    "frame": "request",
                    "request_id": format!("live-{request_id}"),
                    "request": request,
                }),
            );
            let mut response = read_json(reader);
            while response["frame"] != "response" {
                response = read_json(reader);
            }
            response
        };

    let first = peer_request(
        &mut client,
        &mut reader,
        json!({
            "operation": "send",
            "requester_session": peer_id,
            "message": "before narrowing",
            "targets": [target],
            "broadcast": false,
        }),
    );
    assert_eq!(
        first["response"]["kind"], "send",
        "wildcard send: {first:?}"
    );

    let first_raww = peer_request(
        &mut client,
        &mut reader,
        json!({
            "operation": "raww",
            "requester_session": peer_id,
            "target_session": target,
            "text": "before narrowing",
        }),
    );
    assert_eq!(
        first_raww["response"]["kind"], "raww",
        "wildcard raww: {first_raww:?}"
    );

    let narrowed = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("some-other-bundle"),
    );
    assert_eq!(narrowed["response"]["kind"], "change_scope");

    // The same connection loses ingress immediately: no reconnect, no rotation.
    let second = peer_request(
        &mut client,
        &mut reader,
        json!({
            "operation": "send",
            "requester_session": peer_id,
            "message": "after narrowing",
            "targets": [target],
            "broadcast": false,
        }),
    );
    assert_eq!(second["response"]["kind"], "error");
    assert_eq!(
        second["response"]["error"]["code"], "authorization_forbidden",
        "narrowed grant must deny on the existing connection: {second:?}"
    );

    let second_raww = peer_request(
        &mut client,
        &mut reader,
        json!({
            "operation": "raww",
            "requester_session": peer_id,
            "target_session": target,
            "text": "after narrowing",
        }),
    );
    assert_eq!(second_raww["response"]["kind"], "error");
    assert_eq!(
        second_raww["response"]["error"]["code"], "authorization_forbidden",
        "narrowed grant must deny raww: {second_raww:?}"
    );

    let principal_lookup = peer_request(
        &mut client,
        &mut reader,
        json!({"operation": "discover_principals", "namespace": bundle_name}),
    );
    assert_eq!(principal_lookup["response"]["kind"], "error");
    assert_eq!(
        principal_lookup["response"]["error"]["code"], "authorization_forbidden",
        "narrowed grant must deny principal discovery: {principal_lookup:?}"
    );

    // Namespace discovery on the same connection sees only covered candidates:
    // the narrowed scope matches nothing discoverable, so it is an empty
    // success rather than a denial.
    let namespaces = peer_request(
        &mut client,
        &mut reader,
        json!({"operation": "discover_namespaces"}),
    );
    assert_eq!(namespaces["response"]["kind"], "discover_namespaces");
    assert!(
        namespaces["response"]["namespaces"]
            .as_array()
            .expect("namespaces array")
            .is_empty(),
        "narrowed grant must hide the bundle: {namespaces:?}"
    );

    let widened = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );
    assert_eq!(widened["response"]["kind"], "change_scope");

    let third = peer_request(
        &mut client,
        &mut reader,
        json!({
            "operation": "send",
            "requester_session": peer_id,
            "message": "after widening",
            "targets": [target],
            "broadcast": false,
        }),
    );
    assert_eq!(
        third["response"]["kind"], "send",
        "widened grant must restore ingress on the same connection: {third:?}"
    );

    shutdown_stream(&client, "shutdown peer stream");
    join.join().expect("join peer relay thread");
}

#[test]
fn rotation_preserves_the_latest_narrowed_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_rotation";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "rotated@RELAY";

    register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );
    let narrowed = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some(bundle_name),
    );
    assert_eq!(narrowed["response"]["kind"], "change_scope");

    let rotation = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "change_psk", "principal_id": peer_id}),
    );
    assert_eq!(rotation["response"]["kind"], "change_psk");
    let rotated_psk = rotation["response"]["psk"]
        .as_str()
        .expect("rotated psk")
        .to_string();

    let record = store_record(&bundle_paths, peer_id);
    assert_eq!(
        record["scope"], bundle_name,
        "rotation must preserve the narrowed scope: {record}"
    );

    // The rotated credential authenticates and carries the narrowed grant.
    let accepted = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        peer_id,
        &rotated_psk,
        true,
    );
    assert_eq!(accepted["frame"], "hello_ack", "rotated psk: {accepted:?}");
    let denied = peer_send_response(
        &configuration_roots,
        &bundle_paths,
        peer_id,
        &rotated_psk,
        "alpha@some-other-bundle",
    );
    assert_eq!(denied["response"]["kind"], "error");
    assert_eq!(
        denied["response"]["error"]["code"],
        "validation_unknown_target"
    );
}

#[test]
fn drop_revokes_the_peer_connection_and_isolates_re_registration() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_dropbind";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "stale@RELAY";
    let target = format!("alpha@{bundle_name}");

    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );

    let (mut client, join) = spawn_relay_connection(&configuration_roots, &bundle_paths);
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

    // Dropping the peer revokes its live connection: the relay delivers a
    // typed revocation frame rather than leaving a stale session behind.
    let dropped = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "drop_peer", "principal_id": peer_id}),
    );
    assert_eq!(dropped["response"]["kind"], "drop_peer");

    let mut revoked = read_json(&mut reader);
    while revoked["frame"] != "response" {
        revoked = read_json(&mut reader);
    }
    assert_eq!(
        revoked["response"]["error"]["code"], "runtime_identity_revoked",
        "drop must revoke the peer session: {revoked:?}"
    );
    join.join().expect("join revoked peer thread");

    // Re-registering the same id mints an unrelated credential: the old PSK
    // authenticates nothing, so no stale binding can survive under the id.
    let fresh_psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );
    assert_ne!(fresh_psk, psk);
    let stale_hello = hello_first_frame(&configuration_roots, &bundle_paths, peer_id, &psk, true);
    assert_eq!(stale_hello["response"]["kind"], "error");
    assert_eq!(
        stale_hello["response"]["error"]["code"], "validation_unrecognized_credential",
        "superseded credential must not authenticate: {stale_hello:?}"
    );

    let fresh = peer_send_response(
        &configuration_roots,
        &bundle_paths,
        peer_id,
        &fresh_psk,
        &target,
    );
    assert_eq!(
        fresh["response"]["kind"], "send",
        "fresh credential: {fresh:?}"
    );
}

#[test]
fn wildcard_peer_reaches_bundle_targets_and_namespace_discovery() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_wildcard";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "wild@RELAY";

    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );

    let sent = peer_send_response(
        &configuration_roots,
        &bundle_paths,
        peer_id,
        &psk,
        &format!("alpha@{bundle_name}"),
    );
    assert_eq!(sent["response"]["kind"], "send", "wildcard send: {sent:?}");

    let (mut client, join) = spawn_relay_connection(&configuration_roots, &bundle_paths);
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
            "request_id": "wild-namespaces",
            "request": {"operation": "discover_namespaces"},
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    assert_eq!(response["response"]["kind"], "discover_namespaces");
    let namespaces: Vec<&str> = response["response"]["namespaces"]
        .as_array()
        .expect("namespaces array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        namespaces.contains(&bundle_name),
        "wildcard must discover the bundle: {namespaces:?}"
    );
    shutdown_stream(&client, "shutdown wildcard peer stream");
    join.join().expect("join wildcard peer thread");
}
