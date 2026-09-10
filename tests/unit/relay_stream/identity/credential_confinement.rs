//! Confined credential writes: ancestor-symlink rejection for the config
//! sink (both credential operations), the principal store (load and
//! persist), and ancestor exchange between staging and commit. Caller-named
//! path sinks keep the final-target check only.

use agentmux::relay::test_hooks::{
    ConfinedCommitHook, arm_confined_commit_hook, disarm_confined_commit_hook,
};
use agentmux::runtime::paths::BundleRuntimePaths;
use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;

/// Guard disarming the commit hook when the test ends, so a global hook
/// never leaks into parallel tests.
struct DisarmGuard;

impl Drop for DisarmGuard {
    fn drop(&mut self) {
        disarm_confined_commit_hook();
    }
}

/// Replaces `state_root/dir_name` with a symlink to `external`, returning
/// the marker path for untouched assertions.
fn symlink_ancestor(
    temporary: &TempDir,
    state_root: &std::path::Path,
    dir_name: &str,
) -> std::path::PathBuf {
    std::fs::create_dir_all(state_root).expect("create state root");
    let external = temporary.path().join(format!("external-{dir_name}"));
    std::fs::create_dir_all(&external).expect("create external directory");
    std::fs::write(external.join("marker"), "untouched").expect("write marker");
    std::os::unix::fs::symlink(&external, state_root.join(dir_name)).expect("symlink ancestor");
    external
}

fn assert_untouched(external: &std::path::Path) {
    assert_eq!(
        std::fs::read_to_string(external.join("marker")).expect("read marker"),
        "untouched",
        "confined write must not touch the symlink target"
    );
}

fn assert_invalid_credential_path(response: &Value) {
    assert_eq!(
        response["response"]["kind"], "error",
        "expected error: {response:?}"
    );
    assert_eq!(
        response["response"]["error"]["code"], "validation_invalid_credential_path",
        "wrong code: {response:?}"
    );
}

fn config_destination_request(principal_id: &str) -> Value {
    json!({
        "operation": "new_peer",
        "principal_id": principal_id,
        "destination": {"kind": "config"},
    })
}

// `new peer --write-config` aborts through a symlinked `bundles/` ancestor
// without registering the principal.
#[test]
fn new_peer_config_rejects_symlinked_bundles_ancestor() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_conf_bundles";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let external = symlink_ancestor(&temporary, &state_root, "bundles");

    let response = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        config_destination_request(&format!("alpha@{bundle_name}")),
    );
    assert_invalid_credential_path(&response);
    assert_untouched(&external);
    assert!(
        !external.join("identity.psk").exists(),
        "no credential may publish through the symlink"
    );

    // The rejected destination registers nothing: the same principal
    // registers cleanly once the ancestor is real.
    std::fs::remove_file(state_root.join("bundles")).expect("remove symlink");
    std::fs::create_dir_all(state_root.join("bundles")).expect("restore bundles");
    let retry = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        config_destination_request(&format!("alpha@{bundle_name}")),
    );
    assert_eq!(
        retry["response"]["kind"], "new_peer",
        "retry after restoring ancestor rejected: {retry:?}"
    );
}

// `change psk --write-config` aborts through a symlinked ancestor without
// rotating: the prior credential still authenticates.
#[test]
fn change_psk_config_rejects_symlinked_ancestor_without_rotating() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_conf_rotate";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");
    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );
    let external = symlink_ancestor(&temporary, &state_root, "bundles");

    let response = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({
            "operation": "change_psk",
            "principal_id": principal_id,
            "destination": {"kind": "config"},
        }),
    );
    assert_invalid_credential_path(&response);
    assert_untouched(&external);

    std::fs::remove_file(state_root.join("bundles")).expect("remove symlink");
    let frame = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        &principal_id,
        &psk,
        false,
    );
    assert_eq!(
        frame["frame"], "hello_ack",
        "prior credential must survive the aborted rotation: {frame:?}"
    );
}
// Store load through a symlinked `identity/` ancestor aborts instead of
// reading outside the state tree. The relay cannot authenticate without its
// store, so even Hello fail-closes with the confinement code.
#[test]
fn store_load_rejects_symlinked_identity() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_conf_load";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let external = symlink_ancestor(&temporary, &state_root, "identity");
    let decoy = "{\"format_version\": 1, \"principals\": []}";
    std::fs::write(external.join("principals.json"), decoy).expect("write decoy store");

    let frame = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        &format!("alpha@{bundle_name}"),
        "socket-trust",
        false,
    );
    assert_eq!(
        frame["response"]["error"]["code"], "validation_invalid_credential_path",
        "load must abort, not read outside: {frame:?}"
    );
    assert_eq!(
        std::fs::read_to_string(external.join("principals.json")).expect("read decoy"),
        decoy,
        "store load must neither read nor change the symlink target"
    );
    assert_untouched(&external);
}

// Store persist through an ancestor swapped for a symlink between load and
// persist aborts instead of writing outside the state tree. The exchange
// hook swaps `identity/` after the request's load succeeded, so only the
// persist-side traversal observes the symlink.
#[test]
fn store_persist_rejects_symlinked_identity() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_conf_persist";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let external = temporary.path().join("external-identity");
    std::fs::create_dir_all(&external).expect("create external directory");
    std::fs::write(external.join("marker"), "untouched").expect("write marker");
    let _guard = arm_ancestor_exchange(state_root.clone(), "identity", external.clone());

    let response = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "new_peer", "principal_id": "bravo@RELAY"}),
    );
    assert_invalid_credential_path(&response);
    assert_untouched(&external);
    assert!(
        !external.join("principals.json").exists(),
        "store persist must not publish through the symlink"
    );
}
// Arms a one-shot ancestor exchange: on the first commit-boundary firing,
// renames `dir_name` aside and plants a symlink to `external`. Later
// firings observe the planted symlink and do nothing.
fn arm_ancestor_exchange(
    state_root: std::path::PathBuf,
    dir_name: &str,
    external: std::path::PathBuf,
) -> DisarmGuard {
    let dir = dir_name.to_string();
    arm_confined_commit_hook(ConfinedCommitHook::arm(move || {
        let target = state_root.join(&dir);
        if target.is_symlink() {
            return;
        }
        let backup = state_root.join(format!("{dir}.orig"));
        std::fs::rename(&target, &backup).expect("stage exchange");
        std::os::unix::fs::symlink(&external, &target).expect("plant symlink");
    }));
    DisarmGuard
}

// An ancestor exchanged for a symlink between staging and commit aborts the
// sink write before publication.
#[test]
fn sink_commit_aborts_on_ancestor_exchange() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_conf_exchange_sink";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let external = temporary.path().join("external-exchange");
    std::fs::create_dir_all(&external).expect("create external directory");
    std::fs::write(external.join("marker"), "untouched").expect("write marker");
    let _guard = arm_ancestor_exchange(state_root.clone(), "bundles", external.clone());

    let response = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        config_destination_request(&format!("alpha@{bundle_name}")),
    );
    assert_invalid_credential_path(&response);
    assert_untouched(&external);
    assert!(
        !external.join("identity.psk").exists(),
        "no credential may publish through the exchanged ancestor"
    );
    // The staged temp is discarded with the abort: no residue remains.
    let residue: Vec<_> = walkdir_tmp_residue(&state_root.join("bundles.orig"));
    assert!(
        residue.is_empty(),
        "aborted commit must clean its temp: {residue:?}"
    );
}

// An ancestor exchanged for a symlink between staging and commit aborts the
// peer-slot install before publication.
#[test]
fn install_commit_aborts_on_ancestor_exchange() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_conf_exchange_slot";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = "bravo@RELAY".to_string();
    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );
    let external = temporary.path().join("external-slot");
    std::fs::create_dir_all(&external).expect("create external directory");
    std::fs::write(external.join("marker"), "untouched").expect("write marker");
    let _guard = arm_ancestor_exchange(state_root.clone(), "peers", external.clone());

    let response = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({
            "operation": "install_peer_credential",
            "alias": "bravo",
            "psk": psk,
        }),
    );
    assert_invalid_credential_path(&response);
    assert_untouched(&external);
    assert!(
        !external.join("bravo.psk").exists(),
        "no credential may publish through the exchanged ancestor"
    );
}

/// Collects staged-temp residue (`.tmp` files) below `dir`.
fn walkdir_tmp_residue(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut residue = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|value| value.to_str()) == Some("tmp") {
                residue.push(path);
            }
        }
    }
    residue
}

// Caller-named path sinks keep the final-target check only: a symlinked
// parent directory still carries a successful write.
#[test]
fn path_sink_through_symlinked_parent_still_succeeds() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_conf_path_ok";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let real = temporary.path().join("real-out");
    std::fs::create_dir_all(&real).expect("create output directory");
    std::os::unix::fs::symlink(&real, temporary.path().join("linked-out"))
        .expect("symlink output parent");
    let output = temporary.path().join("linked-out").join("credential.psk");

    let response = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({
            "operation": "new_peer",
            "principal_id": "bravo@RELAY",
            "destination": {"kind": "path", "path": output.to_string_lossy()},
        }),
    );
    assert_eq!(
        response["response"]["kind"], "new_peer",
        "path sink through symlinked parent rejected: {response:?}"
    );
    assert!(
        real.join("credential.psk").exists(),
        "path sink must write through the symlinked parent"
    );
}

// Transfer boundary: the issuance Response carries exactly one PSK and no
// file path, while file-sink responses omit the PSK and carry the path.
#[test]
fn issuance_response_carries_psk_while_file_sinks_omit_it() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_conf_boundary";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let issued = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "new_peer", "principal_id": "bravo@RELAY"}),
    );
    let psk = issued["response"]["psk"]
        .as_str()
        .expect("issuance must carry the PSK");
    assert!(
        issued["response"].get("written_path").is_none(),
        "issuance must not carry a path: {issued:?}"
    );
    assert!(
        !issued["response"]["config_snippet"]
            .as_str()
            .unwrap_or_default()
            .contains(psk),
        "config snippet must not leak the PSK"
    );

    let filed = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        config_destination_request(&format!("alpha@{bundle_name}")),
    );
    assert_eq!(filed["response"]["kind"], "new_peer");
    assert!(
        filed["response"].get("psk").is_none(),
        "file-sink response must omit the PSK: {filed:?}"
    );
    assert!(
        filed["response"]["written_path"].is_string(),
        "file-sink response must carry the path: {filed:?}"
    );
}
