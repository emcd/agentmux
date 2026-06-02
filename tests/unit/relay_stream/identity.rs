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
    let principal_id = "engine@EXTERNAL";

    let request = json!({
        "operation": "new_peer",
        "principal_id": principal_id,
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
        principal_id,
        &psk,
        false,
    );
    assert_eq!(frame["frame"], "hello_ack", "expected ack: {frame:?}");
    assert_eq!(frame["principal_id"], principal_id);
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
                "sender_session": "alpha",
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
                "sender_session": "alpha",
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
