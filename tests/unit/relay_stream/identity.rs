//! Identity-federation Slice 1 integration tests (tasks 1.12-1.20).
//!
//! These exercise the integrated Hello -> credential-verification -> register
//! path and the relay-wide `new peer` / `change psk` administration path
//! through `serve_connection`, using the shared relay-stream harness.

use super::*;

/// Writes a configuration whose operator preset grants `new.peer`/`change.psk`
/// at `all:all`, with a `@GLOBAL` operator declared in the TUI configuration so
/// relay-wide credential administration authorizes.
fn write_identity_configuration(temporary: &TempDir, bundle_name: &str) -> PathBuf {
    let configuration_root = write_bundle_configuration(temporary, bundle_name);
    std::fs::write(
        configuration_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "all:home"
look = "self"
send = "all:home"

[[policies]]
id = "operator"

[policies.controls]
find = "self"
list = "all:home"
look = "all:home"
send = "all:home"

[policies.controls.new]
peer = "all:all"

[policies.controls.change]
psk = "all:all"
"#,
    )
    .expect("write operator policies configuration");
    write_tui_configuration(&configuration_root, "operator", bundle_name);
    configuration_root
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
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    request: Value,
) -> Value {
    let (mut client, join) = spawn_relay_connection(configuration_root, bundle_paths);
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

/// Registers `principal_id` via `new peer` and returns the issued PSK.
fn register_peer(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    principal_id: &str,
    scope: Option<&str>,
) -> String {
    let mut request = json!({"operation": "new_peer", "principal_id": principal_id});
    if let Some(scope) = scope {
        request["scope"] = Value::String(scope.to_string());
    }
    let response = operator_request(configuration_root, bundle_paths, bundle_name, request);
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
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    principal_id: &str,
    identity_token: &str,
    require_session_credentials: bool,
) -> Value {
    let (mut client, join) = spawn_relay_connection_with_enforcement(
        configuration_root,
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

// 1.12 Hello with a valid session credential -> session registered with a
// stable principal_id.
#[test]
fn hello_with_valid_session_credential_registers_principal() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_valid_session";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let psk = register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );
    let frame = hello_first_frame(
        &configuration_root,
        &bundle_paths,
        &principal_id,
        &psk,
        false,
    );

    assert_eq!(frame["frame"], "hello_ack", "expected ack: {frame:?}");
    assert_eq!(frame["principal_id"], principal_id);
}

// 1.13 Hello with "socket-trust" and enforcement off -> session accepted, no
// principal store entry created.
#[test]
fn hello_with_socket_trust_enforcement_off_accepts_without_store_entry() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_socket_trust_off";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let frame = hello_first_frame(
        &configuration_root,
        &bundle_paths,
        &principal_id,
        "socket-trust",
        false,
    );

    assert_eq!(frame["frame"], "hello_ack", "expected ack: {frame:?}");
    assert_eq!(frame["principal_id"], principal_id);
    assert!(
        !principal_store_file(&bundle_paths).exists(),
        "socket-trust must not create a principal store entry"
    );
}

// 1.14 Hello with "socket-trust" and enforcement on -> typed error, session not
// registered.
#[test]
fn hello_with_socket_trust_enforcement_on_is_rejected() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_socket_trust_on";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let frame = hello_first_frame(
        &configuration_root,
        &bundle_paths,
        &principal_id,
        "socket-trust",
        true,
    );

    assert_eq!(
        frame["frame"], "response",
        "expected error response: {frame:?}"
    );
    assert_eq!(frame["response"]["kind"], "error");
    assert_eq!(
        frame["response"]["error"]["code"],
        "validation_credential_required"
    );
}

// 1.15 Hello with an unrecognized credential -> typed error regardless of the
// enforcement setting.
#[test]
fn hello_with_unrecognized_credential_is_rejected() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_unrecognized";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    for enforcement in [false, true] {
        let frame = hello_first_frame(
            &configuration_root,
            &bundle_paths,
            &principal_id,
            "unregistered-token",
            enforcement,
        );
        assert_eq!(frame["frame"], "response", "expected error: {frame:?}");
        assert_eq!(frame["response"]["kind"], "error");
        assert_eq!(
            frame["response"]["error"]["code"], "validation_unrecognized_credential",
            "enforcement={enforcement}"
        );
    }
}

// 1.16 Reconnect with the same credential -> same principal_id resolved from the
// store.
#[test]
fn reconnect_with_same_credential_resolves_same_principal() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_reconnect";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let psk = register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );
    let first = hello_first_frame(
        &configuration_root,
        &bundle_paths,
        &principal_id,
        &psk,
        false,
    );
    let second = hello_first_frame(
        &configuration_root,
        &bundle_paths,
        &principal_id,
        &psk,
        false,
    );

    assert_eq!(first["frame"], "hello_ack", "first: {first:?}");
    assert_eq!(second["frame"], "hello_ack", "second: {second:?}");
    assert_eq!(first["principal_id"], principal_id);
    assert_eq!(second["principal_id"], first["principal_id"]);
}

// 1.17 Application principal Hello (@EXTERNAL token registered via new peer) ->
// application principal type assigned. (IdentityIntrospect rights are Slice 2.)
#[test]
fn application_principal_hello_is_accepted() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_application";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    // Relay-wide principals register under their `principal_id` as the registry
    // key (bundle-independent), so application ids must be unique per test or
    // parallel runs collide with `runtime_identity_claim_conflict`.
    let principal_id = format!("engine_{bundle_name}@EXTERNAL");

    let request = json!({
        "operation": "new_peer",
        "principal_id": principal_id.as_str(),
        "scope": "introspect",
    });
    let registration = operator_request(&configuration_root, &bundle_paths, bundle_name, request);
    assert_eq!(
        registration["response"]["kind"], "new_peer",
        "new peer rejected: {registration:?}"
    );
    assert_eq!(registration["response"]["principal_type"], "application");
    let psk = registration["response"]["psk"]
        .as_str()
        .expect("psk in new peer response")
        .to_string();

    let frame = hello_first_frame(
        &configuration_root,
        &bundle_paths,
        principal_id.as_str(),
        &psk,
        false,
    );
    assert_eq!(frame["frame"], "hello_ack", "expected ack: {frame:?}");
    assert_eq!(frame["principal_id"], principal_id.as_str());
}

// 1.18 Hello with a valid credential but mismatched principal_id -> typed error,
// session not registered.
#[test]
fn hello_with_mismatched_principal_id_is_rejected() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_binding_mismatch";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let registered_id = format!("alpha@{bundle_name}");
    let claimed_id = format!("bravo@{bundle_name}");

    let psk = register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &registered_id,
        None,
    );
    // Present alpha's credential while claiming bravo's identity.
    let frame = hello_first_frame(&configuration_root, &bundle_paths, &claimed_id, &psk, false);

    assert_eq!(frame["frame"], "response", "expected error: {frame:?}");
    assert_eq!(frame["response"]["kind"], "error");
    assert_eq!(
        frame["response"]["error"]["code"],
        "validation_identity_binding_mismatch"
    );
}

// 1.19 `new peer` creates a principal in the store and returns its PSK; a
// subsequent Hello with that PSK resolves to the correct principal_id.
#[test]
fn new_peer_creates_principal_resolved_by_subsequent_hello() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_new_peer";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let psk = register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );
    assert!(
        principal_store_file(&bundle_paths).exists(),
        "new peer must persist the principal store"
    );
    assert!(!psk.is_empty(), "new peer must return a non-empty psk");

    let frame = hello_first_frame(
        &configuration_root,
        &bundle_paths,
        &principal_id,
        &psk,
        true,
    );
    assert_eq!(frame["frame"], "hello_ack", "expected ack: {frame:?}");
    assert_eq!(frame["principal_id"], principal_id);
}

// 1.20 `change psk` updates the store; the new PSK is accepted at Hello and the
// old PSK is rejected.
#[test]
fn change_psk_rotates_credential() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_change_psk";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let original_psk = register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );

    let rotation = operator_request(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        json!({"operation": "change_psk", "principal_id": principal_id}),
    );
    assert_eq!(
        rotation["response"]["kind"], "change_psk",
        "change psk rejected: {rotation:?}"
    );
    let rotated_psk = rotation["response"]["psk"]
        .as_str()
        .expect("psk in change psk response")
        .to_string();
    assert_ne!(rotated_psk, original_psk, "rotation must mint a new psk");

    let accepted = hello_first_frame(
        &configuration_root,
        &bundle_paths,
        &principal_id,
        &rotated_psk,
        true,
    );
    assert_eq!(accepted["frame"], "hello_ack", "new psk: {accepted:?}");
    assert_eq!(accepted["principal_id"], principal_id);

    let rejected = hello_first_frame(
        &configuration_root,
        &bundle_paths,
        &principal_id,
        &original_psk,
        false,
    );
    assert_eq!(rejected["frame"], "response", "old psk: {rejected:?}");
    assert_eq!(rejected["response"]["kind"], "error");
    assert_eq!(
        rejected["response"]["error"]["code"],
        "validation_unrecognized_credential"
    );
}

// --- Credential revocation: change psk tears down live sessions (Slice 2, task 2.11) ---

// 2.11 `change psk` on a principal with a live, store-backed session ->
// the session receives a `runtime_identity_revoked` error frame and its
// connection is then closed.
#[test]
fn change_psk_revokes_live_session_holding_old_credential() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_revoke_live";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let psk = register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );

    // Bring up a live, store-backed session and keep its connection open.
    let (mut alpha_client, alpha_join) = spawn_relay_connection(&configuration_root, &bundle_paths);
    let mut alpha_reader = BufReader::new(alpha_client.try_clone().expect("clone alpha stream"));
    send_json(
        &mut alpha_client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": principal_id,
            "identity_token": psk,
        }),
    );
    assert_eq!(
        read_json(&mut alpha_reader)["frame"],
        "hello_ack",
        "alpha hello not acked"
    );

    // Rotate the credential from the operator connection; alpha holds the old
    // credential and must be revoked. `operator_request` only returns after the
    // rotation handler has run, by which point the revocation frame is queued.
    let rotation = operator_request(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        json!({"operation": "change_psk", "principal_id": principal_id}),
    );
    assert_eq!(
        rotation["response"]["kind"], "change_psk",
        "change psk rejected: {rotation:?}"
    );

    // The live session observes the revocation frame ahead of EOF.
    let revoked = read_json_skipping_hello_ack(&mut alpha_reader);
    assert_eq!(revoked["frame"], "response", "revoked frame: {revoked:?}");
    assert_eq!(revoked["response"]["kind"], "error", "{revoked:?}");
    assert_eq!(
        revoked["response"]["error"]["code"], "runtime_identity_revoked",
        "revoked frame: {revoked:?}"
    );
    assert_eq!(
        revoked["response"]["error"]["details"]["principal_id"], principal_id,
        "revoked frame must name the rotated principal: {revoked:?}"
    );

    // The connection is then closed: the next read observes EOF.
    use std::io::BufRead;
    let mut trailing = String::new();
    let read = alpha_reader
        .read_line(&mut trailing)
        .expect("read after revocation");
    assert_eq!(
        read, 0,
        "connection must close after revocation; got {trailing:?}"
    );

    shutdown_stream(&alpha_client, "shutdown alpha stream");
    alpha_join.join().expect("join alpha relay thread");
}

// --- Credential expiry: Hello teardown on an expired principal (Slice 2, tasks 2.12/2.13) ---

/// Rewrites the principal store so `principal_id`'s record carries an
/// `expires_at` in the past, expiring its credential without disturbing the
/// credential hash. The store is loaded fresh on the next Hello, so the patched
/// expiry takes effect on the very next connection.
fn expire_principal_in_store(bundle_paths: &BundleRuntimePaths, principal_id: &str) {
    let path = principal_store_file(bundle_paths);
    let raw = std::fs::read_to_string(&path).expect("read principal store");
    let mut store: Value = serde_json::from_str(&raw).expect("parse principal store");
    let record = store["principals"]
        .as_array_mut()
        .expect("principals array in store")
        .iter_mut()
        .find(|record| record["principal_id"] == json!(principal_id))
        .expect("record for principal in store");
    record["expires_at"] = json!("2000-01-01T00:00:00Z");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&store).expect("serialize patched store"),
    )
    .expect("rewrite principal store");
}

/// Sends one Hello over a fresh connection and returns the first server frame
/// together with whether the relay then closed the connection (EOF on the next
/// read). Used to assert that a rejected Hello delivers its typed error frame
/// before the connection is torn down.
fn hello_frame_and_eof(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    principal_id: &str,
    identity_token: &str,
) -> (Value, bool) {
    use std::io::BufRead;
    let (mut client, join) = spawn_relay_connection(configuration_root, bundle_paths);
    let mut reader = BufReader::new(client.try_clone().expect("clone hello stream"));
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
    let mut trailing = String::new();
    let read = reader
        .read_line(&mut trailing)
        .expect("read after hello rejection");
    shutdown_stream(&client, "shutdown hello stream");
    join.join().expect("join hello relay thread");
    (frame, read == 0)
}

// 2.12 A principal whose credential has expired is rejected at Hello with a
// `runtime_identity_expired` error frame, and the relay then closes the
// connection.
#[test]
fn expired_principal_receives_runtime_identity_expired_before_close() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_expired_teardown";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let psk = register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );
    // Backdate the registered credential's expiry; the PSK itself is unchanged.
    expire_principal_in_store(&bundle_paths, &principal_id);

    let (frame, closed) =
        hello_frame_and_eof(&configuration_root, &bundle_paths, &principal_id, &psk);

    assert_eq!(
        frame["frame"], "response",
        "expected error frame: {frame:?}"
    );
    assert_eq!(frame["response"]["kind"], "error", "{frame:?}");
    assert_eq!(
        frame["response"]["error"]["code"], "runtime_identity_expired",
        "an expired credential must be rejected with the distinct typed code: {frame:?}"
    );
    assert_eq!(
        frame["response"]["error"]["details"]["principal_id"], principal_id,
        "the expiry error must name the expired principal: {frame:?}"
    );
    assert!(
        closed,
        "the relay must close the connection after the expiry frame"
    );
}

// 2.13 The typed identity-lifecycle error codes (`runtime_identity_expired`,
// `runtime_identity_revoked`) are distinct from one another and from the
// transport-level `relay_unavailable`, so a client can tell an expired or
// revoked credential apart from a relay outage.
#[test]
fn typed_identity_error_codes_are_distinct_from_relay_unavailable() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_typed_codes";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    // Expiry path: an expired credential is rejected at Hello.
    let expired_id = format!("alpha@{bundle_name}");
    let expired_psk = register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &expired_id,
        None,
    );
    expire_principal_in_store(&bundle_paths, &expired_id);
    let (expired_frame, _) = hello_frame_and_eof(
        &configuration_root,
        &bundle_paths,
        &expired_id,
        &expired_psk,
    );
    let expired_code = expired_frame["response"]["error"]["code"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Revocation path: rotating a live session's credential tears it down.
    let revoked_id = format!("bravo@{bundle_name}");
    let revoked_psk = register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &revoked_id,
        None,
    );
    let (mut live_client, live_join) = spawn_relay_connection(&configuration_root, &bundle_paths);
    let mut live_reader = BufReader::new(live_client.try_clone().expect("clone live stream"));
    send_json(
        &mut live_client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": revoked_id,
            "identity_token": revoked_psk,
        }),
    );
    assert_eq!(
        read_json(&mut live_reader)["frame"],
        "hello_ack",
        "live hello not acked"
    );
    let rotation = operator_request(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        json!({"operation": "change_psk", "principal_id": revoked_id}),
    );
    assert_eq!(
        rotation["response"]["kind"], "change_psk",
        "change psk rejected: {rotation:?}"
    );
    let revoked_frame = read_json_skipping_hello_ack(&mut live_reader);
    let revoked_code = revoked_frame["response"]["error"]["code"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    shutdown_stream(&live_client, "shutdown live stream");
    live_join.join().expect("join live relay thread");

    assert_eq!(
        expired_code, "runtime_identity_expired",
        "expiry frame: {expired_frame:?}"
    );
    assert_eq!(
        revoked_code, "runtime_identity_revoked",
        "revoked frame: {revoked_frame:?}"
    );
    assert_ne!(
        expired_code, "relay_unavailable",
        "an expired credential must not surface as a transport outage"
    );
    assert_ne!(
        revoked_code, "relay_unavailable",
        "a revoked credential must not surface as a transport outage"
    );
    assert_ne!(
        expired_code, revoked_code,
        "expiry and revocation must carry distinguishable typed codes"
    );
}

// --- Identity introspection: IdentityIntrospect dispatch gate (Slice 2, tasks 2.9/2.10) ---

/// Connects with `principal_id` + `identity_token`, issues one
/// `IdentityIntrospect` for `target_session` (optionally qualified by
/// `bundle_name`), and returns the response frame. The connection is closed
/// before returning.
fn introspect_request(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    principal_id: &str,
    identity_token: &str,
    target_session: &str,
    bundle_name: Option<&str>,
) -> Value {
    let (mut client, join) = spawn_relay_connection(configuration_root, bundle_paths);
    let mut reader = BufReader::new(client.try_clone().expect("clone introspect stream"));
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": principal_id,
            "identity_token": identity_token,
        }),
    );
    assert_eq!(
        read_json(&mut reader)["frame"],
        "hello_ack",
        "introspect client hello not acked"
    );
    let mut request = json!({
        "operation": "identity_introspect",
        "target_session": target_session,
    });
    if let Some(bundle_name) = bundle_name {
        request["bundle_name"] = Value::String(bundle_name.to_string());
    }
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "introspect-1",
            "request": request,
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown introspect stream");
    join.join().expect("join introspect relay thread");
    response
}

// 2.9 An application principal introspects an in-scope session and receives the
// target's principal_id, expires_at, and verified: true.
#[test]
fn application_principal_introspects_active_session() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_introspect_ok";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let target_principal_id = format!("alpha@{bundle_name}");
    // Unique application id per test: the relay-wide registry keys on the
    // `principal_id`, so a shared `@EXTERNAL` id collides across parallel runs.
    let app_principal_id = format!("engine_{bundle_name}@EXTERNAL");

    // Register the introspection target as a session principal in the store.
    register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &target_principal_id,
        None,
    );
    // Register the application principal scoped to the whole bundle (bare-bundle
    // scope covers every session in that namespace).
    let app_psk = register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &app_principal_id,
        Some(bundle_name),
    );

    let response = introspect_request(
        &configuration_root,
        &bundle_paths,
        &app_principal_id,
        &app_psk,
        "alpha",
        Some(bundle_name),
    );

    assert_eq!(
        response["response"]["kind"], "identity_introspect",
        "expected introspect response: {response:?}"
    );
    assert_eq!(
        response["response"]["principal_id"], target_principal_id,
        "introspection must surface the target's stable principal_id"
    );
    assert_eq!(
        response["response"]["verified"], true,
        "an unexpired principal must verify"
    );
    // The target was registered without an expiry, so `expires_at` is absent
    // rather than a placeholder timestamp (it is an optional ISO 8601 field).
    assert!(
        response["response"]["expires_at"].is_null(),
        "expires_at must be absent for a principal that never expires: {response:?}"
    );
    assert!(
        response["response"]["on_behalf_of"].is_null(),
        "on_behalf_of must be absent until its setting mechanism lands"
    );
}

// 2.10 A session principal issuing IdentityIntrospect is denied: it carries no
// introspection rights, so the gate returns an authorization denial and no
// identity data.
#[test]
fn session_principal_introspect_is_denied() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_introspect_denied";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    // A socket-trust session principal: accepted at Hello (enforcement off) but
    // granted no introspection rights.
    let response = introspect_request(
        &configuration_root,
        &bundle_paths,
        &format!("alpha@{bundle_name}"),
        "socket-trust",
        "alpha",
        Some(bundle_name),
    );

    assert_eq!(
        response["response"]["kind"], "error",
        "session introspect must be rejected: {response:?}"
    );
    assert_eq!(
        response["response"]["error"]["code"], "authorization_forbidden",
        "session introspect must be an authorization denial: {response:?}"
    );
    assert!(
        response["response"]["principal_id"].is_null(),
        "a denied introspection must not leak identity data"
    );
}

// 2.6 A trusted-host (application principal) connection receives an
// identity.snapshot event immediately after Hello, carrying the active
// principal records within its registered scope.
#[test]
fn application_principal_receives_identity_snapshot_on_connect() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_snapshot_connect";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let target_principal_id = format!("alpha@{bundle_name}");
    // Unique application id per test: the relay-wide registry keys on the
    // `principal_id`, so a shared `@EXTERNAL` id collides across parallel runs.
    let app_principal_id = format!("engine_{bundle_name}@EXTERNAL");

    // An in-scope session principal in the store, plus the application principal
    // scoped to the whole bundle (bare-bundle scope covers every session in it).
    register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &target_principal_id,
        None,
    );
    let app_psk = register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &app_principal_id,
        Some(bundle_name),
    );

    let (mut client, join) = spawn_relay_connection(&configuration_root, &bundle_paths);
    let mut reader = BufReader::new(client.try_clone().expect("clone app stream"));
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": app_principal_id.as_str(),
            "identity_token": app_psk,
        }),
    );
    assert_eq!(
        read_json(&mut reader)["frame"],
        "hello_ack",
        "app hello not acked"
    );

    // The snapshot is the very next frame after the ack (no permission snapshot
    // is emitted for a non-UI application principal).
    let frame = read_json(&mut reader);
    shutdown_stream(&client, "shutdown app stream");
    join.join().expect("join app relay thread");

    assert_eq!(
        frame["frame"], "event",
        "expected snapshot event: {frame:?}"
    );
    assert_eq!(
        frame["event"]["event_type"], "identity.snapshot",
        "{frame:?}"
    );
    assert_eq!(frame["event"]["target_session"], app_principal_id.as_str());
    let principals = frame["event"]["payload"]["principals"]
        .as_array()
        .expect("principals array in snapshot payload");
    let ids: Vec<&str> = principals
        .iter()
        .filter_map(|entry| entry["principal_id"].as_str())
        .collect();
    assert!(
        ids.contains(&target_principal_id.as_str()),
        "snapshot must carry the in-scope target: {frame:?}"
    );
    // The host's own @EXTERNAL record is outside the bundle scope and omitted.
    assert!(
        !ids.contains(&app_principal_id.as_str()),
        "out-of-scope principals must not appear in the snapshot: {frame:?}"
    );
    let target_entry = principals
        .iter()
        .find(|entry| entry["principal_id"] == json!(target_principal_id))
        .expect("target entry in snapshot");
    assert_eq!(
        target_entry["verified"], true,
        "an active principal is verified in the snapshot"
    );
    assert!(
        target_entry["expires_at"].is_null(),
        "a never-expiring principal omits expires_at: {frame:?}"
    );
}

// 2.7 change psk on an in-scope principal fans out an identity.revoked event to
// every connected trusted host whose scope covers the revoked principal.
#[test]
fn change_psk_fans_out_identity_revoked_to_trusted_hosts() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_revoked_fanout";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let target_principal_id = format!("alpha@{bundle_name}");
    let app_principal_id = format!("engine_{bundle_name}@EXTERNAL");

    // An in-scope session principal to revoke, plus a trusted host scoped to the
    // whole bundle.
    register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &target_principal_id,
        None,
    );
    let app_psk = register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &app_principal_id,
        Some(bundle_name),
    );

    // Connect the trusted host and keep its connection open; it receives the
    // connect-time identity.snapshot first, which the revoked-event read skips.
    let (mut app_client, app_join) = spawn_relay_connection(&configuration_root, &bundle_paths);
    let mut app_reader = BufReader::new(app_client.try_clone().expect("clone app stream"));
    send_json(
        &mut app_client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": app_principal_id.as_str(),
            "identity_token": app_psk,
        }),
    );
    assert_eq!(
        read_json(&mut app_reader)["frame"],
        "hello_ack",
        "app hello not acked"
    );

    // Rotate the target's credential from the operator connection.
    // `operator_request` returns only after the rotation handler has run, by
    // which point the fan-out event is queued to the host's writer.
    let rotation = operator_request(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        json!({"operation": "change_psk", "principal_id": target_principal_id}),
    );
    assert_eq!(
        rotation["response"]["kind"], "change_psk",
        "change psk rejected: {rotation:?}"
    );

    let event = read_until_event_type(&mut app_reader, "identity.revoked");
    shutdown_stream(&app_client, "shutdown app stream");
    app_join.join().expect("join app relay thread");

    assert_eq!(
        event["event"]["payload"]["principal_id"], target_principal_id,
        "revoked event must name the rotated principal: {event:?}"
    );
    assert!(
        event["event"]["payload"]["revoked_at"].is_string(),
        "revoked event must carry a revoked_at timestamp: {event:?}"
    );
    assert_eq!(
        event["event"]["target_session"],
        app_principal_id.as_str(),
        "revoked event must be routed to the watching host"
    );
}

// --- Sender attribution: authenticated_identity on Send (Slice 3, tasks 3.1/3.5/3.6) ---

/// Connects an `@GLOBAL` operator as a relay-wide receiver, then connects as
/// `alpha@<bundle>` presenting `identity_token` and sends one message to the
/// operator. Returns the sender-side `send` response frame. Both connections are
/// closed before returning.
fn alpha_send_response_with_token(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    identity_token: &str,
) -> Value {
    let operator_id = global_user_id(bundle_name);
    let (mut operator_client, operator_join) =
        spawn_relay_connection(configuration_root, bundle_paths);
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

    let (mut alpha_client, alpha_join) = spawn_relay_connection(configuration_root, bundle_paths);
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
                "quiescence_timeout_ms": 2000,
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

// 3.5 Send response for a store-backed session carries `authenticated_identity`
// set to the verified principal_id.
#[test]
fn send_from_store_backed_session_carries_authenticated_identity() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_attr_store_backed";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let psk = register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );
    let response =
        alpha_send_response_with_token(&configuration_root, &bundle_paths, bundle_name, &psk);

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
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response = alpha_send_response_with_token(
        &configuration_root,
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

// --- Delivered-envelope attribution: authenticated_identity on the recipient
// stream (Slice 3, tasks 3.4/3.7) ---

/// Connects an `@GLOBAL` operator as a relay-wide UI receiver, sends one message
/// from `alpha@<bundle>` (presenting `identity_token`) to that operator, and
/// returns the `incoming_message` event the operator receives on its stream.
/// Both connections are closed before returning.
fn operator_incoming_message_for_alpha_send(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    identity_token: &str,
) -> Value {
    let operator_id = global_user_id(bundle_name);
    let (mut operator_client, operator_join) =
        spawn_relay_connection(configuration_root, bundle_paths);
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

    let (mut alpha_client, alpha_join) = spawn_relay_connection(configuration_root, bundle_paths);
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
                "quiescence_timeout_ms": 2000,
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

// 3.7 Delivered envelope on the recipient stream carries `authenticated_identity`
// set to the sender's verified principal_id.
#[test]
fn delivered_envelope_on_recipient_stream_carries_authenticated_identity() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_delivery_store_backed";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let psk = register_peer(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );
    let event = operator_incoming_message_for_alpha_send(
        &configuration_root,
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
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let event = operator_incoming_message_for_alpha_send(
        &configuration_root,
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

// --- `new peer --output` credential-file writer (task 1.5 / write_psk_output_file) ---

/// Issues `new peer` for `principal_id` with an `output_path` and returns the
/// full response frame (a `new_peer` response on success, or an error response
/// when the writer rejects the path).
fn new_peer_with_output(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    principal_id: &str,
    output_path: &Path,
) -> Value {
    operator_request(
        configuration_root,
        bundle_paths,
        bundle_name,
        json!({
            "operation": "new_peer",
            "principal_id": principal_id,
            "output_path": output_path.to_string_lossy(),
        }),
    )
}

fn assert_invalid_output_path(response: &Value) {
    assert_eq!(
        response["response"]["kind"], "error",
        "expected error response: {response:?}"
    );
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_invalid_output_path"
    );
}

// `new peer --output` rejects a non-absolute path.
#[test]
fn new_peer_output_rejects_relative_path() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_output_relative";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response = new_peer_with_output(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &format!("alpha@{bundle_name}"),
        Path::new("relative-credential.psk"),
    );
    assert_invalid_output_path(&response);
}

// `new peer --output` rejects an absolute path whose parent does not exist (no
// auto-creation of directory trees).
#[test]
fn new_peer_output_rejects_missing_parent_directory() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_output_missing_parent";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let output_path = temporary.path().join("absent").join("credential.psk");

    let response = new_peer_with_output(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &format!("alpha@{bundle_name}"),
        &output_path,
    );
    assert_invalid_output_path(&response);
    assert!(
        !output_path.exists(),
        "writer must not materialize the credential under a missing parent"
    );
}

// `new peer --output` refuses to follow a symlink at the target path
// (O_NOFOLLOW), so a symlinked output cannot redirect the credential write.
#[test]
fn new_peer_output_rejects_symlinked_target() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_output_symlink";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let output_directory = temporary.path().join("out");
    std::fs::create_dir_all(&output_directory).expect("create output directory");
    let link_path = output_directory.join("credential.psk");
    std::os::unix::fs::symlink(temporary.path().join("elsewhere.psk"), &link_path)
        .expect("create output symlink");

    let response = new_peer_with_output(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &format!("alpha@{bundle_name}"),
        &link_path,
    );
    assert_invalid_output_path(&response);
}

// `new peer --output` writes the PSK to the target with mode 0600, omits the
// PSK from the response, and the written credential authenticates at Hello.
#[test]
fn new_peer_output_writes_credential_with_mode_0600() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_output_success";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");
    let output_path = temporary.path().join("credential.psk");

    let response = new_peer_with_output(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        &principal_id,
        &output_path,
    );
    assert_eq!(
        response["response"]["kind"], "new_peer",
        "new peer with output rejected: {response:?}"
    );
    assert_eq!(
        response["response"]["output_path"],
        output_path.to_string_lossy().as_ref(),
        "response must echo the written output path"
    );
    assert!(
        response["response"]["psk"].is_null(),
        "psk must be omitted from the response when written to a file"
    );

    let mode = std::fs::metadata(&output_path)
        .expect("stat credential file")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "credential file must be owner-only (0600)");

    let psk = std::fs::read_to_string(&output_path).expect("read credential file");
    assert!(!psk.is_empty(), "written credential must be non-empty");
    let frame = hello_first_frame(
        &configuration_root,
        &bundle_paths,
        &principal_id,
        &psk,
        true,
    );
    assert_eq!(
        frame["frame"], "hello_ack",
        "written credential must authenticate: {frame:?}"
    );
    assert_eq!(frame["principal_id"], principal_id);
}
