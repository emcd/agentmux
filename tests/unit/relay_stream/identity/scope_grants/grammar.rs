use super::*;

#[test]
fn new_peer_canonicalizes_set_scope_in_store() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_canon";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "canon@RELAY";

    register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("  zeta , alpha,alpha ,GLOBAL "),
    );

    let record = store_record(&bundle_paths, peer_id);
    assert_eq!(
        record["scope"], "GLOBAL,alpha,zeta",
        "scope must persist trimmed, sorted, and deduplicated: {record}"
    );
}

#[test]
fn new_peer_rejects_malformed_scopes() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_malformed";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    // Each case names a distinct peer so a rejected registration cannot shadow
    // a later case; every rejection must be a scope validation failure.
    let cases = [
        ("bad-empty-item@RELAY", "alpha,,beta"),
        ("bad-mixed-wildcard@RELAY", "*,alpha"),
        ("bad-exact@RELAY", "alpha@ident_scope_malformed"),
        ("bad-relay@RELAY", "RELAY"),
        ("bad-external@RELAY", "EXTERNAL"),
        ("bad-chars@RELAY", "not a namespace"),
    ];
    for (peer_id, scope) in cases {
        let response = operator_request(
            &configuration_roots,
            &bundle_paths,
            bundle_name,
            json!({"operation": "new_peer", "principal_id": peer_id, "scope": scope}),
        );
        assert_eq!(
            response["response"]["kind"], "error",
            "malformed scope {scope:?} must not register: {response:?}"
        );
        assert_eq!(
            response["response"]["error"]["code"], "validation_invalid_params",
            "malformed scope {scope:?}: {response:?}"
        );
    }
}
