//! Relay-wide and cross-bundle `raww` routing, including the bare-target
//! rejection and the `home`-vs-`all` raww-scope matrix.

use agentmux::configuration::ConfigurationRoots;
use std::{io::BufReader, path::Path};

use agentmux::{relay::BundleCatalog, runtime::paths::BundleRuntimePaths};
use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;

/// Connects as the bundle's `@GLOBAL` operator (a relay-wide principal with no
/// bound bundle), sends one `raww` request to the given target, and returns the
/// response frame.
fn relay_wide_operator_raww(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    target: &str,
) -> Value {
    let (mut client, join) = spawn_relay_connection(configuration_roots, bundle_paths);
    let mut reader = BufReader::new(client.try_clone().expect("clone stream"));
    let operator_id = global_user_id(bundle_name);
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
            "request": {
                "operation": "raww",
                "requester_session": operator_id,
                "target_session": target,
                "text": "operator raw input",
            },
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown operator stream");
    join.join().expect("join relay thread");
    response
}

/// Connects as a bundle-bound `alpha` session and sends one `raww` request to a
/// same-bundle target, returning the response frame.
fn bundle_session_raww(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    target: &str,
) -> Value {
    let (mut client, join) = spawn_relay_connection(configuration_roots, bundle_paths);
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
                "operation": "raww",
                "requester_session": "alpha",
                "target_session": target,
                "text": "alpha raw input",
            },
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown session stream");
    join.join().expect("join relay thread");
    response
}

/// Connects as a bundle-bound `alpha` session in `bundle_name` and issues one
/// cross-bundle `raww` at `target_session`, returning the response frame. Unlike
/// `bundle_session_raww`, this uses a multi-bundle catalog so the peer target's
/// bundle is resolvable.
fn cross_bundle_raww(
    configuration_roots: &ConfigurationRoots,
    state_root: &Path,
    catalog: BundleCatalog,
    bundle_name: &str,
    target_session: &str,
) -> Value {
    let (mut client, join) =
        spawn_relay_connection_with_catalog(configuration_roots, state_root, catalog);
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
                "operation": "raww",
                "requester_session": "alpha",
                "target_session": target_session,
                "text": "alpha raw input",
            },
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown cross-bundle raww client");
    join.join().expect("join relay thread");
    response
}

/// A relay-wide (`@GLOBAL`) principal rawws to a bundle target named by its
/// `@<bundle>` suffix. The relay infers the routing bundle from the single
/// target's suffix (mirroring `Send`) and, under `raww = all`, authorizes the
/// cross-namespace reach: a `@GLOBAL` operator's home is `GLOBAL`, so reaching
/// into a bundle is cross-namespace and requires `all`. Routing and
/// authorization succeed, so the request reaches async dispatch and returns a
/// queued raww response; the transport outcome (the absent harness pane) is
/// reported out-of-band via a delivery_outcome event, not in this response.
#[test]
fn relay_wide_raww_routes_to_bundle_target_by_suffix() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_relay_wide_raww_to_bundle";
    let configuration_roots = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_roots, "default", bundle_name);
    write_policies_with_raww(&configuration_roots, "all");
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response = relay_wide_operator_raww(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &format!("alpha@{bundle_name}"),
    );

    // Routing resolved the bundle and authorization passed at `all`, so the
    // request reaches async dispatch and returns immediately as queued. Neither
    // a routing rejection (`validation_missing_routing_namespace`) nor an authz
    // denial (`authorization_forbidden`) must appear.
    assert_eq!(response["response"]["kind"], "raww");
    assert_eq!(response["response"]["status"], "queued");
}

/// A relay-wide (`@GLOBAL`) principal rawwing into a bundle under only `home`
/// raww scope is denied: reaching a bundle is cross-namespace for a `@GLOBAL`
/// principal (its home is `GLOBAL`), so it requires `all`. Mirrors
/// `relay_wide_send_into_bundle_denied_under_home_scope`.
#[test]
fn relay_wide_raww_into_bundle_denied_under_home_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_relay_wide_raww_home_denied";
    let configuration_roots = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_roots, "default", bundle_name);
    write_policies_with_raww(&configuration_roots, "home");
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response = relay_wide_operator_raww(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &format!("alpha@{bundle_name}"),
    );

    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );
    assert_eq!(
        response["response"]["error"]["details"]["capability"],
        "raww.write"
    );
}

/// A bundle-bound session rawwing a same-bundle peer is unaffected by the
/// cross-namespace threshold: it is a home-tier act and succeeds under `home`,
/// reaching async dispatch and returning a queued raww response (the transport
/// outcome against the absent harness pane is reported out-of-band).
#[test]
fn same_bundle_raww_permitted_under_home_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_same_bundle_raww_home";
    let configuration_roots = write_bundle_configuration(&temporary, bundle_name);
    write_policies_with_raww(&configuration_roots, "home");
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response = bundle_session_raww(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &format!("bravo@{bundle_name}"),
    );

    assert_eq!(response["response"]["kind"], "raww");
    assert_eq!(response["response"]["status"], "queued");
}

/// A relay-wide principal whose raww target is bare (no `@<namespace>` suffix) is
/// rejected with `validation_unqualified_target`. The relay requires
/// fully-qualified targets and no longer infers a routing bundle from the sender
/// or the wire namespace, so the bare target is rejected uniformly at the
/// config-free resolution stage rather than via the old
/// `validation_missing_routing_namespace` routing-bundle artifact.
#[test]
fn relay_wide_raww_with_bare_target_is_rejected() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_relay_wide_raww_bare";
    let configuration_roots = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_roots, "default", bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response =
        relay_wide_operator_raww(&configuration_roots, &bundle_paths, bundle_name, "alpha");

    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_unqualified_target"
    );
}

/// Cross-namespace raww sender fix (routing-layer Steps 6-7): a bundle-bound
/// session reaching a session in a peer bundle is authorized in its **home**
/// namespace and resolves the peer member — rather than failing because the relay
/// dispatched the raww through a borrowed (target) bundle, where the home session
/// is unknown. The request resolves through routing and the peer-bundle member
/// lookup; the UI peer then fails the `can_be_written` capability gate. The
/// capability-specific code (not unknown-bundle/unknown-target) proves
/// peer-bundle and member resolution both succeeded.
#[test]
fn cross_bundle_raww_permitted_under_all_scope_resolves_peer() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_a = "raww_peer_a";
    let bundle_b = "raww_peer_b";
    let configuration_roots = write_bundle_configuration(&temporary, bundle_a);
    write_ui_bundle(&configuration_roots, bundle_b);
    write_policies_with_raww(&configuration_roots, "all");
    let state_root = temporary.path().join("state");
    let paths_a = BundleRuntimePaths::resolve(&state_root, bundle_a).expect("bundle-a paths");
    let paths_b = BundleRuntimePaths::resolve(&state_root, bundle_b).expect("bundle-b paths");
    let catalog = multi_bundle_catalog(&[paths_a, paths_b]);

    let response = cross_bundle_raww(
        &configuration_roots,
        &state_root,
        catalog,
        bundle_a,
        &format!("agent@{bundle_b}"),
    );

    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"], "validation_unsupported_operation",
        "cross-bundle raww resolves the peer member and reaches the capability gate"
    );
    assert_eq!(
        response["response"]["error"]["details"]["can_be_written"],
        false
    );
}

/// `home` is sufficient for same-namespace raww but deliberately insufficient
/// to cross the bundle boundary. The requester is authorized in its home
/// namespace (where its `raww` control resolves) and denied — it is no longer
/// dispatched through the target bundle, so the denial is a real
/// `authorization_forbidden`, not an unknown-target artifact. The peer is a tmux
/// member so the pre-authorization capability gate passes and the denial
/// surfaces.
#[test]
fn cross_bundle_raww_denied_under_home_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_a = "raww_home_a";
    let bundle_b = "raww_home_b";
    let configuration_roots = write_bundle_configuration(&temporary, bundle_a);
    write_tmux_bundle(&configuration_roots, bundle_b);
    write_policies_with_raww(&configuration_roots, "home");
    let state_root = temporary.path().join("state");
    let paths_a = BundleRuntimePaths::resolve(&state_root, bundle_a).expect("bundle-a paths");
    let paths_b = BundleRuntimePaths::resolve(&state_root, bundle_b).expect("bundle-b paths");
    let catalog = multi_bundle_catalog(&[paths_a, paths_b]);

    let response = cross_bundle_raww(
        &configuration_roots,
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
        "raww.write"
    );
}
