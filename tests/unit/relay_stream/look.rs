//! Cross-bundle `Look` tests (todos/relay/69).
//!
//! These exercise the peer-bundle resolution path through `serve_connection`:
//! a bundle-bound session inspects a session in a peer bundle named by the
//! target's `@<bundle>` suffix. They assert the deliberately tightened
//! authorization (cross-bundle inspection requires `all:all`, not the
//! same-bundle `all:home`) and the resolution error codes
//! (`validation_unknown_bundle` / `validation_unknown_target`) that replaced the
//! retired `validation_cross_bundle_unsupported` stub.
//!
//! Peer choice tracks the pre-authorization capability gate: a coder-less UI
//! peer fails `can_be_looked` in the prepare stage, so it surfaces
//! `validation_unsupported_operation` — proving peer-bundle/member resolution
//! succeeded without a live tmux pane — while a tmux peer passes the gate and
//! keeps the authorization outcome observable for the denial test.

use super::*;

/// Overwrites the policy artifact so the `default` preset's `look` control uses
/// the given scope, leaving the other controls at workable defaults.
fn write_policies_with_look(configuration_root: &Path, look: &str) {
    std::fs::write(
        configuration_root.join("policies.toml"),
        format!(
            r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "all:home"
look = "{look}"
send = "all:home"
"#
        ),
    )
    .expect("write policies configuration");
}

/// Connects as a bundle-bound `alpha` session and issues one cross-bundle `look`
/// at `target_session`, returning the response frame.
fn cross_bundle_look(
    configuration_root: &Path,
    state_root: &Path,
    catalog: BundleCatalog,
    bundle_name: &str,
    target_session: &str,
) -> Value {
    let (mut client, join) =
        spawn_relay_connection_with_catalog(configuration_root, state_root, catalog);
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
                "operation": "look",
                "requester_session": "alpha",
                "target_session": target_session,
                "lines": 3,
            },
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown look client");
    join.join().expect("join relay thread");
    response
}

/// A cross-bundle target resolves through routing and the peer-bundle member
/// lookup; the UI peer then fails the `can_be_looked` capability gate. The
/// capability-specific code (not unknown-bundle/unknown-target) proves
/// peer-bundle and member resolution both succeeded.
#[test]
fn cross_bundle_look_permitted_under_all_scope_resolves_peer() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_a = "look_peer_a";
    let bundle_b = "look_peer_b";
    let configuration_root = write_bundle_configuration(&temporary, bundle_a);
    write_ui_bundle(&configuration_root, bundle_b);
    write_policies_with_look(&configuration_root, "all:all");
    let state_root = temporary.path().join("state");
    let paths_a = BundleRuntimePaths::resolve(&state_root, bundle_a).expect("bundle-a paths");
    let paths_b = BundleRuntimePaths::resolve(&state_root, bundle_b).expect("bundle-b paths");
    let catalog = multi_bundle_catalog(&[paths_a, paths_b]);

    let response = cross_bundle_look(
        &configuration_root,
        &state_root,
        catalog,
        bundle_a,
        &format!("agent@{bundle_b}"),
    );

    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"], "validation_unsupported_operation",
        "cross-bundle look resolves the peer member and reaches the capability gate"
    );
    assert_eq!(
        response["response"]["error"]["details"]["can_be_looked"],
        false
    );
}

/// `all:home` is sufficient for same-bundle inspection but deliberately
/// insufficient to cross the bundle boundary. The peer is a tmux member so the
/// pre-authorization capability gate passes and the policy denial surfaces.
#[test]
fn cross_bundle_look_denied_under_home_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_a = "look_home_a";
    let bundle_b = "look_home_b";
    let configuration_root = write_bundle_configuration(&temporary, bundle_a);
    write_tmux_bundle(&configuration_root, bundle_b);
    write_policies_with_look(&configuration_root, "all:home");
    let state_root = temporary.path().join("state");
    let paths_a = BundleRuntimePaths::resolve(&state_root, bundle_a).expect("bundle-a paths");
    let paths_b = BundleRuntimePaths::resolve(&state_root, bundle_b).expect("bundle-b paths");
    let catalog = multi_bundle_catalog(&[paths_a, paths_b]);

    let response = cross_bundle_look(
        &configuration_root,
        &state_root,
        catalog,
        bundle_a,
        &format!("agent@{bundle_b}"),
    );

    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );
    assert_eq!(
        response["response"]["error"]["details"]["capability"],
        "look.inspect"
    );
}

/// A target naming a bundle absent from the catalog is rejected fail-closed.
#[test]
fn cross_bundle_look_rejects_unknown_peer_bundle() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_a = "look_nobundle_a";
    let configuration_root = write_bundle_configuration(&temporary, bundle_a);
    write_policies_with_look(&configuration_root, "all:all");
    let state_root = temporary.path().join("state");
    let paths_a = BundleRuntimePaths::resolve(&state_root, bundle_a).expect("bundle-a paths");
    let catalog = multi_bundle_catalog(&[paths_a]);

    let response = cross_bundle_look(
        &configuration_root,
        &state_root,
        catalog,
        bundle_a,
        "agent@look_nobundle_missing",
    );

    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_unknown_bundle"
    );
}

/// A target session absent from an existing peer bundle is rejected, even when
/// the requester is permitted to look cross-bundle.
#[test]
fn cross_bundle_look_rejects_unknown_target_in_peer_bundle() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_a = "look_notarget_a";
    let bundle_b = "look_notarget_b";
    let configuration_root = write_bundle_configuration(&temporary, bundle_a);
    write_ui_bundle(&configuration_root, bundle_b);
    write_policies_with_look(&configuration_root, "all:all");
    let state_root = temporary.path().join("state");
    let paths_a = BundleRuntimePaths::resolve(&state_root, bundle_a).expect("bundle-a paths");
    let paths_b = BundleRuntimePaths::resolve(&state_root, bundle_b).expect("bundle-b paths");
    let catalog = multi_bundle_catalog(&[paths_a, paths_b]);

    let response = cross_bundle_look(
        &configuration_root,
        &state_root,
        catalog,
        bundle_a,
        &format!("ghost@{bundle_b}"),
    );

    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_unknown_target"
    );
}
