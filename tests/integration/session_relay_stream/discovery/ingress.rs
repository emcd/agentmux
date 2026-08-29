//! Receiving-side (ingress) discovery, filtered by the peer principal's scope.
//!
//! A namespace scope exposes the complete namespace, an exact-principal scope
//! exposes a `principals_partial` subset, an out-of-scope or empty namespace
//! discloses no existence, and an absent scope denies. The scope filter also
//! governs what reaches the receiving operator's inscriptions.

use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread;

use agentmux::configuration::ConfigurationRoots;
use agentmux::runtime::paths::BundleRuntimePaths;
use serde_json::{Value, json};
use tempfile::TempDir;
use uuid::Uuid;

use super::super::{
    global_user_id, ingress_request_response, read_json, send_json, spawn_relay_stream,
    unique_relay_principal_id, write_bundle_configuration, write_ingress_peer_store,
};

#[test]
fn ingress_namespace_discovery_derives_from_receiver_catalog() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_roots = write_bundle_configuration(&temporary, &bundle_name);
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
        &configuration_roots,
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
    let configuration_roots = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    write_ingress_peer_store(&bundle_paths.state_root, relay_principal_id.as_str(), None);
    let response = ingress_request_response(
        &configuration_roots,
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
    let configuration_roots = write_bundle_configuration(&temporary, &bundle_name);
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
        &configuration_roots,
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

const SCOPE_UNMATCHED_EVENT: &str = "relay.discovery.namespaces.scope_unmatched";

// Collects the lines recording `event` that also contain `containing`, which is
// how a test picks its own records out of a sink it may be sharing. An empty
// `containing` matches every record for the event.
fn inscription_lines(path: &Path, event: &str, containing: &str) -> Vec<String> {
    let needle = format!("\"event\":\"{event}\"");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains(needle.as_str()) && line.contains(containing))
        .map(str::to_string)
        .collect()
}

// Offers `state_root` as the process inscription sink and returns wherever the
// sink actually is.
//
// The sink is a process-wide `OnceLock`, so the first test to configure one in a
// given process wins and every later offer is declined. Under the canonical runner
// that never arises, because each test is its own process; under a shared-process
// harness it decides which file the emissions reach. Returning the configured path
// rather than the offered one keeps a test reading the file its own emissions went
// to either way, and the append recreates the directory if the winner's temporary
// directory has already been removed.
fn configure_inscriptions(state_root: &Path) -> std::path::PathBuf {
    let offered = state_root.join("inscriptions").join("relay.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&offered);
    agentmux::runtime::inscriptions::process_inscriptions_path()
        .expect("inscription sink configured")
        .to_path_buf()
}

/// An ingress scope covering nothing is recorded with the scope and the peer that
/// presented it.
///
/// The wire result stays an ordinary empty success — asserted here alongside the
/// record, because the whole point is that the diagnosis reaches the receiving
/// operator without reaching the peer. `namespace_count: 0` on its own says a peer
/// saw nothing but not which peer or under what grant, which is what left the
/// original smoke-test failure undiagnosable.
#[test]
fn ingress_namespace_discovery_records_a_scope_that_matched_nothing() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_roots = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let inscriptions = configure_inscriptions(&bundle_paths.state_root);
    let relay_principal_id = unique_relay_principal_id();
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some("phantom"),
    );

    let response = ingress_request_response(
        &configuration_roots,
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
    let records = inscription_lines(
        inscriptions.as_path(),
        SCOPE_UNMATCHED_EVENT,
        relay_principal_id.as_str(),
    );
    assert_eq!(records.len(), 1, "expected one record, got {records:?}");
    assert!(
        records[0].contains("\"scope\":\"phantom\""),
        "record omits the scope that matched nothing: {}",
        records[0]
    );
    assert!(
        records[0].contains(relay_principal_id.as_str()),
        "record omits the asking peer: {}",
        records[0]
    );
}

/// A scope that covers a namespace is not recorded as having matched nothing.
///
/// Both assertions are keyed to principals unique to this test, because the sink
/// may be shared with tests running concurrently. The absence assertion is stated
/// second and carries its own control: an unmatched scope is driven through the
/// same relay first, so a sink nobody wired up, or a recorder that stopped
/// emitting, fails on the control instead of satisfying the absence.
#[test]
fn ingress_namespace_discovery_records_nothing_when_the_scope_matches() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_roots = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let inscriptions = configure_inscriptions(&bundle_paths.state_root);

    let unmatched_principal = unique_relay_principal_id();
    write_ingress_peer_store(
        &bundle_paths.state_root,
        unmatched_principal.as_str(),
        Some("phantom"),
    );
    ingress_request_response(
        &configuration_roots,
        &bundle_paths,
        unmatched_principal.as_str(),
        json!({"operation": "discover_namespaces"}),
    );
    assert_eq!(
        inscription_lines(
            inscriptions.as_path(),
            SCOPE_UNMATCHED_EVENT,
            unmatched_principal.as_str(),
        )
        .len(),
        1,
        "control: an unmatched scope must reach the sink for the absence below to mean anything"
    );

    let matched_principal = unique_relay_principal_id();
    write_ingress_peer_store(
        &bundle_paths.state_root,
        matched_principal.as_str(),
        Some(bundle_name.as_str()),
    );
    let response = ingress_request_response(
        &configuration_roots,
        &bundle_paths,
        matched_principal.as_str(),
        json!({"operation": "discover_namespaces"}),
    );

    assert_eq!(
        response["response"]["namespaces"]
            .as_array()
            .expect("namespaces array"),
        &vec![Value::from(bundle_name.as_str())]
    );
    assert!(
        inscription_lines(
            inscriptions.as_path(),
            SCOPE_UNMATCHED_EVENT,
            matched_principal.as_str(),
        )
        .is_empty(),
        "a scope that covered a namespace must not be recorded as unmatched"
    );
}

#[test]
fn ingress_principal_discovery_returns_complete_namespace_under_namespace_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_roots = write_bundle_configuration(&temporary, &bundle_name);
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
        &configuration_roots,
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
    let configuration_roots = write_bundle_configuration(&temporary, &bundle_name);
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
        &configuration_roots,
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
    let configuration_roots = write_bundle_configuration(&temporary, &bundle_name);
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
        &configuration_roots,
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
    let configuration_roots = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    // A peer registered without an ingress scope is fail-closed for principal
    // discovery, exactly as for namespace discovery.
    write_ingress_peer_store(&bundle_paths.state_root, relay_principal_id.as_str(), None);
    let response = ingress_request_response(
        &configuration_roots,
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
    let configuration_roots = write_bundle_configuration(&temporary, &bundle_name);
    // Reconfigure the bundle to the single member `alpha`. `bravo` is no longer a
    // configured member, but a stale startup-failure record for it survives on
    // disk (startup history is keyed by session id, independent of membership).
    std::fs::write(
        configuration_roots
            .base_layer()
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
        &configuration_roots,
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
fn ingress_principal_subset_suppresses_out_of_scope_bundle_diagnostics() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_roots = write_bundle_configuration(&temporary, &bundle_name);
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
        &configuration_roots,
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
    let configuration_roots = write_bundle_configuration(&temporary, &bundle_name);
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
        spawn_live_global_principal(&configuration_roots, &bundle_paths, global_id.as_str());
    let response = ingress_request_response(
        &configuration_roots,
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
    let configuration_roots = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let relay_principal_id = unique_relay_principal_id();
    // Two declared GLOBAL operators, both live in the registry; the exact grant
    // covers only one.
    let (covered, excluded) = declare_two_global_operators(&configuration_roots, &bundle_name);
    write_ingress_peer_store(
        &bundle_paths.state_root,
        relay_principal_id.as_str(),
        Some(covered.as_str()),
    );
    let (covered_client, covered_handle) =
        spawn_live_global_principal(&configuration_roots, &bundle_paths, covered.as_str());
    let (excluded_client, excluded_handle) =
        spawn_live_global_principal(&configuration_roots, &bundle_paths, excluded.as_str());
    let response = ingress_request_response(
        &configuration_roots,
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
    let configuration_roots = write_bundle_configuration(&temporary, &bundle_name);
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
        &configuration_roots,
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
fn declare_two_global_operators(
    configuration_roots: &ConfigurationRoots,
    bundle_name: &str,
) -> (String, String) {
    let first = global_user_id(bundle_name);
    let second = first.replace("@GLOBAL", "-two@GLOBAL");
    std::fs::write(
        configuration_roots.base_layer().join("users.toml"),
        format!(
            "default-session = \"{first}\"\n\n[[sessions]]\nid = \"{first}\"\npolicy = \"operator\"\n\n[sessions.ui]\n\n[[sessions]]\nid = \"{second}\"\npolicy = \"operator\"\n\n[sessions.ui]\n"
        ),
    )
    .expect("write users configuration");
    (first, second)
}

// Opens a live `@GLOBAL` connection and holds it registered in the process-wide
// stream registry, returning the client stream and serve handle so the caller can
// keep it alive across a discovery request and tear it down afterward.
fn spawn_live_global_principal(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    global_id: &str,
) -> (UnixStream, thread::JoinHandle<()>) {
    let (mut client, handle) = spawn_relay_stream(configuration_roots, bundle_paths);
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
