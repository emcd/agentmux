//! `List` with `namespace = "GLOBAL"`: registered relay-wide sessions,
//! excluded bundle sessions, process-only registration seeding, and the
//! declared-but-never-connected offline principal.

use std::io::BufReader;

use agentmux::runtime::paths::BundleRuntimePaths;
use tempfile::TempDir;

use super::*;

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
            "request": {"operation": "list", "requester_session": operator_id},
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
    let sessions = response["response"]["bundle"]["principals"]
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
    let sessions = response["response"]["bundle"]["principals"]
        .as_array()
        .expect("sessions array");
    assert!(
        !sessions.iter().any(|session| {
            session["id"] == "alpha" || session["id"] == format!("alpha@{bundle_name}")
        }),
        "GLOBAL list must not include bundle sessions"
    );
}

/// The process-only / `--no-autostart` host path registers a bundle's configured
/// members as static (offline) registry shells before any Hello, so the unified
/// registry holds every known principal even when no transport is started. This
/// exercises the exact public entry point that path calls.
#[test]
fn process_only_registration_seeds_configured_bundle_members() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_process_only_register";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);

    agentmux::relay::register_configured_bundle(&configuration_root, bundle_name)
        .expect("register configured bundle members");

    let ids = agentmux::relay::registered_principal_ids(bundle_name);
    assert!(
        ids.iter().any(|id| id == &format!("alpha@{bundle_name}")),
        "configured member alpha is a static registry shell before any Hello"
    );
    assert!(
        ids.iter().any(|id| id == &format!("bravo@{bundle_name}")),
        "configured member bravo is a static registry shell before any Hello"
    );
}

/// A relay-wide principal declared in `users.toml` but never connected is listed
/// in the `GLOBAL` namespace with `ready = false` — offline is a state, not
/// absence — rather than being filtered out. The declared operator id is unique
/// per test (`global_user_id`) and is never connected here, so its offline state
/// is stable under the process-wide registry shared across parallel tests.
#[test]
fn list_global_namespace_includes_declared_offline_relay_wide_principal() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_global_list_offline";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let operator_id = global_user_id(bundle_name);

    // Seed the declared relay-wide principal as a static (offline) registry entry,
    // as the host startup path does, without connecting it.
    agentmux::relay::register_configured_relay_wide_principals(&configuration_root)
        .expect("register declared relay-wide principals");

    let response = bundle_session_list_with_namespace(
        &configuration_root,
        &bundle_paths,
        bundle_name,
        "GLOBAL",
    );

    assert_eq!(response["response"]["kind"], "list");
    assert_eq!(response["response"]["bundle"]["id"], "GLOBAL");
    let sessions = response["response"]["bundle"]["principals"]
        .as_array()
        .expect("sessions array");
    let operator = sessions
        .iter()
        .find(|session| session["id"] == operator_id)
        .expect("declared offline operator present in GLOBAL list");
    assert_eq!(
        operator["ready"], false,
        "a declared-but-never-connected relay-wide principal is listed offline"
    );
    assert_eq!(operator["transport"], "ui");
}
