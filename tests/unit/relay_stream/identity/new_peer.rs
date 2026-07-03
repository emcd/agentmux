//! `new peer --output` credential-file writer (path validation, O_NOFOLLOW,
//! mode 0600, end-to-end auth).

use std::path::Path;

use agentmux::runtime::paths::BundleRuntimePaths;
use serde_json::Value;
use tempfile::TempDir;

use super::*;

/// Issues `new peer` for `principal_id` with an `output_path` and returns the
/// full response frame (a `new_peer` response on success, or an error response
/// when the writer rejects the path).
fn new_peer_with_output(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    principal_id: &str,
    output_path: &Path,
) -> Value {
    operator_request(
        configuration_root,
        bundle_paths,
        bundle_name,
        json!({
            "operation": "new_peer",
            "principal_id": principal_id,
            "output_path": output_path.to_string_lossy(),
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
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let response = new_peer_with_output(
        &configuration_root,
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
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let output_path = temporary.path().join("absent").join("credential.psk");

    let response = new_peer_with_output(
        &configuration_root,
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
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let output_directory = temporary.path().join("out");
    std::fs::create_dir_all(&output_directory).expect("create output directory");
    let link_path = output_directory.join("credential.psk");
    std::os::unix::fs::symlink(temporary.path().join("elsewhere.psk"), &link_path)
        .expect("create output symlink");

    let response = new_peer_with_output(
        &configuration_root,
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
    let configuration_root = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("alpha@{bundle_name}");
    let output_path = temporary.path().join("credential.psk");

    let response = new_peer_with_output(
        &configuration_root,
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
        response["response"]["output_path"],
        output_path.to_string_lossy().as_ref(),
        "response must echo the written output path"
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
        &configuration_root,
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
