//! Public provisioning, rotation, drop, and introspection over the relay
//! identity store, plus `direct_psk` verification against provisioned
//! records.

use agentmux::relay::{
    CredentialSource, IdentityDescriptor, ProvisionParams, check_context,
    drop_principal, introspect_principal, provision_principal,
    rotate_principal_credential, verify_identity,
};
use agentmux::runtime::paths::principal_store_path;
use std::os::unix::fs::PermissionsExt;

use super::{provision_alpha, test_runtime};
use tempfile::TempDir;

/// Reads the raw identity store file for assertions about what is (and is
/// not) persisted.
fn store_body(temporary: &TempDir) -> String {
    let state_root = temporary.path().join("state");
    std::fs::read_to_string(principal_store_path(&state_root))
        .expect("read principals.json")
}

/// A provisioned agent verifies with its one-time PSK through `direct_psk`,
/// with no Agentmux-managed credential file and no socket-trust involved.
#[test]
fn provisioned_agent_verifies_via_direct_psk() {
    let (temporary, runtime) = test_runtime();
    let psk = provision_alpha(&runtime);

    let context = verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "alpha@party".to_string(),
            CredentialSource::DirectPsk(psk.clone()),
        ),
    )
    .expect("direct_psk verifies against the provisioned record");

    assert!(
        format!("{context:?}").contains("party"),
        "verified context carries the principal home: {context:?}"
    );
    let state_root = temporary.path().join("state");
    assert!(
        !state_root.join("bundles").exists(),
        "no Agentmux-managed credential file was read or required"
    );
}

/// The raw PSK appears exactly once — in the provisioning response — and
/// never in the persisted store, so introspection cannot leak it later.
#[test]
fn raw_credential_is_returned_once_and_never_persisted() {
    let (temporary, runtime) = test_runtime();
    let psk = provision_alpha(&runtime);

    assert!(
        !store_body(&temporary).contains(psk.as_str()),
        "the store keeps hashes alone; the raw PSK is not persisted"
    );
    let snapshot = introspect_principal(&runtime, "alpha@party")
        .expect("introspect")
        .expect("provisioned principal is described");
    assert_eq!(snapshot.principal_id, "alpha@party");
    assert_eq!(snapshot.principal_type, "session");
    let snapshot_json = serde_json::to_string(&snapshot).unwrap_or_default();
    assert!(
        !snapshot_json.contains(psk.as_str()),
        "introspection carries descriptive data only, never credential material"
    );
}

/// Unknown credentials and binding mismatches are rejected before any
/// routing or authorization could run.
#[test]
fn verification_rejects_unknown_credentials_and_binding_mismatch() {
    let (_temporary, runtime) = test_runtime();
    let psk = provision_alpha(&runtime);

    let unknown = verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "alpha@party".to_string(),
            CredentialSource::DirectPsk("wrong-psk".to_string()),
        ),
    )
    .expect_err("unknown credential must be rejected");
    assert_eq!(unknown.code, "validation_unrecognized_credential");

    let mismatch = verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "bravo@party".to_string(),
            CredentialSource::DirectPsk(psk),
        ),
    )
    .expect_err("hash naming another principal must be rejected");
    assert_eq!(mismatch.code, "validation_identity_binding_mismatch");
}

/// Metadata persists on the record and verifies identically with or without
/// it: descriptive data is never authorization authority.
#[test]
fn metadata_persists_and_grants_nothing() {
    let (_temporary, runtime) = test_runtime();
    let mut params = ProvisionParams::new("alpha@party".to_string());
    params
        .metadata
        .insert("team".to_string(), "red".to_string());
    let credential = provision_principal(&runtime, params)
        .expect("provision with metadata");
    let psk = credential.psk.expect("one-time PSK");

    let snapshot = introspect_principal(&runtime, "alpha@party")
        .expect("introspect")
        .expect("record described");
    assert_eq!(
        snapshot.metadata.get("team").map(String::as_str),
        Some("red"),
        "metadata persists on the record"
    );
    verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "alpha@party".to_string(),
            CredentialSource::DirectPsk(psk),
        ),
    )
    .expect("metadata neither helps nor hinders verification");
}

/// Provisioning the same id twice is refused, and the original binding is
/// left intact.
#[test]
fn duplicate_provisioning_is_rejected_without_clobbering() {
    let (_temporary, runtime) = test_runtime();
    let psk = provision_alpha(&runtime);

    let duplicate = provision_principal(
        &runtime,
        ProvisionParams::new("alpha@party".to_string()),
    )
    .expect_err("duplicate principal must be rejected");
    assert_eq!(duplicate.code, "validation_principal_exists");
    verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "alpha@party".to_string(),
            CredentialSource::DirectPsk(psk),
        ),
    )
    .expect("original binding survives the refused duplicate");
}

/// A file-sink destination omits the PSK from the response, writes the file
/// mode 0600, and the written material verifies.
#[test]
fn file_destination_omits_psk_and_writes_mode_0600() {
    use agentmux::relay::CredentialDestination;

    let (temporary, runtime) = test_runtime();
    let credential_path = temporary
        .path()
        .join("agent.psk")
        .to_string_lossy()
        .to_string();
    let mut params = ProvisionParams::new("alpha@party".to_string());
    params.destination = CredentialDestination::Path {
        path: credential_path.clone(),
    };
    let credential =
        provision_principal(&runtime, params).expect("provision to file");
    assert!(
        credential.psk.is_none(),
        "file sinks omit the raw PSK from the response"
    );
    let written = credential.written_path.expect("written path reported");
    let metadata = std::fs::metadata(&written).expect("credential file");
    assert_eq!(
        format!("{:o}", metadata.permissions().mode() & 0o777),
        "600",
        "credential file is owner-only"
    );
    let material =
        std::fs::read_to_string(&written).expect("read credential file");
    verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "alpha@party".to_string(),
            CredentialSource::DirectPsk(material.trim().to_string()),
        ),
    )
    .expect("written material verifies");
}

/// Rotation preserves type/scope/expiry/metadata, invalidates the old
/// binding, and mints a working new one.
#[test]
fn rotation_preserves_record_and_invalidates_old_binding() {
    let (_temporary, runtime) = test_runtime();
    let psk = provision_alpha(&runtime);
    let old_context = verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "alpha@party".to_string(),
            CredentialSource::DirectPsk(psk.clone()),
        ),
    )
    .expect("old credential verifies before rotation");
    check_context(&runtime, &old_context)
        .expect("context valid before rotation");

    let rotated = rotate_principal_credential(
        &runtime,
        "alpha@party",
        agentmux::relay::CredentialDestination::Response,
    )
    .expect("rotate");
    let new_psk = rotated.psk.expect("rotated raw PSK in response");

    verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "alpha@party".to_string(),
            CredentialSource::DirectPsk(psk),
        ),
    )
    .expect_err("superseded credential no longer verifies");
    let stale = check_context(&runtime, &old_context)
        .expect_err("outstanding context dies with its binding");
    assert_eq!(stale.code, "validation_stale_verified_context");
    verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "alpha@party".to_string(),
            CredentialSource::DirectPsk(new_psk),
        ),
    )
    .expect("rotated credential verifies");
}

/// Drop removes the binding; the deleted context stays dead, and recreating
/// the id starts a new epoch rather than resurrecting the old one.
#[test]
fn drop_invalidates_and_recreation_starts_a_new_epoch() {
    let (_temporary, runtime) = test_runtime();
    let psk = provision_alpha(&runtime);
    let context = verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "alpha@party".to_string(),
            CredentialSource::DirectPsk(psk),
        ),
    )
    .expect("credential verifies before drop");

    drop_principal(&runtime, "alpha@party").expect("drop");
    assert!(
        introspect_principal(&runtime, "alpha@party")
            .expect("introspect")
            .is_none(),
        "dropped principal is undescribed"
    );
    let stale = check_context(&runtime, &context)
        .expect_err("deleted binding invalidates outstanding context");
    assert_eq!(stale.code, "validation_stale_verified_context");

    let new_psk = provision_alpha(&runtime);
    let recreated = verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "alpha@party".to_string(),
            CredentialSource::DirectPsk(new_psk),
        ),
    )
    .expect("recreated binding verifies under its new epoch");
    check_context(&runtime, &recreated).expect("new epoch context is valid");
    check_context(&runtime, &context)
        .expect_err("recreation does not resurrect the old context");
}

/// Debug rendering never leaks credential material.
#[test]
fn debug_output_redacts_secret_material() {
    let (_temporary, runtime) = test_runtime();
    let credential = provision_principal(
        &runtime,
        ProvisionParams::new("alpha@party".to_string()),
    )
    .expect("provision");
    let psk = credential.psk.as_deref().expect("one-time PSK");
    assert!(
        !format!("{credential:?}").contains(psk),
        "provisioned credential Debug must not print the raw PSK"
    );
    let descriptor = IdentityDescriptor::new(
        "alpha@party".to_string(),
        CredentialSource::DirectPsk(psk.to_string()),
    );
    assert!(
        !format!("{descriptor:?}").contains(psk),
        "descriptor Debug must not print the presented material"
    );
}

/// A relay peer provisions with its scope canonicalized (sorted, trimmed).
#[test]
fn relay_scope_is_canonicalized_on_provision() {
    let (_temporary, runtime) = test_runtime();
    let mut params = ProvisionParams::new("peer1@RELAY".to_string());
    params.scope = Some("b, a".to_string());
    provision_principal(&runtime, params).expect("provision relay peer");

    let snapshot = introspect_principal(&runtime, "peer1@RELAY")
        .expect("introspect")
        .expect("relay peer described");
    assert_eq!(
        snapshot.scope.as_deref(),
        Some("a,b"),
        "relay scope is canonicalized at provisioning"
    );
}

/// Unknown principals fail every credential operation loudly.
#[test]
fn unknown_principals_fail_credential_operations() {
    let (_temporary, runtime) = test_runtime();

    let rotated = rotate_principal_credential(
        &runtime,
        "nobody@party",
        agentmux::relay::CredentialDestination::Response,
    )
    .expect_err("rotating an unknown principal must fail");
    assert_eq!(rotated.code, "validation_unknown_principal");
    drop_principal(&runtime, "nobody@party")
        .expect_err("dropping an unknown principal must fail");
    assert!(
        introspect_principal(&runtime, "nobody@party")
            .expect("introspect")
            .is_none(),
        "unknown principals are undescribed"
    );
}
