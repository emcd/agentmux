//! Cross-bundle `List` tests (todos/relay/73, Step 3).
//!
//! These exercise the routing/authorization spine's separation of the
//! requester's home (dispatch) bundle from the enumerated bundle: a bundle-bound
//! session lists a peer bundle named by the wire `namespace` selector. Before the
//! dispatch layer this failed with `validation_unknown_sender`, because the
//! requester was looked up in the enumerated bundle's members rather than its own
//! home bundle. They assert the uniform cross-bundle threshold (cross-bundle list
//! requires `all:all`, same-bundle list needs only `all:home`) and that the
//! enumerated bundle's sessions are returned.

use super::*;

/// Connects as a bundle-bound `alpha` session and issues one `list` whose wire
/// frame carries the given routing `namespace`, returning the response frame.
fn cross_bundle_list(
    configuration_root: &Path,
    state_root: &Path,
    catalog: BundleCatalog,
    bundle_name: &str,
    namespace: &str,
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
            "namespace": namespace,
            "request": {"operation": "list", "requester_session": "alpha"},
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown list client");
    join.join().expect("join relay thread");
    response
}

/// A session permitted cross-bundle (`list = all:all`) enumerates a peer bundle's
/// sessions rather than being rejected as an unknown sender.
#[test]
fn cross_bundle_list_permitted_under_all_scope_enumerates_peer() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_a = "list_peer_a";
    let bundle_b = "list_peer_b";
    let configuration_root = write_bundle_configuration(&temporary, bundle_a);
    write_ui_bundle(&configuration_root, bundle_b);
    write_policies_with_list(&configuration_root, "all:all");
    let state_root = temporary.path().join("state");
    let paths_a = BundleRuntimePaths::resolve(&state_root, bundle_a).expect("bundle-a paths");
    let paths_b = BundleRuntimePaths::resolve(&state_root, bundle_b).expect("bundle-b paths");
    let catalog = multi_bundle_catalog(&[paths_a, paths_b]);

    let response = cross_bundle_list(
        &configuration_root,
        &state_root,
        catalog,
        bundle_a,
        bundle_b,
    );

    assert_eq!(response["response"]["kind"], "list");
    assert_eq!(response["response"]["bundle"]["id"], bundle_b);
    let sessions = response["response"]["bundle"]["sessions"]
        .as_array()
        .expect("sessions array");
    assert!(
        sessions
            .iter()
            .any(|session| session["id"] == format!("agent@{bundle_b}")),
        "peer bundle's session is enumerated"
    );
}

/// `all:home` is sufficient for same-bundle listing but deliberately insufficient
/// to enumerate a peer bundle.
#[test]
fn cross_bundle_list_denied_under_home_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_a = "list_home_a";
    let bundle_b = "list_home_b";
    let configuration_root = write_bundle_configuration(&temporary, bundle_a);
    write_ui_bundle(&configuration_root, bundle_b);
    write_policies_with_list(&configuration_root, "all:home");
    let state_root = temporary.path().join("state");
    let paths_a = BundleRuntimePaths::resolve(&state_root, bundle_a).expect("bundle-a paths");
    let paths_b = BundleRuntimePaths::resolve(&state_root, bundle_b).expect("bundle-b paths");
    let catalog = multi_bundle_catalog(&[paths_a, paths_b]);

    let response = cross_bundle_list(
        &configuration_root,
        &state_root,
        catalog,
        bundle_a,
        bundle_b,
    );

    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );
    assert_eq!(
        response["response"]["error"]["details"]["capability"],
        "list.read"
    );
}

/// Same-bundle listing is unaffected by the cross-bundle threshold: a session
/// lists its own bundle by name under `all:home`.
#[test]
fn same_bundle_list_permitted_under_home_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "list_same_home";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_policies_with_list(&configuration_root, "all:home");
    let state_root = temporary.path().join("state");
    let paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let catalog = multi_bundle_catalog(&[paths]);

    let response = cross_bundle_list(
        &configuration_root,
        &state_root,
        catalog,
        bundle_name,
        bundle_name,
    );

    assert_eq!(response["response"]["kind"], "list");
    assert_eq!(response["response"]["bundle"]["id"], bundle_name);
}

/// Connects as the given relay-wide operator and lists the given `namespace`.
fn relay_wide_list(
    configuration_root: &Path,
    state_root: &Path,
    catalog: BundleCatalog,
    operator_id: &str,
    namespace: &str,
) -> Value {
    let (mut client, join) =
        spawn_relay_connection_with_catalog(configuration_root, state_root, catalog);
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
            "namespace": namespace,
            "request": {"operation": "list", "requester_session": operator_id},
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown list client");
    join.join().expect("join relay thread");
    response
}

/// A relay-wide operator's home namespace is `GLOBAL`. Listing a bundle is
/// therefore cross-namespace and permitted only under `all:all`.
#[test]
fn relay_wide_list_of_bundle_permitted_under_all_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "list_relay_wide_all";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    write_policies_with_list(&configuration_root, "all:all");
    let state_root = temporary.path().join("state");
    let paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let catalog = multi_bundle_catalog(&[paths]);
    let operator_id = global_user_id(bundle_name);

    let response = relay_wide_list(
        &configuration_root,
        &state_root,
        catalog,
        &operator_id,
        bundle_name,
    );

    assert_eq!(response["response"]["kind"], "list");
    assert_eq!(response["response"]["bundle"]["id"], bundle_name);
}

/// The same relay-wide operator under only `all:home` is denied a bundle list:
/// `all:home` confers authority only within its own (`GLOBAL`) namespace.
#[test]
fn relay_wide_list_of_bundle_denied_under_home_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "list_relay_wide_home";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    write_policies_with_list(&configuration_root, "all:home");
    let state_root = temporary.path().join("state");
    let paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let catalog = multi_bundle_catalog(&[paths]);
    let operator_id = global_user_id(bundle_name);

    let response = relay_wide_list(
        &configuration_root,
        &state_root,
        catalog,
        &operator_id,
        bundle_name,
    );

    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );
    assert_eq!(
        response["response"]["error"]["details"]["capability"],
        "list.read"
    );
}

/// A `namespace` naming a bundle absent from the catalog is rejected fail-closed.
#[test]
fn cross_bundle_list_rejects_unknown_namespace() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_a = "list_nobundle_a";
    let configuration_root = write_bundle_configuration(&temporary, bundle_a);
    write_policies_with_list(&configuration_root, "all:all");
    let state_root = temporary.path().join("state");
    let paths_a = BundleRuntimePaths::resolve(&state_root, bundle_a).expect("bundle-a paths");
    let catalog = multi_bundle_catalog(&[paths_a]);

    let response = cross_bundle_list(
        &configuration_root,
        &state_root,
        catalog,
        bundle_a,
        "list_nobundle_missing",
    );

    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_unknown_bundle"
    );
}
