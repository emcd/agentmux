//! Relay-scope serialization of identity-admin (`new peer` / `change psk`) store
//! transactions: concurrent operations against one shared serving context must
//! not interleave store persists and credential renames, so the final canonical
//! credential authenticates and no unrelated registration is lost.

use std::sync::{Arc, Barrier};
use std::thread;

use agentmux::runtime::paths::{BundleRuntimePaths, session_identity_psk_path};
use serde_json::json;
use tempfile::TempDir;

use super::*;

// Rotating one session (to config) while concurrently registering an unrelated
// principal must serialize: both mutations survive in the final store, and the
// rotated credential written to the session's canonical path authenticates.
#[test]
fn concurrent_admin_ops_serialize_without_losing_updates() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_concurrent";
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    // Declare two distinct `@GLOBAL` operators on the operator policy so the two
    // concurrent admin callers don't collide on a single operator identity claim
    // (the identity registry allows one live claim per principal id).
    let operator_one = "operatorone@GLOBAL";
    let operator_two = "operatortwo@GLOBAL";
    std::fs::write(
        configuration_root.join("users.toml"),
        format!(
            r#"
default-session = "{operator_one}"

[[sessions]]
id = "{operator_one}"
policy = "operator"

[sessions.ui]

[[sessions]]
id = "{operator_two}"
policy = "operator"

[sessions.ui]
"#
        ),
    )
    .expect("write two-operator users configuration");
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let catalog = single_bundle_catalog(&bundle_paths);
    let context = shared_serve_context(&configuration_root, &state_root, catalog);

    // Pre-register the session we will rotate (response mode).
    let session_id = format!("alpha@{bundle_name}");
    let seed = operator_request_on_context(
        &context,
        operator_one,
        json!({"operation": "new_peer", "principal_id": session_id}),
    );
    assert_eq!(
        seed["response"]["kind"], "new_peer",
        "seed registration failed: {seed:?}"
    );

    // Fire both admin ops so they contend on the transaction as closely as the
    // barrier allows: rotate `alpha` to its config credential AND register an
    // unrelated `bravo`.
    let barrier = Arc::new(Barrier::new(2));
    let rotate_id = session_id.clone();
    let rotate = {
        let context = Arc::clone(&context);
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            barrier.wait();
            operator_request_on_context(
                &context,
                operator_one,
                json!({
                    "operation": "change_psk",
                    "principal_id": rotate_id,
                    "destination": {"kind": "config"},
                }),
            )
        })
    };
    let unrelated_id = format!("bravo@{bundle_name}");
    let register = {
        let context = Arc::clone(&context);
        let barrier = Arc::clone(&barrier);
        let unrelated_id = unrelated_id.clone();
        thread::spawn(move || {
            barrier.wait();
            operator_request_on_context(
                &context,
                operator_two,
                json!({"operation": "new_peer", "principal_id": unrelated_id}),
            )
        })
    };

    let rotation = rotate.join().expect("join rotate thread");
    let registration = register.join().expect("join register thread");
    assert_eq!(
        rotation["response"]["kind"], "change_psk",
        "rotation failed: {rotation:?}"
    );
    assert_eq!(
        registration["response"]["kind"], "new_peer",
        "registration failed: {registration:?}"
    );

    // The rotated credential (written to config) must authenticate against the
    // FINAL store — i.e. the store persist and credential rename were not
    // interleaved with the concurrent registration's whole-store persist.
    let canonical = session_identity_psk_path(&state_root, bundle_name, "alpha");
    let rotated_psk = std::fs::read_to_string(&canonical).expect("read rotated config credential");
    let accepted = hello_first_frame(
        &configuration_root,
        &bundle_paths,
        &session_id,
        &rotated_psk,
        true,
    );
    assert_eq!(
        accepted["frame"], "hello_ack",
        "rotated credential must authenticate against the final store: {accepted:?}"
    );

    // The unrelated registration must not have been clobbered by the rotation's
    // whole-store persist: its issued credential authenticates.
    let bravo_psk = registration["response"]["psk"]
        .as_str()
        .expect("bravo psk in response")
        .to_string();
    let bravo_ok = hello_first_frame(
        &configuration_root,
        &bundle_paths,
        &unrelated_id,
        &bravo_psk,
        true,
    );
    assert_eq!(
        bravo_ok["frame"], "hello_ack",
        "unrelated registration must survive a concurrent rotation: {bravo_ok:?}"
    );
}
