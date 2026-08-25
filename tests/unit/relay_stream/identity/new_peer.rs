//! `new peer` credential-file writers: the caller-named `path` destination
//! (path validation, symlink refusal, mode 0600, end-to-end auth) and the
//! relay-owned `config` destination (session-only, traversal-safe, atomic).

use agentmux::configuration::ConfigurationRoots;
use std::path::Path;

use agentmux::runtime::paths::{BundleRuntimePaths, session_identity_psk_path};
use serde_json::Value;
use tempfile::TempDir;

use super::*;

/// Issues `new peer` for `principal_id` with a caller-named `path` destination
/// and returns the full response frame (a `new_peer` response on success, or an
/// error response when the writer rejects the path).
fn new_peer_with_output(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    principal_id: &str,
    output_path: &Path,
) -> Value {
    operator_request(
        configuration_roots,
        bundle_paths,
        bundle_name,
        json!({
            "operation": "new_peer",
            "principal_id": principal_id,
            "destination": {"kind": "path", "path": output_path.to_string_lossy()},
        }),
    )
}

/// Issues `new peer` for `principal_id` with the relay-owned `config`
/// destination and returns the full response frame.
fn new_peer_with_config(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    principal_id: &str,
) -> Value {
    operator_request(
        configuration_roots,
        bundle_paths,
        bundle_name,
        json!({
            "operation": "new_peer",
            "principal_id": principal_id,
            "destination": {"kind": "config"},
        }),
    )
}

fn assert_invalid_output_path(response: &Value) {
    assert_eq!(
        response["response"]["kind"], "error",
        "expected error response: {response:?}"
    );
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_invalid_output_path"
    );
}

// `new peer --output` rejects a non-absolute path.
#[test]
fn new_peer_output_rejects_relative_path() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_output_relative";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response = new_peer_with_output(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &format!("alpha@{bundle_name}"),
        Path::new("relative-credential.psk"),
    );
    assert_invalid_output_path(&response);
}

// `new peer --output` rejects an absolute path whose parent does not exist (no
// auto-creation of directory trees).
#[test]
fn new_peer_output_rejects_missing_parent_directory() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_output_missing_parent";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let output_path = temporary.path().join("absent").join("credential.psk");

    let response = new_peer_with_output(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &format!("alpha@{bundle_name}"),
        &output_path,
    );
    assert_invalid_output_path(&response);
    assert!(
        !output_path.exists(),
        "writer must not materialize the credential under a missing parent"
    );
}

// `new peer --output` refuses to follow a symlink at the target path
// (O_NOFOLLOW), so a symlinked output cannot redirect the credential write.
#[test]
fn new_peer_output_rejects_symlinked_target() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_output_symlink";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let output_directory = temporary.path().join("out");
    std::fs::create_dir_all(&output_directory).expect("create output directory");
    let link_path = output_directory.join("credential.psk");
    std::os::unix::fs::symlink(temporary.path().join("elsewhere.psk"), &link_path)
        .expect("create output symlink");

    let response = new_peer_with_output(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &format!("alpha@{bundle_name}"),
        &link_path,
    );
    assert_invalid_output_path(&response);
}

// `new peer --output` writes the PSK to the target with mode 0600, omits the
// PSK from the response, and the written credential authenticates at Hello.
#[test]
fn new_peer_output_writes_credential_with_mode_0600() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_output_success";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");
    let output_path = temporary.path().join("credential.psk");

    let response = new_peer_with_output(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &principal_id,
        &output_path,
    );
    assert_eq!(
        response["response"]["kind"], "new_peer",
        "new peer with output rejected: {response:?}"
    );
    assert_eq!(
        response["response"]["written_path"],
        output_path.to_string_lossy().as_ref(),
        "response must echo the written path"
    );
    assert!(
        response["response"]["psk"].is_null(),
        "psk must be omitted from the response when written to a file"
    );

    let mode = std::fs::metadata(&output_path)
        .expect("stat credential file")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "credential file must be owner-only (0600)");

    let psk = std::fs::read_to_string(&output_path).expect("read credential file");
    assert!(!psk.is_empty(), "written credential must be non-empty");
    let frame = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        &principal_id,
        &psk,
        true,
    );
    assert_eq!(
        frame["frame"], "hello_ack",
        "written credential must authenticate: {frame:?}"
    );
    assert_eq!(frame["principal_id"], principal_id);
}

// `new peer` with the `config` destination writes the PSK to the session's
// relay-owned canonical `identity.psk` (mode 0600), omits it from the response,
// and the written credential authenticates at Hello.
#[test]
fn new_peer_config_writes_session_identity_and_authenticates() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_config_success";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");

    let response = new_peer_with_config(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &principal_id,
    );
    assert_eq!(
        response["response"]["kind"], "new_peer",
        "new peer config rejected: {response:?}"
    );

    let canonical = session_identity_psk_path(&state_root, bundle_name, "alpha");
    assert_eq!(
        response["response"]["written_path"],
        canonical.to_string_lossy().as_ref(),
        "response must report the relay-owned canonical path"
    );
    assert!(
        response["response"]["psk"].is_null(),
        "psk must be omitted when written to config"
    );

    let mode = std::fs::metadata(&canonical)
        .expect("stat config credential")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "config credential must be owner-only (0600)");
    let psk = std::fs::read_to_string(&canonical).expect("read config credential");
    assert!(!psk.is_empty(), "written credential must be non-empty");
    let frame = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        &principal_id,
        &psk,
        true,
    );
    assert_eq!(
        frame["frame"], "hello_ack",
        "config credential must authenticate: {frame:?}"
    );
}

// `new peer` with `config` is rejected for a non-session (peer relay) principal
// whose credential location is not relay-owned, and the rejected attempt
// registers nothing: a later response-mode registration of the same id succeeds
// instead of colliding.
#[test]
fn new_peer_config_rejected_for_relay_principal_leaves_store_unchanged() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_config_relay";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = "peer1@RELAY";

    let rejected = new_peer_with_config(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        principal_id,
    );
    assert_eq!(
        rejected["response"]["kind"], "error",
        "config on a relay principal must reject: {rejected:?}"
    );
    assert_eq!(
        rejected["response"]["error"]["code"],
        "validation_config_destination_unsupported"
    );

    let retry = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "new_peer", "principal_id": principal_id}),
    );
    assert_eq!(
        retry["response"]["kind"], "new_peer",
        "the rejected config write must not have registered the principal: {retry:?}"
    );
}

// `new peer` with `config` refuses principal ids that are path-safe but outside
// the configured session-id grammar (a leading digit or a `.`), which no
// configured session could own, with `validation_invalid_principal_id`.
#[test]
fn new_peer_config_rejected_for_non_grammar_session_id() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_config_grammar";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    for session_id in ["1worker", "worker.name"] {
        let response = new_peer_with_config(
            &configuration_roots,
            &bundle_paths,
            bundle_name,
            &format!("{session_id}@{bundle_name}"),
        );
        assert_eq!(
            response["response"]["kind"], "error",
            "path-safe but non-grammar id '{session_id}' must reject: {response:?}"
        );
        assert_eq!(
            response["response"]["error"]["code"], "validation_invalid_principal_id",
            "id '{session_id}' must fail the session-id grammar"
        );
    }
}

// `new peer --output` whose publishing rename fails after the store commit (the
// target path is an existing directory, so the rename is EISDIR) rolls the
// just-inserted record back out: a later response-mode registration of the same
// id succeeds rather than colliding.
#[test]
fn new_peer_output_finalization_failure_leaves_store_unchanged() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_output_finalization";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");
    // A directory at the output path passes validation (absolute, parent exists,
    // not a symlink) but makes the final publishing rename fail after the store
    // has already been persisted, exercising the post-commit rollback.
    let output_path = temporary.path().join("occupied.psk");
    std::fs::create_dir_all(&output_path).expect("create directory at output path");

    let response = new_peer_with_output(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &principal_id,
        &output_path,
    );
    assert_eq!(
        response["response"]["kind"], "error",
        "finalization failure must surface an error: {response:?}"
    );

    let retry = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "new_peer", "principal_id": principal_id}),
    );
    assert_eq!(
        retry["response"]["kind"], "new_peer",
        "rollback must leave the store unchanged: {retry:?}"
    );
}

// `new peer` with `config` accepts a dotted bundle namespace (the canonical
// bundle-name grammar permits `.`), writing to the dotted canonical path — a
// real configured bundle such as `team.one` must not be rejected.
#[test]
fn new_peer_config_accepts_dotted_bundle_name() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_config_dotted";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let dotted_namespace = "team.one";
    let principal_id = format!("alpha@{dotted_namespace}");

    let response = new_peer_with_config(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &principal_id,
    );
    assert_eq!(
        response["response"]["kind"], "new_peer",
        "a dotted bundle name must be accepted for config: {response:?}"
    );
    let canonical = session_identity_psk_path(&state_root, dotted_namespace, "alpha");
    assert_eq!(
        response["response"]["written_path"],
        canonical.to_string_lossy().as_ref(),
        "config must write to the dotted canonical path"
    );
    assert!(
        response["response"]["psk"].is_null(),
        "psk must be omitted when written to config"
    );
}

// `new peer` with `config` refuses a principal id whose components are not safe
// path segments (`..@bundle` classifies as a session but would traverse out of
// the state root), before any canonical path is derived or written.
#[test]
fn new_peer_config_rejected_for_traversal_principal_id() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_config_traversal";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response = new_peer_with_config(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &format!("..@{bundle_name}"),
    );
    assert_eq!(
        response["response"]["kind"], "error",
        "traversal principal id must reject: {response:?}"
    );
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_invalid_principal_id"
    );
}

// An ingress scope spelled as a policy tier is almost certainly a confusion
// between two vocabularies: session-policy controls take `none`/`self`/`home`/
// `all`, while an ingress scope is matched literally against a `session@bundle`
// id or a bare namespace. Every one of those words is a legal namespace name, so
// the relay advises and registers rather than refusing.
#[test]
fn new_peer_advises_on_a_scope_spelled_as_a_policy_tier() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_tier_advice";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    for (index, tier) in ["none", "self", "home", "all"].into_iter().enumerate() {
        let principal_id = format!("tier{index}@RELAY");
        let response = operator_request(
            &configuration_roots,
            &bundle_paths,
            bundle_name,
            json!({
                "operation": "new_peer",
                "principal_id": principal_id,
                "scope": tier,
            }),
        );
        assert_eq!(
            response["response"]["kind"], "new_peer",
            "a tier-spelled scope must still register: {response:?}"
        );
        assert_eq!(
            response["response"]["diagnostics"][0]["code"], "advisory_scope_resembles_policy_tier",
            "scope '{tier}' must raise the vocabulary advisory: {response:?}"
        );
        assert!(
            response["response"]["diagnostics"][0]["message"]
                .as_str()
                .is_some_and(|message| message.contains(tier)),
            "the advisory must name the offending scope: {response:?}"
        );
    }
}

// A scope that merely resolves to nothing stays silent. Peer credentials are
// routinely minted before the namespace they scope exists, and a cross-relay
// scope may name a namespace this relay cannot see, so unresolvability is not
// evidence of a mistake the way a tier word is.
#[test]
fn new_peer_stays_silent_for_a_scope_that_merely_resolves_to_nothing() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_silent";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({
            "operation": "new_peer",
            "principal_id": "quiet@RELAY",
            "scope": "no-such-namespace-anywhere",
        }),
    );
    assert_eq!(
        response["response"]["kind"], "new_peer",
        "new peer rejected: {response:?}"
    );
    assert!(
        response["response"]["diagnostics"].is_null()
            || response["response"]["diagnostics"]
                .as_array()
                .is_some_and(|entries| entries.is_empty()),
        "an unresolvable scope must not raise the vocabulary advisory: {response:?}"
    );
}

// A registration with no scope at all raises nothing.
#[test]
fn new_peer_without_a_scope_raises_no_diagnostics() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_absent";
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "new_peer", "principal_id": format!("alpha@{bundle_name}")}),
    );
    assert_eq!(
        response["response"]["kind"], "new_peer",
        "new peer rejected: {response:?}"
    );
    assert!(
        response["response"]["diagnostics"].is_null()
            || response["response"]["diagnostics"]
                .as_array()
                .is_some_and(|entries| entries.is_empty()),
        "an absent scope must raise nothing: {response:?}"
    );
}
