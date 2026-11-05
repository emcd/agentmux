//! Verified principal context authority, lifetime, and the
//! descriptor/context separation.
//!
//! A descriptor alone never authorizes: without a successful credential
//! verification there is no `VerifiedPrincipal` value at all (the type has
//! no public constructor), and every negative path below is rejected before
//! routing or authorization could run.

use agentmux::relay::{
    CredentialSource, IdentityDescriptor, ProvisionParams, check_context,
    introspect_principal, provision_principal, verify_identity,
};

use super::{provision_alpha, test_runtime};

/// A descriptor presented without its credential verifies nothing: the
/// failure names the credential, not a routing or authorization outcome.
#[test]
fn descriptor_without_credential_authorizes_nothing() {
    let (_temporary, runtime) = test_runtime();
    let psk = provision_alpha(&runtime);

    let missing = verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "alpha@party".to_string(),
            CredentialSource::DirectPsk("not-the-psk".to_string()),
        ),
    )
    .expect_err("a descriptor without its credential verifies nothing");
    assert_eq!(missing.code, "validation_unrecognized_credential");

    verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "alpha@party".to_string(),
            CredentialSource::DirectPsk(psk),
        ),
    )
    .expect("the same descriptor with its credential verifies");
}

/// A context minted in one runtime is rejected by another before routing or
/// authorization, and each handle's identity state stays its own.
#[test]
fn cross_instance_context_is_rejected() {
    let (_temp_a, runtime_a) = test_runtime();
    let (_temp_b, runtime_b) = test_runtime();
    let psk = provision_alpha(&runtime_a);
    let context = verify_identity(
        &runtime_a,
        &IdentityDescriptor::new(
            "alpha@party".to_string(),
            CredentialSource::DirectPsk(psk),
        ),
    )
    .expect("context mints in A");

    let foreign = check_context(&runtime_b, &context)
        .expect_err("A's context authorizes nothing in B");
    assert_eq!(foreign.code, "validation_foreign_relay_instance");
    assert!(
        introspect_principal(&runtime_b, "alpha@party")
            .expect("introspect")
            .is_none(),
        "provisioning in A leaves B's identity store untouched"
    );
    check_context(&runtime_a, &context).expect("context stays valid in A");
}

/// Home is frozen at verification time from the principal record itself.
#[test]
fn home_is_frozen_from_the_record_at_verification() {
    let (_temporary, runtime) = test_runtime();
    let psk = provision_alpha(&runtime);
    let context = verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "alpha@party".to_string(),
            CredentialSource::DirectPsk(psk),
        ),
    )
    .expect("context mints");

    // Opaque by design: no public accessor exposes home until Phase 2
    // dispatch consumes it. The derived Debug rendering is the only public
    // window, and it must show the frozen home.
    assert!(
        format!("{context:?}").contains("party"),
        "verified context carries the principal home: {context:?}"
    );
    check_context(&runtime, &context).expect("context valid");
}

/// A relay-wide principal verifies with a relay-wide home that is never a
/// bundle scope.
#[test]
fn relay_wide_home_is_global() {
    let (_temporary, runtime) = test_runtime();
    let credential = provision_principal(
        &runtime,
        ProvisionParams::new("ops@GLOBAL".to_string()),
    )
    .expect("provision relay-wide principal")
    .psk
    .expect("one-time PSK");
    let context = verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "ops@GLOBAL".to_string(),
            CredentialSource::DirectPsk(credential),
        ),
    )
    .expect("relay-wide principal verifies");

    assert!(
        format!("{context:?}").contains("GLOBAL"),
        "relay-wide home is GLOBAL, never a bundle: {context:?}"
    );
    check_context(&runtime, &context).expect("context valid");
}

/// Introspection output describes; it never authorizes. A snapshot carries
/// the principal id as data, but no API builds verified context from one —
/// presenting the id string where a context is required cannot typecheck,
/// and the only runtime path from id to context is a fresh verification.
#[test]
fn introspection_output_cannot_authorize_dispatch() {
    let (_temporary, runtime) = test_runtime();
    let psk = provision_alpha(&runtime);
    let snapshot = introspect_principal(&runtime, "alpha@party")
        .expect("introspect")
        .expect("provisioned principal is described");

    // The snapshot's principal id string is not authority: verification
    // still demands the credential the snapshot never carries.
    let without_credential = verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            snapshot.principal_id.clone(),
            CredentialSource::DirectPsk("wrong-psk".to_string()),
        ),
    )
    .expect_err("the bare id from introspection verifies nothing");
    assert_eq!(
        without_credential.code,
        "validation_unrecognized_credential"
    );
    verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            snapshot.principal_id,
            CredentialSource::DirectPsk(psk),
        ),
    )
    .expect("only the credential completes verification");
}

/// Once a runtime begins controlled shutdown, outstanding contexts stop
/// authorizing even though their bindings are unchanged.
#[test]
fn shutdown_rejects_valid_contexts() {
    let (_temporary, runtime) = test_runtime();
    let psk = provision_alpha(&runtime);
    let context = verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "alpha@party".to_string(),
            CredentialSource::DirectPsk(psk),
        ),
    )
    .expect("context mints before shutdown");
    check_context(&runtime, &context).expect("context valid while serving");

    runtime.begin_shutdown();
    let refused = check_context(&runtime, &context)
        .expect_err("shutdown rejects dispatch regardless of context");
    assert_eq!(refused.code, "runtime_shutting_down");
}
