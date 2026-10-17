use super::*;

#[test]
fn change_scope_updates_grant_and_returns_canonical() {
    let temporary = TempDir::new_in("/tmp").expect("short scratch space");
    let bundle_name = "ident_scope_update";
    let configuration_roots =
        write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name)
        .expect("bundle paths");
    let peer_id = "update@RELAY";

    register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some(bundle_name),
    );
    let hash_before =
        store_record(&bundle_paths, peer_id)["credential_hash"].clone();

    let updated = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some(" zeta,alpha "),
    );
    assert_eq!(
        updated["response"]["kind"], "change_scope",
        "change scope rejected: {updated:?}"
    );
    assert_eq!(updated["response"]["scope"], "alpha,zeta");

    let record = store_record(&bundle_paths, peer_id);
    assert_eq!(record["scope"], "alpha,zeta");
    assert_eq!(
        record["credential_hash"], hash_before,
        "scope replacement must preserve the credential: {record}"
    );
}

#[test]
fn change_scope_empty_clears_grant_and_repeats_safely() {
    let temporary = TempDir::new_in("/tmp").expect("short scratch space");
    let bundle_name = "ident_scope_clear";
    let configuration_roots =
        write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name)
        .expect("bundle paths");
    let peer_id = "clear@RELAY";

    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );

    for _ in 0..2 {
        let cleared = change_scope_request(
            &configuration_roots,
            &bundle_paths,
            bundle_name,
            peer_id,
            Some(""),
        );
        assert_eq!(
            cleared["response"]["kind"], "change_scope",
            "clear rejected: {cleared:?}"
        );
        assert_eq!(cleared["response"]["scope"], "");
    }
    assert!(
        store_record(&bundle_paths, peer_id).get("scope").is_none(),
        "cleared scope must persist absent"
    );

    let denied = peer_send_response(
        &configuration_roots,
        &bundle_paths,
        peer_id,
        &psk,
        &format!("alpha@{bundle_name}"),
    );
    assert_eq!(denied["response"]["kind"], "error");
    assert_eq!(
        denied["response"]["error"]["code"], "authorization_forbidden",
        "cleared peer must lose ingress rights: {denied:?}"
    );
}

#[test]
fn change_scope_requires_dedicated_authority() {
    let temporary = TempDir::new_in("/tmp").expect("short scratch space");
    let bundle_name = "ident_scope_auth";
    // The shared identity configuration grants change.psk but not
    // change.scope: rotation rights must not confer scope administration.
    let configuration_roots =
        write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name)
        .expect("bundle paths");
    let peer_id = "auth@RELAY";

    register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some(bundle_name),
    );

    let denied = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );
    assert_eq!(denied["response"]["kind"], "error");
    assert_eq!(
        denied["response"]["error"]["code"], "authorization_forbidden",
        "rotation-only caller must not change scope: {denied:?}"
    );
    assert_eq!(
        denied["response"]["error"]["details"]["capability"], "change.scope",
        "denial must identify the scope capability: {denied:?}"
    );
    assert_eq!(
        store_record(&bundle_paths, peer_id)["scope"],
        bundle_name,
        "denied update must leave the record untouched"
    );
}

#[test]
fn change_scope_unknown_peer_reports_behind_authorization() {
    let temporary = TempDir::new_in("/tmp").expect("short scratch space");
    let bundle_name = "ident_scope_unknown";
    let configuration_roots =
        write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name)
        .expect("bundle paths");

    let unknown = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        "ghost@RELAY",
        Some("*"),
    );
    assert_eq!(unknown["response"]["kind"], "error");
    assert_eq!(
        unknown["response"]["error"]["code"], "validation_unknown_principal",
        "authorized caller sees unknown-principal: {unknown:?}"
    );

    let non_relay = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &format!("alpha@{bundle_name}"),
        Some("*"),
    );
    assert_eq!(non_relay["response"]["kind"], "error");
    assert_eq!(
        non_relay["response"]["error"]["code"],
        "validation_invalid_principal_id",
        "non-relay target is rejected: {non_relay:?}"
    );
}

#[test]
fn change_scope_omitted_scope_is_invalid_not_a_clear() {
    let temporary = TempDir::new_in("/tmp").expect("short scratch space");
    let bundle_name = "ident_scope_omitted";
    let configuration_roots =
        write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name)
        .expect("bundle paths");
    let peer_id = "omitted@RELAY";

    register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some(bundle_name),
    );

    let rejected = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        None,
    );
    assert_eq!(rejected["response"]["kind"], "error");
    assert_eq!(
        rejected["response"]["error"]["code"], "validation_invalid_arguments",
        "omitted scope must not clear: {rejected:?}"
    );
    assert_eq!(
        store_record(&bundle_paths, peer_id)["scope"],
        bundle_name,
        "rejected update must preserve the grant"
    );
}
