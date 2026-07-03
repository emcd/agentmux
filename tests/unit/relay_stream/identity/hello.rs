//! Hello with valid / mismatched / unrecognized credentials, including the
//! socket-trust paths and the application-principal hello.

use agentmux::runtime::paths::BundleRuntimePaths;
use serde_json::json;
use tempfile::TempDir;

use super::*;

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

// Application principal Hello (@EXTERNAL token registered via new peer) ->
// application principal type assigned. IdentityIntrospect rights are
// exercised separately in [`introspect`].
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
