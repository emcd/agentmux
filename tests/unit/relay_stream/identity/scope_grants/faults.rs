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
fn drop_under_dirsync_failure_revokes_and_reports_uncertainty() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_dropfault";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "dropfault@RELAY";

    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );

    // The peer stays connected so the test can observe whether revocation
    // still runs when durability is uncertain.
    let (mut client, join) = spawn_relay_connection(&configuration_roots, &bundle_paths);
    let read_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": peer_id,
            "identity_token": psk,
        }),
    );
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");

    std::fs::write(dir_sync_fault_file(&bundle_paths), "fail").expect("arm fault");
    let dropped = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "drop_peer", "principal_id": peer_id}),
    );
    std::fs::remove_file(dir_sync_fault_file(&bundle_paths)).expect("clear fault");
    assert_eq!(dropped["response"]["kind"], "error");
    assert_eq!(
        dropped["response"]["error"]["code"], "internal_store_durability_uncertain",
        "published removal must report uncertainty, not success: {dropped:?}"
    );

    // The removal is effective despite the uncertainty: the live session is
    // revoked and the credential authenticates nothing.
    let mut revoked = read_json(&mut reader);
    while revoked["frame"] != "response" {
        revoked = read_json(&mut reader);
    }
    assert_eq!(
        revoked["response"]["error"]["code"], "runtime_identity_revoked",
        "revocation must run for the effective removal: {revoked:?}"
    );
    join.join().expect("join revoked peer thread");

    let stale = hello_first_frame(&configuration_roots, &bundle_paths, peer_id, &psk, true);
    assert_eq!(stale["response"]["kind"], "error");
    assert_eq!(
        stale["response"]["error"]["code"], "validation_unrecognized_credential",
        "removed credential must not authenticate: {stale:?}"
    );
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

    // Retrying the same scope with the fault still armed reports uncertainty
    // again: the retry performs authorization and synchronization rather than
    // short-circuiting as a no-op success.
    let rearmed = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("some-other-bundle"),
    );
    assert_eq!(rearmed["response"]["kind"], "error");
    assert_eq!(
        rearmed["response"]["error"]["code"], "internal_store_durability_uncertain",
        "armed retry must synchronize, not shortcut: {rearmed:?}"
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

/// Path of the pre-rename fault-injection seam failing a store persist after
/// staging but before the rename publication point.
fn prename_fault_file(bundle_paths: &BundleRuntimePaths) -> std::path::PathBuf {
    bundle_paths
        .state_root
        .join("identity")
        .join(".fault-pre-rename")
}

#[test]
fn prename_storage_failure_leaves_old_grant_intact() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_prestorage";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "prestorage@RELAY";
    let target = format!("alpha@{bundle_name}");

    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );

    // Fail the persist after staging but before rename publication: the
    // failure is a plain store error, never durability uncertainty, and the
    // old record and effective grant are untouched.
    std::fs::write(prename_fault_file(&bundle_paths), "fail").expect("arm fault");
    let failed = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some(bundle_name),
    );
    std::fs::remove_file(prename_fault_file(&bundle_paths)).expect("clear fault");
    assert_eq!(failed["response"]["kind"], "error");
    assert_eq!(
        failed["response"]["error"]["code"], "internal_principal_store",
        "pre-rename failure must not report uncertainty: {failed:?}"
    );
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

    // Clearing the fault lets the same replacement commit normally.
    let retried = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some(bundle_name),
    );
    assert_eq!(retried["response"]["kind"], "change_scope", "{retried:?}");
}

#[test]
fn rotation_compensation_uncertainty_preserves_old_credential() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_doublefault";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "doublefault@RELAY";

    let original_psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );

    // With directory sync faulted, both the rotation and its compensation
    // publish but stay uncertain: the prior record is effective, so the
    // original uncertainty is reported and the old credential keeps working.
    std::fs::write(dir_sync_fault_file(&bundle_paths), "fail").expect("arm fault");
    let failed = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "change_psk", "principal_id": peer_id}),
    );
    assert_eq!(failed["response"]["kind"], "error");
    assert_eq!(
        failed["response"]["error"]["code"], "internal_store_durability_uncertain",
        "compensation uncertainty must report uncertainty: {failed:?}"
    );
    let live = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        peer_id,
        &original_psk,
        true,
    );
    assert_eq!(
        live["frame"], "hello_ack",
        "old credential stays effective: {live:?}"
    );

    // After the fault clears, rotating again restores a known-good credential.
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

#[test]
fn rotation_compensation_uncertainty_discards_staged_file() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_filefault";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "filefault@RELAY";
    let output_path = temporary.path().join("rotated.psk");

    let original_psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );

    // With directory sync faulted, the rotation and its compensation both
    // publish but stay uncertain: the prior record is effective, so the
    // staged credential file must be discarded rather than published against
    // the old hash — otherwise the file would authenticate nothing.
    std::fs::write(dir_sync_fault_file(&bundle_paths), "fail").expect("arm fault");
    let failed = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({
            "operation": "change_psk",
            "principal_id": peer_id,
            "destination": {"kind": "path", "path": output_path.to_string_lossy()},
        }),
    );
    assert_eq!(failed["response"]["kind"], "error");
    assert_eq!(
        failed["response"]["error"]["code"], "internal_store_durability_uncertain",
        "{failed:?}"
    );
    assert!(
        !output_path.exists(),
        "staged file must be discarded when the old credential stays effective"
    );
    let live = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        peer_id,
        &original_psk,
        true,
    );
    assert_eq!(live["frame"], "hello_ack", "old credential: {live:?}");

    // After the fault clears, the same rotation publishes the file and
    // the file authenticates against the effective store.
    std::fs::remove_file(dir_sync_fault_file(&bundle_paths)).expect("clear fault");
    let rotation = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({
            "operation": "change_psk",
            "principal_id": peer_id,
            "destination": {"kind": "path", "path": output_path.to_string_lossy()},
        }),
    );
    assert_eq!(rotation["response"]["kind"], "change_psk", "{rotation:?}");
    let file_psk = std::fs::read_to_string(&output_path).expect("read published file");
    let accepted = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        peer_id,
        file_psk.trim(),
        true,
    );
    assert_eq!(
        accepted["frame"], "hello_ack",
        "published file must authenticate against the effective store: {accepted:?}"
    );
}

#[test]
fn uncertain_registration_retry_observes_clean_unknown_principal() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_regretry";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "regretry@RELAY";

    // Registration under fault publishes but stays uncertain; compensation
    // removes it, so the retry observes a clean unknown-principal rather than
    // a half-published registration.
    std::fs::create_dir_all(bundle_paths.state_root.join("identity")).expect("create identity dir");
    std::fs::write(dir_sync_fault_file(&bundle_paths), "fail").expect("arm fault");
    let failed = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "new_peer", "principal_id": peer_id, "scope": "*"}),
    );
    assert_eq!(failed["response"]["kind"], "error");
    assert_eq!(
        failed["response"]["error"]["code"], "internal_store_durability_uncertain",
        "{failed:?}"
    );
    std::fs::remove_file(dir_sync_fault_file(&bundle_paths)).expect("clear fault");
    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );
    let accepted = hello_first_frame(&configuration_roots, &bundle_paths, peer_id, &psk, true);
    assert_eq!(accepted["frame"], "hello_ack", "{accepted:?}");
}
