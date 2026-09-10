//! Peer-slot install operation: alias-referent binding, owner-only install,
//! unsafe-alias refusal, idempotent retry, and PSK omission from responses.

use std::os::unix::fs::PermissionsExt;

use agentmux::runtime::paths::BundleRuntimePaths;
use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;

/// Issues `install_peer_credential` for `alias` carrying `psk` as
/// `operator` and returns the full response frame.
fn install_peer_credential_as(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    operator: &str,
    alias: &str,
    psk: &str,
) -> Value {
    let (mut client, join) = spawn_relay_connection(configuration_roots, bundle_paths);
    let read_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": operator,
            "identity_token": "socket-trust",
        }),
    );
    let ack = read_json(&mut reader);
    assert_eq!(
        ack["frame"], "hello_ack",
        "operator hello not acked: {ack:?}"
    );
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "admin-1",
            "request": {
                "operation": "install_peer_credential",
                "alias": alias,
                "psk": psk,
            },
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    shutdown_stream(&client, "shutdown operator stream");
    join.join().expect("join operator relay thread");
    response
}

/// Issues `install_peer_credential` for `alias` carrying `psk` and returns
/// the full response frame.
fn install_peer_credential(
    configuration_roots: &ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
    bundle_name: &str,
    alias: &str,
    psk: &str,
) -> Value {
    operator_request(
        configuration_roots,
        bundle_paths,
        bundle_name,
        json!({
            "operation": "install_peer_credential",
            "alias": alias,
            "psk": psk,
        }),
    )
}

fn install_paths(
    temporary: &TempDir,
    bundle_name: &str,
    alias: &str,
) -> (
    ConfigurationRoots,
    BundleRuntimePaths,
    String,
    std::path::PathBuf,
) {
    let configuration_roots = write_identity_configuration(temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let principal_id = format!("{alias}@RELAY");
    let slot = state_root.join("peers").join(format!("{alias}.psk"));
    (configuration_roots, bundle_paths, principal_id, slot)
}

// Install writes the issued PSK into the relay-owned peer slot with
// owner-only modes and omits it from every response field.
#[test]
fn install_peer_credential_writes_slot_and_omits_psk() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_install_happy";
    let (configuration_roots, bundle_paths, principal_id, slot) =
        install_paths(&temporary, bundle_name, "bravo");
    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );

    let response = install_peer_credential(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        "bravo",
        &psk,
    );
    assert_eq!(
        response["response"]["kind"], "install_peer_credential",
        "install rejected: {response:?}"
    );
    assert_eq!(
        response["response"]["alias"], "bravo",
        "install misaddressed: {response:?}"
    );
    assert!(
        response["response"].get("psk").is_none(),
        "install must never return the PSK: {response:?}"
    );
    assert_eq!(
        response["response"]["written_path"].as_str(),
        Some(slot.to_string_lossy().as_ref()),
        "install misreported path: {response:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&slot).expect("read installed slot"),
        psk,
        "installed slot holds the wrong secret"
    );
    assert_eq!(
        std::fs::metadata(&slot)
            .expect("slot metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600,
        "installed slot is not owner-only"
    );
}

// Install without a registered alias referent fails naming the alias and
// writes nothing.
#[test]
fn install_peer_credential_requires_alias_referent() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_install_referent";
    let (configuration_roots, bundle_paths, _, slot) =
        install_paths(&temporary, bundle_name, "ghost");

    let response = install_peer_credential(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        "ghost",
        "some-psk-value",
    );
    assert_eq!(
        response["response"]["kind"], "error",
        "expected error: {response:?}"
    );
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_unknown_principal"
    );
    assert_eq!(
        response["response"]["error"]["details"]["alias"].as_str(),
        Some("ghost"),
        "error must name the alias: {response:?}"
    );
    assert!(!slot.exists(), "install must not write without a referent");
}

// Unsafe alias components are refused before any filesystem access.
#[test]
fn install_peer_credential_rejects_unsafe_alias() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_install_alias";
    let (configuration_roots, bundle_paths, _, _) = install_paths(&temporary, bundle_name, "bravo");

    for alias in ["../escape", "a/b", "with@qualifier", "", ".", ".."] {
        let response = install_peer_credential(
            &configuration_roots,
            &bundle_paths,
            bundle_name,
            alias,
            "some-psk-value",
        );
        assert_eq!(
            response["response"]["kind"], "error",
            "expected error for alias {alias:?}: {response:?}"
        );
        assert_eq!(
            response["response"]["error"]["code"], "validation_invalid_credential_path",
            "wrong code for alias {alias:?}: {response:?}"
        );
    }
}

// Repeating an identical install succeeds without changing the slot: the
// lost-response retry path.
#[test]
fn install_peer_credential_identical_retry_is_idempotent() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_install_retry";
    let (configuration_roots, bundle_paths, principal_id, slot) =
        install_paths(&temporary, bundle_name, "bravo");
    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );
    let first = install_peer_credential(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        "bravo",
        &psk,
    );
    assert_eq!(first["response"]["kind"], "install_peer_credential");

    let second = install_peer_credential(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        "bravo",
        &psk,
    );
    assert_eq!(
        second["response"]["kind"], "install_peer_credential",
        "identical retry rejected: {second:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&slot).expect("read installed slot"),
        psk,
        "retry must preserve slot content"
    );
}

// A symlinked `peers/` ancestor aborts the install without touching the
// external target.
#[test]
fn install_peer_credential_rejects_symlinked_ancestor() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_install_symlink";
    let (configuration_roots, bundle_paths, principal_id, _) =
        install_paths(&temporary, bundle_name, "bravo");
    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );
    let state_root = temporary.path().join("state");
    let external = temporary.path().join("external");
    std::fs::create_dir_all(&external).expect("create external directory");
    std::fs::write(external.join("marker"), "untouched").expect("write marker");
    std::os::unix::fs::symlink(&external, state_root.join("peers")).expect("symlink peers");

    let response = install_peer_credential(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        "bravo",
        &psk,
    );
    assert_eq!(
        response["response"]["kind"], "error",
        "expected error: {response:?}"
    );
    assert_eq!(
        response["response"]["error"]["code"], "validation_invalid_credential_path",
        "wrong code: {response:?}"
    );
    assert_eq!(
        std::fs::read_to_string(external.join("marker")).expect("read marker"),
        "untouched",
        "install must not write through the symlink"
    );
    assert!(
        !external.join("bravo.psk").exists(),
        "install must not publish through the symlink"
    );
}

// Derives a short unique `@GLOBAL` user id per test and role, mirroring
// `global_user_id`: the stream registry keys relay-wide principals by id
// alone, and raw bundle names exceed the session-id length limit.
fn limited_user_id(bundle_name: &str, role: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bundle_name.hash(&mut hasher);
    role.hash(&mut hasher);
    format!("l{:016x}@GLOBAL", hasher.finish())
}

// Writes the standard operator configuration, then adds provisioner-only
// (`new.peer=all`), rotator-only (`change.psk=all`), and control-free
// (default policy) `@GLOBAL` users for slot-state authorization tests.
// Returns the roots plus the three limited user ids.
fn write_limited_configuration(
    temporary: &TempDir,
    bundle_name: &str,
) -> (ConfigurationRoots, String, String, String) {
    let configuration_roots = write_identity_configuration(temporary, bundle_name);
    std::fs::write(
        configuration_roots.base_layer().join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "home"
look = "home"
send = "home"

[[policies]]
id = "operator"

[policies.controls]
list = "home"
look = "home"
send = "home"

[policies.controls.new]
peer = "all"

[policies.controls.change]
psk = "all"

[policies.controls.drop]
peer = "all"

[[policies]]
id = "provisioner"

[policies.controls]
list = "home"
look = "home"
send = "home"

[policies.controls.new]
peer = "all"

[[policies]]
id = "rotator"

[policies.controls]
list = "home"
look = "home"
send = "home"

[policies.controls.change]
psk = "all"
"#,
    )
    .expect("write limited policies configuration");
    let operator = super::super::global_user_id(bundle_name);
    let ids = [
        ("provisioner", limited_user_id(bundle_name, "provisioner")),
        ("rotator", limited_user_id(bundle_name, "rotator")),
        ("bystander", limited_user_id(bundle_name, "bystander")),
    ];
    let sessions = ids
        .iter()
        .map(|(user, id)| {
            let policy = if *user == "bystander" {
                "default"
            } else {
                user
            };
            format!("[[sessions]]\nid = \"{id}\"\npolicy = \"{policy}\"\n\n[sessions.ui]\n")
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(
        configuration_roots.base_layer().join("users.toml"),
        format!(
            "default-session = \"{operator}\"\n\n[[sessions]]\nid = \"{operator}\"\npolicy = \"operator\"\n\n[sessions.ui]\n\n{sessions}"
        ),
    )
    .expect("write full users configuration");
    (
        configuration_roots,
        limited_user_id(bundle_name, "provisioner"),
        limited_user_id(bundle_name, "rotator"),
        limited_user_id(bundle_name, "bystander"),
    )
}

fn assert_forbidden(response: &Value) {
    assert_eq!(
        response["response"]["kind"], "error",
        "expected denial: {response:?}"
    );
    assert_eq!(
        response["response"]["error"]["code"], "authorization_forbidden",
        "wrong denial code: {response:?}"
    );
}

// An absent slot provisions under `new.peer=all` alone.
#[test]
fn install_absent_slot_succeeds_with_provision_only_authority() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_install_absent_new";
    let (configuration_roots, provisioner, _, _) =
        write_limited_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let register = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "new_peer", "principal_id": "bravo@RELAY"}),
    );
    assert_eq!(register["response"]["kind"], "new_peer");

    let response = install_peer_credential_as(
        &configuration_roots,
        &bundle_paths,
        &provisioner,
        "bravo",
        "provisioned-psk",
    );
    assert_eq!(
        response["response"]["kind"], "install_peer_credential",
        "provision-only install rejected: {response:?}"
    );
}
// An absent slot refuses a rotation-only caller.
#[test]
fn install_absent_slot_refuses_rotation_only_authority() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_install_absent_change";
    let (configuration_roots, _, rotator, _) = write_limited_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let register = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "new_peer", "principal_id": "bravo@RELAY"}),
    );
    assert_eq!(register["response"]["kind"], "new_peer");

    let response = install_peer_credential_as(
        &configuration_roots,
        &bundle_paths,
        &rotator,
        "bravo",
        "rotation-psk",
    );
    assert_forbidden(&response);
    assert!(
        !state_root.join("peers").join("bravo.psk").exists(),
        "denied install must not write"
    );
}

// A present slot with different content rotates under `change.psk=all`
// alone and refuses a provision-only caller.
#[test]
fn install_different_content_splits_authority() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_install_rotate";
    let (configuration_roots, provisioner, rotator, _) =
        write_limited_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let register = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "new_peer", "principal_id": "bravo@RELAY"}),
    );
    assert_eq!(register["response"]["kind"], "new_peer");
    let slot = state_root.join("peers").join("bravo.psk");

    let first = install_peer_credential_as(
        &configuration_roots,
        &bundle_paths,
        &provisioner,
        "bravo",
        "first-psk",
    );
    assert_eq!(first["response"]["kind"], "install_peer_credential");

    let denied = install_peer_credential_as(
        &configuration_roots,
        &bundle_paths,
        &provisioner,
        "bravo",
        "second-psk",
    );
    assert_forbidden(&denied);
    assert_eq!(
        std::fs::read_to_string(&slot).expect("read slot"),
        "first-psk",
        "denied rotation must preserve slot content"
    );

    let rotated = install_peer_credential_as(
        &configuration_roots,
        &bundle_paths,
        &rotator,
        "bravo",
        "second-psk",
    );
    assert_eq!(
        rotated["response"]["kind"], "install_peer_credential",
        "rotation-only rotation rejected: {rotated:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&slot).expect("read slot"),
        "second-psk",
        "rotation must replace slot content"
    );
}

// The pinned same-authority retry: a provision-only caller whose install
// succeeded but whose response was lost retries the identical install and
// succeeds, with content preserved.
#[test]
fn install_identical_retry_succeeds_with_provision_only_authority() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_install_same_retry";
    let (configuration_roots, provisioner, _, _) =
        write_limited_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let register = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "new_peer", "principal_id": "bravo@RELAY"}),
    );
    assert_eq!(register["response"]["kind"], "new_peer");
    let slot = state_root.join("peers").join("bravo.psk");

    // The "lost" first write lands through another caller; the retry below
    // classifies the slot as present-identical under provision-only
    // authority.
    let operator = super::super::global_user_id(bundle_name);
    let first = install_peer_credential_as(
        &configuration_roots,
        &bundle_paths,
        &operator,
        "bravo",
        "retry-psk",
    );
    assert_eq!(first["response"]["kind"], "install_peer_credential");

    let retry = install_peer_credential_as(
        &configuration_roots,
        &bundle_paths,
        &provisioner,
        "bravo",
        "retry-psk",
    );
    assert_eq!(
        retry["response"]["kind"], "install_peer_credential",
        "same-authority identical retry rejected: {retry:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&slot).expect("read slot"),
        "retry-psk",
        "retry must preserve slot content"
    );
}

// A caller with neither control is refused on every slot state.
#[test]
fn install_refuses_control_free_caller() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_install_bystander";
    let (configuration_roots, _, _, bystander) =
        write_limited_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let register = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "new_peer", "principal_id": "bravo@RELAY"}),
    );
    assert_eq!(register["response"]["kind"], "new_peer");

    let absent = install_peer_credential_as(
        &configuration_roots,
        &bundle_paths,
        &bystander,
        "bravo",
        "some-psk",
    );
    assert_forbidden(&absent);
}

// A post-rename directory-sync failure keeps the published slot and
// reports durability uncertainty instead of rolling back.
#[test]
fn install_dir_sync_failure_stays_published_as_uncertain() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_install_uncertain";
    let (configuration_roots, bundle_paths, principal_id, slot) =
        install_paths(&temporary, bundle_name, "bravo");
    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &principal_id,
        None,
    );
    std::fs::create_dir_all(slot.parent().expect("slot parent")).expect("create peers dir");
    std::fs::write(
        slot.parent().expect("slot parent").join(".fault-dir-sync"),
        "",
    )
    .expect("arm dir-sync fault");

    let response = install_peer_credential(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        "bravo",
        &psk,
    );
    assert_eq!(
        response["response"]["kind"], "error",
        "expected error: {response:?}"
    );
    assert_eq!(
        response["response"]["error"]["code"], "internal_peer_credential_durability_uncertain",
        "wrong code: {response:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&slot).expect("read published slot"),
        psk,
        "uncertain install must leave the published file in place"
    );
}
