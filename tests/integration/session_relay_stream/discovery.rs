//! Cross-relay and local discovery over `serve_connection`:
//! - `list.relays` enumerates the configured outbound aliases (sorted, no dial)
//!   and requires the requester's `list` control at `all`.
//! - Local `list.namespaces` reflects the requester's `list` visibility.
//! - Origin-side foreign discovery gates on `all` before contacting a peer and
//!   forwards a request with the `relay` selector cleared and no `on_behalf_of`,
//!   propagating peer-authored results and typed peer errors unchanged.
//! - Receiving-side (ingress) discovery filters the relay's own catalog and
//!   registry by the authenticated peer principal's ingress scope: a namespace
//!   scope exposes the complete namespace, an exact-principal scope exposes a
//!   `principals_partial` subset, an out-of-scope or empty namespace discloses no
//!   existence, and an absent scope denies.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use agentmux::relay::{
    BundleCatalog, ConnectionDrainCoordinator, ConnectionServeContext, PeerConfiguration,
    PeerConnectionManager, serve_connection,
};
use agentmux::runtime::paths::BundleRuntimePaths;
use serde_json::{Value, json};
use tempfile::TempDir;
use uuid::Uuid;

use super::{
    global_user_id, hello_payload, ingress_request_response, read_json, send_json,
    spawn_answering_peer, spawn_relay_stream, spawn_relay_stream_with_peer,
    unique_relay_principal_id, write_bundle_configuration, write_ingress_peer_store,
    write_peer_credential,
};

// Serves one origin relay connection with an explicit bundle catalog and set of
// configured peer aliases. Aliases point at nonexistent sockets: discovery paths
// that must not dial (relay enumeration) succeed regardless, and paths that fail
// authorization before dialing never reach them.
fn spawn_origin_relay(
    configuration_root: &Path,
    state_root: &Path,
    catalog_paths: Vec<BundleRuntimePaths>,
    aliases: &[&str],
) -> (UnixStream, thread::JoinHandle<()>) {
    let (server_stream, client_stream) = UnixStream::pair().expect("unix stream pair");
    let root = configuration_root.to_path_buf();
    let state = state_root.to_path_buf();
    let catalog = BundleCatalog::from_paths(catalog_paths);
    let peers: Vec<PeerConfiguration> = aliases
        .iter()
        .map(|alias| PeerConfiguration {
            alias: (*alias).to_string(),
            address: format!("/tmp/agentmux-nonexistent-{alias}.sock"),
            connect_as: "origin-relay".to_string(),
        })
        .collect();
    let handle = thread::spawn(move || {
        server_stream
            .set_nonblocking(true)
            .expect("non-blocking server stream");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build current-thread runtime");
        runtime
            .block_on(async move {
                let stream = tokio::net::UnixStream::from_std(server_stream)?;
                let manager = Arc::new(PeerConnectionManager::from_configuration(&state, &peers));
                let serve_context = ConnectionServeContext::new(
                    root,
                    state.clone(),
                    catalog,
                    manager,
                    peers.iter().map(|peer| peer.alias.clone()).collect(),
                    false,
                    Duration::from_secs(2),
                );
                serve_connection(
                    stream,
                    &serve_context,
                    ConnectionDrainCoordinator::new().register_worker(),
                )
                .await
            })
            .expect("serve connection");
    });
    (client_stream, handle)
}

// Hellos as `hello_session` on `client`, sends one discovery `request`, returns
// the decoded response frame, and tears the connection down.
fn discovery_exchange(
    client: UnixStream,
    handle: thread::JoinHandle<()>,
    bundle_name: &str,
    hello_session: &str,
    request: Value,
) -> Value {
    let reader_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(reader_stream);
    let mut client = client;
    send_json(&mut client, hello_payload(bundle_name, hello_session));
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");
    let request_id = format!("req-{}", Uuid::new_v4().simple());
    send_json(
        &mut client,
        json!({"frame": "request", "request_id": request_id, "request": request}),
    );
    let response = read_json(&mut reader);
    client.shutdown(std::net::Shutdown::Both).ok();
    handle.join().expect("join relay stream");
    response
}

fn origin_fixture() -> (TempDir, String, std::path::PathBuf, BundleRuntimePaths) {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    (temporary, bundle_name, configuration_root, bundle_paths)
}

#[test]
fn list_relays_enumerates_configured_aliases_sorted_without_dialing() {
    let (_temporary, bundle_name, configuration_root, bundle_paths) = origin_fixture();
    let state_root = bundle_paths.state_root.clone();
    // Two aliases in reverse order; nonexistent sockets prove no dial occurs.
    let (client, handle) = spawn_origin_relay(
        &configuration_root,
        &state_root,
        vec![bundle_paths.clone()],
        &["west", "east"],
    );
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        global_user_id(&bundle_name).as_str(),
        json!({"operation": "list_relays"}),
    );
    assert_eq!(response["response"]["kind"], "list_relays");
    let relays = response["response"]["relays"]
        .as_array()
        .expect("relays array");
    assert_eq!(relays.len(), 2);
    assert_eq!(relays[0]["alias"], "east");
    assert_eq!(relays[1]["alias"], "west");
}

#[test]
fn list_relays_returns_empty_when_no_peers_configured() {
    let (_temporary, bundle_name, configuration_root, bundle_paths) = origin_fixture();
    let state_root = bundle_paths.state_root.clone();
    let (client, handle) = spawn_origin_relay(
        &configuration_root,
        &state_root,
        vec![bundle_paths.clone()],
        &[],
    );
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        global_user_id(&bundle_name).as_str(),
        json!({"operation": "list_relays"}),
    );
    assert_eq!(response["response"]["kind"], "list_relays");
    assert!(
        response["response"]["relays"]
            .as_array()
            .expect("relays array")
            .is_empty()
    );
}

#[test]
fn list_relays_requires_all_scope() {
    let (_temporary, bundle_name, configuration_root, bundle_paths) = origin_fixture();
    let state_root = bundle_paths.state_root.clone();
    // `alpha` resolves the default policy (`list = home`), below the `all` tier.
    let (client, handle) = spawn_origin_relay(
        &configuration_root,
        &state_root,
        vec![bundle_paths.clone()],
        &["west"],
    );
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        "alpha",
        json!({"operation": "list_relays"}),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );
}

#[test]
fn local_namespace_discovery_follows_list_scope() {
    let (temporary, bundle_name, configuration_root, bundle_paths) = origin_fixture();
    let state_root = bundle_paths.state_root.clone();
    // A second bundle the requester is not a member of, loaded into the catalog.
    let other_bundle = format!("other-{}", Uuid::new_v4().simple());
    std::fs::write(
        configuration_root
            .join("bundles")
            .join(format!("{other_bundle}.toml")),
        "\nformat-version = 1\n\n[[sessions]]\nid = \"alpha\"\nname = \"Alpha\"\ndirectory = \"/tmp\"\ncoder = \"shell\"\n",
    )
    .expect("write second bundle");
    let other_paths = BundleRuntimePaths::resolve(&state_root, other_bundle.as_str())
        .expect("other bundle paths");
    let catalog = vec![bundle_paths.clone(), other_paths];

    // The `all`-tier operator sees every configured bundle namespace plus GLOBAL.
    let (client, handle) =
        spawn_origin_relay(&configuration_root, &state_root, catalog.clone(), &[]);
    let operator = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        global_user_id(&bundle_name).as_str(),
        json!({"operation": "discover_namespaces"}),
    );
    assert_eq!(operator["response"]["kind"], "discover_namespaces");
    assert!(operator["response"].get("relay").is_none());
    let namespaces: Vec<&str> = operator["response"]["namespaces"]
        .as_array()
        .expect("namespaces array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(namespaces.contains(&"GLOBAL"));
    assert!(namespaces.contains(&bundle_name.as_str()));
    assert!(namespaces.contains(&other_bundle.as_str()));

    // A home-tier member sees only its home namespace and GLOBAL.
    let (client, handle) = spawn_origin_relay(&configuration_root, &state_root, catalog, &[]);
    let member = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        "alpha",
        json!({"operation": "discover_namespaces"}),
    );
    let member_namespaces: Vec<&str> = member["response"]["namespaces"]
        .as_array()
        .expect("namespaces array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(member_namespaces.contains(&"GLOBAL"));
    assert!(member_namespaces.contains(&bundle_name.as_str()));
    assert!(!member_namespaces.contains(&other_bundle.as_str()));
    drop(temporary);
}

#[test]
fn foreign_discovery_requires_all_scope_before_peer_contact() {
    let (_temporary, bundle_name, configuration_root, bundle_paths) = origin_fixture();
    write_peer_credential(&bundle_paths.state_root, "west", "peer-secret");
    // No listener bound: if authorization did not fail first, the forward would
    // dial this dead socket. `alpha` holds `list = home`, so the origin denies.
    let peer_socket = configuration_root.join("west.sock");
    let (client, handle) = spawn_relay_stream_with_peer(
        &configuration_root,
        &bundle_paths,
        "west",
        "origin-relay",
        &peer_socket,
    );
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        "alpha",
        json!({"operation": "discover_namespaces", "relay": "west"}),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );
}

#[test]
fn foreign_namespace_discovery_forwards_without_origin_selectors() {
    let (temporary, bundle_name, configuration_root, bundle_paths) = origin_fixture();
    write_peer_credential(&bundle_paths.state_root, "west", "peer-secret");
    let peer_socket = temporary.path().join("west.sock");
    let observed = spawn_answering_peer(
        &peer_socket,
        json!({
            "kind": "discover_namespaces",
            "schema_version": "test",
            "namespaces": ["myapp"],
        }),
    );
    let (client, handle) = spawn_relay_stream_with_peer(
        &configuration_root,
        &bundle_paths,
        "west",
        "origin-relay",
        &peer_socket,
    );
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        global_user_id(&bundle_name).as_str(),
        json!({"operation": "discover_namespaces", "relay": "west"}),
    );
    // The peer-authored namespaces propagate unchanged.
    assert_eq!(response["response"]["kind"], "discover_namespaces");
    assert_eq!(response["response"]["namespaces"][0], "myapp");
    // The forwarded wire request carries no origin-local relay selector and no
    // on_behalf_of, and cannot trigger an onward peer lookup.
    let forwarded = observed
        .recv_timeout(Duration::from_secs(2))
        .expect("peer observed forwarded request");
    assert_eq!(forwarded["request"]["operation"], "discover_namespaces");
    assert!(forwarded["request"].get("relay").is_none());
    assert!(forwarded["request"].get("on_behalf_of").is_none());
}

#[test]
fn foreign_principal_discovery_propagates_peer_bundles_unchanged() {
    let (temporary, bundle_name, configuration_root, bundle_paths) = origin_fixture();
    write_peer_credential(&bundle_paths.state_root, "west", "peer-secret");
    let peer_socket = temporary.path().join("west.sock");
    // A principal-scoped subset the peer authored, foreign id `myapp`.
    let observed = spawn_answering_peer(
        &peer_socket,
        json!({
            "kind": "discover_principals",
            "schema_version": "test",
            "bundles": [{
                "id": "myapp",
                "hosted": true,
                "state": "up",
                "startup_failure_count": 0,
                "recent_startup_failures": [],
                "principals": [{"id": "agent@myapp", "transport": "tmux", "ready": true}],
                "principals_partial": true,
            }],
        }),
    );
    let (client, handle) = spawn_relay_stream_with_peer(
        &configuration_root,
        &bundle_paths,
        "west",
        "origin-relay",
        &peer_socket,
    );
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        global_user_id(&bundle_name).as_str(),
        json!({"operation": "discover_principals", "relay": "west", "namespace": "myapp"}),
    );
    assert_eq!(response["response"]["kind"], "discover_principals");
    let bundles = response["response"]["bundles"]
        .as_array()
        .expect("bundles array");
    assert_eq!(bundles.len(), 1);
    // The foreign id is not rewritten and the partial marker is preserved.
    assert_eq!(bundles[0]["id"], "myapp");
    assert_eq!(bundles[0]["principals_partial"], true);
    let forwarded = observed
        .recv_timeout(Duration::from_secs(2))
        .expect("peer observed forwarded request");
    assert_eq!(forwarded["request"]["operation"], "discover_principals");
    assert_eq!(forwarded["request"]["namespace"], "myapp");
    assert!(forwarded["request"].get("relay").is_none());
    assert!(forwarded["request"].get("on_behalf_of").is_none());
}

#[test]
fn foreign_discovery_propagates_peer_authorization_denial() {
    let (temporary, bundle_name, configuration_root, bundle_paths) = origin_fixture();
    write_peer_credential(&bundle_paths.state_root, "west", "peer-secret");
    let peer_socket = temporary.path().join("west.sock");
    let _observed = spawn_answering_peer(
        &peer_socket,
        json!({
            "kind": "error",
            "error": {
                "code": "authorization_forbidden",
                "message": "cross-relay discovery denied by peer relay ingress scope",
            },
        }),
    );
    let (client, handle) = spawn_relay_stream_with_peer(
        &configuration_root,
        &bundle_paths,
        "west",
        "origin-relay",
        &peer_socket,
    );
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        global_user_id(&bundle_name).as_str(),
        json!({"operation": "discover_namespaces", "relay": "west"}),
    );
    // The peer's typed denial propagates unchanged.
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );
}

#[test]
fn foreign_discovery_reports_unknown_alias_typed_error() {
    let (_temporary, bundle_name, configuration_root, bundle_paths) = origin_fixture();
    write_peer_credential(&bundle_paths.state_root, "west", "peer-secret");
    let peer_socket = configuration_root.join("west.sock");
    let (client, handle) = spawn_relay_stream_with_peer(
        &configuration_root,
        &bundle_paths,
        "west",
        "origin-relay",
        &peer_socket,
    );
    // The origin is `all`-authorized, but names an alias absent from `[[peers]]`.
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        global_user_id(&bundle_name).as_str(),
        json!({"operation": "discover_namespaces", "relay": "nonexistent"}),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_unknown_peer"
    );
}

#[test]
fn ingress_namespace_discovery_derives_from_receiver_catalog() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    // A namespace-wide scope covers every principal in the local bundle.
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some(bundle_name.as_str()),
    );
    let response = ingress_request_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        json!({"operation": "discover_namespaces"}),
    );
    assert_eq!(response["response"]["kind"], "discover_namespaces");
    // The receiving relay authors namespace ids from its own catalog.
    let namespaces: Vec<&str> = response["response"]["namespaces"]
        .as_array()
        .expect("namespaces array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(namespaces, vec![bundle_name.as_str()]);
}

#[test]
fn ingress_namespace_discovery_denies_absent_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    write_ingress_peer_store(&bundle_paths.state_root, relay_principal_id.as_str(), None);
    let response = ingress_request_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        json!({"operation": "discover_namespaces"}),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );
}

#[test]
fn ingress_namespace_discovery_omits_empty_namespace() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    // The scope names a namespace this relay does not host: it must not appear,
    // revealing nothing about whether it exists.
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some("phantom"),
    );
    let response = ingress_request_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        json!({"operation": "discover_namespaces"}),
    );
    assert_eq!(response["response"]["kind"], "discover_namespaces");
    assert!(
        response["response"]["namespaces"]
            .as_array()
            .expect("namespaces array")
            .is_empty()
    );
}

#[test]
fn ingress_principal_discovery_returns_complete_namespace_under_namespace_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some(bundle_name.as_str()),
    );
    let response = ingress_request_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        json!({"operation": "discover_principals", "namespace": bundle_name}),
    );
    assert_eq!(response["response"]["kind"], "discover_principals");
    let bundles = response["response"]["bundles"]
        .as_array()
        .expect("bundles array");
    assert_eq!(bundles.len(), 1);
    assert_eq!(bundles[0]["id"], bundle_name.as_str());
    // Both configured members are exposed; a complete listing omits the marker.
    let principals = bundles[0]["principals"]
        .as_array()
        .expect("principals array");
    assert_eq!(principals.len(), 2);
    assert!(bundles[0].get("principals_partial").is_none());
}

#[test]
fn ingress_principal_discovery_marks_subset_under_exact_principal_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    // An exact-principal scope exposes only that principal.
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some(format!("alpha@{bundle_name}").as_str()),
    );
    let response = ingress_request_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        json!({"operation": "discover_principals", "namespace": bundle_name}),
    );
    assert_eq!(response["response"]["kind"], "discover_principals");
    let bundles = response["response"]["bundles"]
        .as_array()
        .expect("bundles array");
    assert_eq!(bundles.len(), 1);
    let principals = bundles[0]["principals"]
        .as_array()
        .expect("principals array");
    assert_eq!(principals.len(), 1);
    assert_eq!(principals[0]["id"], format!("alpha@{bundle_name}"));
    // The subset omits `bravo`, so the marker is set.
    assert_eq!(bundles[0]["principals_partial"], true);
}

#[test]
fn ingress_principal_discovery_denies_out_of_scope_namespace_without_disclosure() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    // Scope covers a different namespace, so a request for the local bundle is
    // denied uniformly, disclosing no existence.
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some("some-other-bundle"),
    );
    let response = ingress_request_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        json!({"operation": "discover_principals", "namespace": bundle_name}),
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
    // The denial names no namespace, so it cannot confirm existence.
    assert!(
        response["response"]["error"]["details"]
            .get("namespace")
            .is_none()
    );
}

#[test]
fn ingress_principal_discovery_denies_absent_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    // A peer registered without an ingress scope is fail-closed for principal
    // discovery, exactly as for namespace discovery.
    write_ingress_peer_store(&bundle_paths.state_root, relay_principal_id.as_str(), None);
    let response = ingress_request_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        json!({"operation": "discover_principals", "namespace": bundle_name}),
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
fn ingress_exact_scope_suppresses_stale_out_of_scope_startup_history() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    // Reconfigure the bundle to the single member `alpha`. `bravo` is no longer a
    // configured member, but a stale startup-failure record for it survives on
    // disk (startup history is keyed by session id, independent of membership).
    std::fs::write(
        configuration_root
            .join("bundles")
            .join(format!("{bundle_name}.toml")),
        "\nformat-version = 1\n\n[[sessions]]\nid = \"alpha\"\nname = \"Alpha\"\ndirectory = \"/tmp\"\ncoder = \"shell\"\n",
    )
    .expect("rewrite sole-alpha bundle");
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    write_startup_failure(&bundle_paths.runtime_directory, "bravo");
    let relay_principal_id = unique_relay_principal_id();
    // Exact-principal scope for the sole configured member: nothing is omitted, so
    // the partial marker stays unset — but the exact-principal grant must still
    // suppress the stale out-of-scope history rather than leak it because
    // `len == total`.
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some(format!("alpha@{bundle_name}").as_str()),
    );
    let response = ingress_request_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        json!({"operation": "discover_principals", "namespace": bundle_name}),
    );
    assert_eq!(response["response"]["kind"], "discover_principals");
    let bundle = &response["response"]["bundles"][0];
    let principals = bundle["principals"].as_array().expect("principals array");
    assert_eq!(principals.len(), 1);
    assert_eq!(principals[0]["id"], format!("alpha@{bundle_name}"));
    // No configured principal was omitted, so the partial marker is absent.
    assert!(bundle.get("principals_partial").is_none() || bundle["principals_partial"].is_null());
    // The exact-principal grant still suppresses the stale out-of-scope history
    // and every bundle diagnostic.
    assert_eq!(bundle["startup_failure_count"], 0);
    assert!(
        bundle["recent_startup_failures"]
            .as_array()
            .expect("failures array")
            .is_empty()
    );
    assert!(
        !serde_json::to_string(bundle)
            .expect("encode bundle")
            .contains("bravo"),
        "stale out-of-scope startup history must not leak: {bundle}"
    );
    drop(temporary);
}

#[test]
fn foreign_discovery_reports_unknown_peer_when_none_configured() {
    let (_temporary, bundle_name, configuration_root, bundle_paths) = origin_fixture();
    let state_root = bundle_paths.state_root.clone();
    // No peers are configured at all. An `all`-scoped requester still gets a typed
    // unknown-peer error, distinct from the unknown-alias case (which has a peer
    // configured under a different alias).
    let (client, handle) = spawn_origin_relay(
        &configuration_root,
        &state_root,
        vec![bundle_paths.clone()],
        &[],
    );
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        global_user_id(&bundle_name).as_str(),
        json!({"operation": "discover_namespaces", "relay": "west"}),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_unknown_peer"
    );
}

#[test]
fn foreign_discovery_reports_unreachable_on_peer_authentication_failure() {
    let (temporary, bundle_name, configuration_root, bundle_paths) = origin_fixture();
    write_peer_credential(&bundle_paths.state_root, "west", "peer-secret");
    let peer_socket = temporary.path().join("west.sock");
    // A peer that answers the Hello with the structured credential-rejection error
    // frame a real relay emits. A rejected handshake — like an unreachable
    // endpoint — surfaces as `runtime_peer_unavailable` per the peer connection
    // classification, so authentication failure is not a distinct code.
    spawn_rejecting_peer(&peer_socket);
    let (client, handle) = spawn_relay_stream_with_peer(
        &configuration_root,
        &bundle_paths,
        "west",
        "origin-relay",
        &peer_socket,
    );
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        global_user_id(&bundle_name).as_str(),
        json!({"operation": "discover_namespaces", "relay": "west"}),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "runtime_peer_unavailable"
    );
    drop(temporary);
}

#[test]
fn ingress_principal_subset_suppresses_out_of_scope_bundle_diagnostics() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    // A recorded startup failure for out-of-scope `bravo`, carrying its session
    // id, reason, and details. An `alpha`-only grant must not leak any of it.
    write_startup_failure(&bundle_paths.runtime_directory, "bravo");
    let relay_principal_id = unique_relay_principal_id();
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some(format!("alpha@{bundle_name}").as_str()),
    );
    let response = ingress_request_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        json!({"operation": "discover_principals", "namespace": bundle_name}),
    );
    assert_eq!(response["response"]["kind"], "discover_principals");
    let bundle = &response["response"]["bundles"][0];
    // Only the covered principal survives, and the subset is marked.
    let principals = bundle["principals"].as_array().expect("principals array");
    assert_eq!(principals.len(), 1);
    assert_eq!(principals[0]["id"], format!("alpha@{bundle_name}"));
    assert_eq!(bundle["principals_partial"], true);
    // The out-of-scope failure record and its count are suppressed, along with
    // every other bundle-level diagnostic that describes namespace-wide state.
    assert_eq!(bundle["startup_failure_count"], 0);
    assert!(
        bundle["recent_startup_failures"]
            .as_array()
            .expect("failures array")
            .is_empty()
    );
    assert_eq!(bundle["hosted"], false);
    assert_eq!(bundle["state"], "down");
    // No serialized field carries the out-of-scope session id.
    assert!(
        !serde_json::to_string(bundle)
            .expect("encode bundle")
            .contains("bravo"),
        "subset listing must not leak out-of-scope session data: {bundle}"
    );
    drop(temporary);
}

#[test]
fn ingress_global_principal_discovery_enumerates_registry_under_namespace_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    // A GLOBAL-namespace ingress grant covers every relay-wide principal.
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some("GLOBAL"),
    );
    // A live relay-wide principal registered in the process-wide registry. Before
    // the GLOBAL registry path existed, foreign GLOBAL principal discovery always
    // returned an empty bundle even though namespace discovery advertised it. A
    // GLOBAL principal must be declared in users.toml to Hello; the default
    // configuration declares exactly this operator.
    let global_id = global_user_id(&bundle_name);
    let (global_client, global_handle) =
        spawn_live_global_principal(&configuration_root, &bundle_paths, global_id.as_str());
    let response = ingress_request_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        json!({"operation": "discover_principals", "namespace": "GLOBAL"}),
    );
    assert_eq!(response["response"]["kind"], "discover_principals");
    let bundle = &response["response"]["bundles"][0];
    assert_eq!(bundle["id"], "GLOBAL");
    let ids: Vec<&str> = bundle["principals"]
        .as_array()
        .expect("principals array")
        .iter()
        .filter_map(|principal| principal["id"].as_str())
        .collect();
    assert!(
        ids.contains(&global_id.as_str()),
        "GLOBAL registry principal is enumerated: {ids:?}"
    );
    // A namespace grant covering all present principals is a complete listing,
    // and its state mirrors canonical GLOBAL semantics: hosted/up because the
    // live principal is ready (not the neutral placeholder of a subset view).
    assert!(bundle.get("principals_partial").is_none() || bundle["principals_partial"].is_null());
    assert_eq!(bundle["hosted"], true);
    assert_eq!(bundle["state"], "up");
    global_client.shutdown(std::net::Shutdown::Both).ok();
    global_handle.join().expect("join global principal");
    drop(temporary);
}

#[test]
fn ingress_global_principal_discovery_marks_subset_under_exact_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    // Two declared GLOBAL operators, both live in the registry; the exact grant
    // covers only one.
    let (covered, excluded) = declare_two_global_operators(&configuration_root, &bundle_name);
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some(covered.as_str()),
    );
    let (covered_client, covered_handle) =
        spawn_live_global_principal(&configuration_root, &bundle_paths, covered.as_str());
    let (excluded_client, excluded_handle) =
        spawn_live_global_principal(&configuration_root, &bundle_paths, excluded.as_str());
    let response = ingress_request_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        json!({"operation": "discover_principals", "namespace": "GLOBAL"}),
    );
    assert_eq!(response["response"]["kind"], "discover_principals");
    let bundle = &response["response"]["bundles"][0];
    let principals = bundle["principals"].as_array().expect("principals array");
    assert_eq!(principals.len(), 1);
    assert_eq!(principals[0]["id"], covered);
    // The excluded relay-wide principal is omitted and the subset is marked.
    assert_eq!(bundle["principals_partial"], true);
    // An exact-principal grant is addressing-only: neutral diagnostics even
    // though the covered principal is live.
    assert_eq!(bundle["hosted"], false);
    assert_eq!(bundle["state"], "down");
    covered_client.shutdown(std::net::Shutdown::Both).ok();
    excluded_client.shutdown(std::net::Shutdown::Both).ok();
    covered_handle.join().expect("join covered principal");
    excluded_handle.join().expect("join excluded principal");
    drop(temporary);
}

#[test]
fn ingress_discovery_rejects_peer_reforward_without_dialing() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some(bundle_name.as_str()),
    );
    // A peer relay ingress requester presents a relay selector to chain discovery
    // onward. No peers are configured here, so a dial would surface
    // `validation_unknown_peer`; `authorization_forbidden` proves the re-forward
    // was refused before any onward lookup or dial.
    let response = ingress_request_response(
        &configuration_root,
        &bundle_paths,
        relay_principal_id.as_str(),
        json!({"operation": "discover_namespaces", "relay": "west"}),
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
fn foreign_discovery_reports_missing_peer_credential_typed_error() {
    let (_temporary, bundle_name, configuration_root, bundle_paths) = origin_fixture();
    // No peer credential is provisioned, so the forward cannot present an identity.
    let peer_socket = configuration_root.join("west.sock");
    let (client, handle) = spawn_relay_stream_with_peer(
        &configuration_root,
        &bundle_paths,
        "west",
        "origin-relay",
        &peer_socket,
    );
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        global_user_id(&bundle_name).as_str(),
        json!({"operation": "discover_namespaces", "relay": "west"}),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "runtime_peer_credential_missing"
    );
}

#[test]
fn foreign_discovery_reports_unreachable_peer_typed_error() {
    let (_temporary, bundle_name, configuration_root, bundle_paths) = origin_fixture();
    // The credential is present but no listener is bound, so the dial itself fails.
    write_peer_credential(&bundle_paths.state_root, "west", "peer-secret");
    let peer_socket = configuration_root.join("west.sock");
    let (client, handle) = spawn_relay_stream_with_peer(
        &configuration_root,
        &bundle_paths,
        "west",
        "origin-relay",
        &peer_socket,
    );
    let response = discovery_exchange(
        client,
        handle,
        bundle_name.as_str(),
        global_user_id(&bundle_name).as_str(),
        json!({"operation": "discover_namespaces", "relay": "west"}),
    );
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "runtime_peer_unavailable"
    );
}

// Writes a startup-failure history file into the bundle runtime directory with a
// single failure record for `session_id`, so a list projection folds it into
// `recent_startup_failures` (and its count) unless a scope filter suppresses it.
fn write_startup_failure(runtime_directory: &Path, session_id: &str) {
    std::fs::create_dir_all(runtime_directory).expect("create runtime directory");
    let body = json!({
        "schema_version": 1,
        "next_sequence": 2,
        "records": [{
            "session_id": session_id,
            "transport": "tmux",
            "code": "runtime_startup_failed",
            "reason": "boom",
            "timestamp": "2026-07-21T00:00:00Z",
            "sequence": 1,
            "details": {"note": "out-of-scope detail"},
        }],
    });
    std::fs::write(
        runtime_directory.join("startup_failures.json"),
        serde_json::to_string(&body).expect("encode failure history"),
    )
    .expect("write startup failure history");
}

// Rewrites users.toml to declare two GLOBAL operator principals, returning their
// ids. A GLOBAL principal must be declared to Hello, so a multi-principal GLOBAL
// registry test needs more than the single default operator the standard
// configuration declares.
fn declare_two_global_operators(configuration_root: &Path, bundle_name: &str) -> (String, String) {
    let first = global_user_id(bundle_name);
    let second = first.replace("@GLOBAL", "-two@GLOBAL");
    std::fs::write(
        configuration_root.join("users.toml"),
        format!(
            "default-session = \"{first}\"\n\n[[sessions]]\nid = \"{first}\"\npolicy = \"operator\"\n\n[sessions.ui]\n\n[[sessions]]\nid = \"{second}\"\npolicy = \"operator\"\n\n[sessions.ui]\n"
        ),
    )
    .expect("write users configuration");
    (first, second)
}

// A stub peer that accepts one connection, reads the dialer's Hello, then answers
// with the structured error frame a real relay emits for an unrecognized
// credential (rather than closing the socket). This drives the dialer's Response-
// error classification path (`client.rs` hello loop), not the bare-EOF path, so a
// regression parsing/classifying a real credential denial would be caught. The
// dialer surfaces the rejected handshake as `runtime_peer_unavailable`.
fn spawn_rejecting_peer(socket_path: &Path) {
    let listener = UnixListener::bind(socket_path).expect("bind rejecting peer socket");
    thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut reader = BufReader::new(stream.try_clone().expect("clone rejecting stream"));
        let mut stream = stream;
        let mut hello_line = String::new();
        if reader.read_line(&mut hello_line).is_err() {
            return;
        }
        let rejection = json!({
            "frame": "response",
            "request_id": Value::Null,
            "response": {
                "kind": "error",
                "error": {
                    "code": "validation_unrecognized_credential",
                    "message": "peer relay credential is not recognized",
                },
            },
        });
        let _ = writeln!(stream, "{rejection}");
        let _ = stream.flush();
        thread::sleep(Duration::from_millis(50));
    });
}

// Opens a live `@GLOBAL` connection and holds it registered in the process-wide
// stream registry, returning the client stream and serve handle so the caller can
// keep it alive across a discovery request and tear it down afterward.
fn spawn_live_global_principal(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    global_id: &str,
) -> (UnixStream, thread::JoinHandle<()>) {
    let (mut client, handle) = spawn_relay_stream(configuration_root, bundle_paths);
    let reader_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(reader_stream);
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": global_id,
            "identity_token": "socket-trust",
        }),
    );
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");
    (client, handle)
}
