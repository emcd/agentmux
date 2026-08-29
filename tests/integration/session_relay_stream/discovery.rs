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
//!
//! The cases are grouped by which side of the exchange they exercise:
//! [`relays`] for enumeration and local namespace visibility, [`origin`] for
//! the requesting side of a foreign lookup, and [`ingress`] for the receiving
//! side's scope filter. This module holds the fixtures all three share.

mod ingress;
mod origin;
mod relays;

use agentmux::configuration::ConfigurationRoots;
use std::io::BufReader;
use std::os::unix::net::UnixStream;
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

use super::{hello_payload, read_json, send_json, write_bundle_configuration};

// Serves one origin relay connection with an explicit bundle catalog and set of
// configured peer aliases. Aliases point at nonexistent sockets: discovery paths
// that must not dial (relay enumeration) succeed regardless, and paths that fail
// authorization before dialing never reach them.
fn spawn_origin_relay(
    configuration_roots: &ConfigurationRoots,
    state_root: &Path,
    catalog_paths: Vec<BundleRuntimePaths>,
    aliases: &[&str],
) -> (UnixStream, thread::JoinHandle<()>) {
    let (server_stream, client_stream) = UnixStream::pair().expect("unix stream pair");
    let root = configuration_roots.clone();
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

fn origin_fixture() -> (TempDir, String, ConfigurationRoots, BundleRuntimePaths) {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_roots = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    (temporary, bundle_name, configuration_roots, bundle_paths)
}
