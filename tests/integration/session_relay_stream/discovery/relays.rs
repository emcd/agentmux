//! Relay enumeration and local namespace discovery.
//!
//! `list.relays` answers from configuration alone and must never dial; local
//! `list.namespaces` reflects the requester's own `list` visibility. Both are
//! origin-side paths that resolve without contacting a peer.

use agentmux::runtime::paths::BundleRuntimePaths;
use serde_json::{Value, json};
use uuid::Uuid;

use super::super::global_user_id;
use super::{discovery_exchange, origin_fixture, spawn_origin_relay};

#[test]
fn list_relays_enumerates_configured_aliases_sorted_without_dialing() {
    let (_temporary, bundle_name, configuration_roots, bundle_paths) = origin_fixture();
    let state_root = bundle_paths.state_root.clone();
    // Two aliases in reverse order; nonexistent sockets prove no dial occurs.
    let (client, handle) = spawn_origin_relay(
        &configuration_roots,
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
    let (_temporary, bundle_name, configuration_roots, bundle_paths) = origin_fixture();
    let state_root = bundle_paths.state_root.clone();
    let (client, handle) = spawn_origin_relay(
        &configuration_roots,
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
    let (_temporary, bundle_name, configuration_roots, bundle_paths) = origin_fixture();
    let state_root = bundle_paths.state_root.clone();
    // `alpha` resolves the default policy (`list = home`), below the `all` tier.
    let (client, handle) = spawn_origin_relay(
        &configuration_roots,
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
    let (temporary, bundle_name, configuration_roots, bundle_paths) = origin_fixture();
    let state_root = bundle_paths.state_root.clone();
    // A second bundle the requester is not a member of, loaded into the catalog.
    let other_bundle = format!("other-{}", Uuid::new_v4().simple());
    std::fs::write(
        configuration_roots
            .base_layer()
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
        spawn_origin_relay(&configuration_roots, &state_root, catalog.clone(), &[]);
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
    let (client, handle) = spawn_origin_relay(&configuration_roots, &state_root, catalog, &[]);
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
