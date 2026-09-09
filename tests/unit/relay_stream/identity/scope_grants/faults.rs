use super::*;

/// Path of the debug-only fault-injection seam forcing post-rename
/// directory-sync failure for the store under `state_root`.
fn dir_sync_fault_file(bundle_paths: &BundleRuntimePaths) -> std::path::PathBuf {
    bundle_paths
        .state_root
        .join("identity")
        .join(".fault-dir-sync")
}

#[test]
fn precommit_validation_failure_leaves_old_grant_intact() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_prename";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "prename@RELAY";
    let target = format!("alpha@{bundle_name}");

    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );

    // A malformed replacement fails before the store transaction begins: the
    // failure is reported as field validation, never durability uncertainty.
    let failed = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*,other"),
    );
    assert_eq!(failed["response"]["kind"], "error");
    assert_eq!(
        failed["response"]["error"]["code"], "validation_invalid_params",
        "pre-commit failure must not report uncertainty: {failed:?}"
    );

    // The old grant is fully intact: the stored record is unchanged and the
    // wildcard still authorizes ingress.
    assert_eq!(
        store_record(&bundle_paths, peer_id)["scope"],
        "*",
        "failed update must not touch the record"
    );
    let allowed = peer_send_response(&configuration_roots, &bundle_paths, peer_id, &psk, &target);
    assert_eq!(
        allowed["response"]["kind"], "send",
        "old grant must survive a failed update: {allowed:?}"
    );
}

#[test]
fn postrename_dirsync_failure_keeps_replacement_effective() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_postrename";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "postrename@RELAY";
    let target = format!("alpha@{bundle_name}");

    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );

    std::fs::write(dir_sync_fault_file(&bundle_paths), "fail").expect("arm fault");
    let uncertain = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("some-other-bundle"),
    );
    assert_eq!(uncertain["response"]["kind"], "error");
    assert_eq!(
        uncertain["response"]["error"]["code"], "internal_store_durability_uncertain",
        "post-rename failure must report uncertainty, not success: {uncertain:?}"
    );
    assert_eq!(
        uncertain["response"]["error"]["details"]["scope"], "some-other-bundle",
        "uncertainty must carry the effective replacement scope: {uncertain:?}"
    );

    // The published replacement governs ingress even though success was never
    // acknowledged: the narrowed grant denies the previously covered target.
    let denied = peer_send_response(&configuration_roots, &bundle_paths, peer_id, &psk, &target);
    assert_eq!(denied["response"]["kind"], "error");
    assert_eq!(
        denied["response"]["error"]["code"], "authorization_forbidden",
        "effective replacement must govern despite uncertainty: {denied:?}"
    );

    // Clearing the fault lets the idempotent same-scope retry synchronize and
    // succeed rather than short-circuiting.
    std::fs::remove_file(dir_sync_fault_file(&bundle_paths)).expect("clear fault");
    let retried = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("some-other-bundle"),
    );
    assert_eq!(
        retried["response"]["kind"], "change_scope",
        "retry after fault must succeed: {retried:?}"
    );
    assert_eq!(retried["response"]["scope"], "some-other-bundle");
}

#[test]
fn rotation_under_dirsync_failure_reports_honestly_and_recovers() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_rotfault";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "rotfault@RELAY";

    let original_psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );

    // With directory sync faulted, the rotation cannot complete durability:
    // the handler reports the double fault honestly instead of claiming the
    // old credential survived untouched.
    std::fs::write(dir_sync_fault_file(&bundle_paths), "fail").expect("arm fault");
    let failed = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "change_psk", "principal_id": peer_id}),
    );
    assert_eq!(failed["response"]["kind"], "error");
    assert!(
        [
            "internal_store_durability_uncertain",
            "internal_credential_rollback_failed"
        ]
        .contains(&failed["response"]["error"]["code"].as_str().unwrap_or("")),
        "rotation under fault must report uncertainty, never success: {failed:?}"
    );

    // After the fault clears, re-rotating restores a known-good credential.
    std::fs::remove_file(dir_sync_fault_file(&bundle_paths)).expect("clear fault");
    let rotation = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "change_psk", "principal_id": peer_id}),
    );
    assert_eq!(
        rotation["response"]["kind"], "change_psk",
        "recovery rotation rejected: {rotation:?}"
    );
    let rotated_psk = rotation["response"]["psk"]
        .as_str()
        .expect("rotated psk")
        .to_string();
    assert_ne!(rotated_psk, original_psk);
    let accepted = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        peer_id,
        &rotated_psk,
        true,
    );
    assert_eq!(
        accepted["frame"], "hello_ack",
        "recovered psk: {accepted:?}"
    );
}
