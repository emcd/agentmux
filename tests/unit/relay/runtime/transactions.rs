//! Store transaction races and durability fault injection.
//!
//! The per-store transaction lock serializes every load→mutate→persist
//! sequence for the same identity store: concurrent public provisioning
//! paths cannot lose each other's updates, and a stale snapshot can never
//! restore a principal after a drop or rotation already returned. The fault
//! seams below (`.fault-pre-rename`, `.fault-dir-sync` beside the store
//! file) drive both sides of the atomic-rename publication point.
//!
//! Coverage limit, stated rather than implied: the barrier tests below gate
//! thread *starts* only. They prove that fully serialized transactions
//! neither lose nor resurrect records; they do not deterministically
//! interleave the middles of two transactions, because the lock under test
//! forbids such an interleaving by construction.

//!
//! Explicit deferral (Tier-2 carry): deterministic seam-level serialization
//! evidence across the supported mutation paths -- a controlled stale-load,
//! publication, or compensation interleaving that must block -- is not
//! provided here. Serialization instead rests on the per-store transaction
//! mutex held across the complete load-to-persist sequence at every
//! mutation site: the public provisioning paths in `relay::runtime`, the
//! socket identity-admin dispatch in `relay`, and the startup prune in
//! `relay::prune_principal_store`. Cross-process staleness is excluded by
//! the domain claim: only the domain owner may persist. Mid-transaction
//! interleaving tests remain deferred pending Tier-2 acceptance.

use std::sync::{Arc, Barrier};

use agentmux::relay::{
    CredentialSource, IdentityDescriptor, ProvisionParams, RelayRuntime,
    check_context, drop_principal, introspect_principal, provision_principal,
    rotate_principal_credential, verify_identity,
};
use tempfile::TempDir;

use super::{provision_alpha, shared_runtime, test_runtime};

/// Concurrent provisions of distinct principals all land: no lost updates.
#[test]
fn concurrent_provisions_do_not_lose_updates() {
    let (_temporary, runtime) = shared_runtime();
    let barrier = Arc::new(Barrier::new(4));
    std::thread::scope(|scope| {
        for index in 0..4 {
            let runtime = Arc::clone(&runtime);
            let barrier = Arc::clone(&barrier);
            scope.spawn(move || {
                barrier.wait();
                let mut params =
                    ProvisionParams::new(format!("agent{index}@party"));
                params.destination =
                    agentmux::relay::CredentialDestination::Response;
                provision_principal(&runtime, params)
                    .expect("concurrent provision");
            });
        }
    });
    for index in 0..4 {
        let snapshot = introspect_principal(
            &runtime,
            format!("agent{index}@party").as_str(),
        )
        .expect("introspect");
        assert!(
            snapshot.is_some(),
            "every concurrently provisioned principal is present"
        );
    }
}

/// A drop racing an unrelated provision removes exactly its own target: the
/// dropped principal stays gone (no resurrection from a stale snapshot) and
/// the unrelated registration lands.
#[test]
fn drop_vs_unrelated_provision_neither_loses_nor_resurrects() {
    let (_temporary, runtime) = shared_runtime();
    let psk = provision_alpha(&runtime);
    let victim = verify(&runtime, &psk);
    let barrier = Arc::new(Barrier::new(2));
    let bravo_psk = std::thread::scope(|scope| {
        let (runtime_a, barrier_a) =
            (Arc::clone(&runtime), Arc::clone(&barrier));
        scope.spawn(move || {
            barrier_a.wait();
            drop_principal(&runtime_a, "alpha@party")
                .expect("concurrent drop");
        });
        let (runtime_b, barrier_b) =
            (Arc::clone(&runtime), Arc::clone(&barrier));
        let bravo_psk = scope.spawn(move || {
            barrier_b.wait();
            provision_alpha_on(&runtime_b, "bravo@party")
        });
        bravo_psk.join().expect("provision thread")
    });
    assert!(
        introspect_principal(&runtime, "alpha@party")
            .expect("introspect")
            .is_none(),
        "the dropped principal stays dropped"
    );
    check_context(&runtime, &victim)
        .expect_err("dropped binding invalidates outstanding context");
    verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "bravo@party".to_string(),
            CredentialSource::DirectPsk(bravo_psk),
        ),
    )
    .expect("unrelated provision verifies");
}

/// A rotation racing an unrelated provision keeps both effects: the new
/// binding is effective and the other principal verifies.
#[test]
fn rotate_vs_unrelated_provision_keeps_both_effects() {
    let (_temporary, runtime) = shared_runtime();
    let psk = provision_alpha(&runtime);
    let old = verify(&runtime, &psk);
    let barrier = Arc::new(Barrier::new(2));
    let new_psk = std::thread::scope(|scope| {
        let (runtime_a, barrier_a) =
            (Arc::clone(&runtime), Arc::clone(&barrier));
        let rotated = scope.spawn(move || {
            barrier_a.wait();
            rotate_principal_credential(
                &runtime_a,
                "alpha@party",
                agentmux::relay::CredentialDestination::Response,
            )
            .expect("concurrent rotate")
            .psk
            .expect("rotated raw PSK")
        });
        let (runtime_b, barrier_b) =
            (Arc::clone(&runtime), Arc::clone(&barrier));
        let bravo_psk = scope.spawn(move || {
            barrier_b.wait();
            provision_alpha_on(&runtime_b, "bravo@party")
        });
        let bravo_psk = bravo_psk.join().expect("provision thread");
        let new_psk = rotated.join().expect("rotate thread");
        (bravo_psk, new_psk)
    });
    let (bravo_psk, new_psk) = new_psk;
    verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "bravo@party".to_string(),
            CredentialSource::DirectPsk(bravo_psk),
        ),
    )
    .expect("unrelated provision is unaffected");
    check_context(&runtime, &old)
        .expect_err("rotation invalidates the outstanding context");
    verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "alpha@party".to_string(),
            CredentialSource::DirectPsk(new_psk),
        ),
    )
    .expect("rotated binding is effective");
}

/// A pre-publication failure leaves the prior state untouched: the old
/// binding still verifies and its contexts still check.
#[test]
fn pre_publication_failure_leaves_prior_state_untouched() {
    let (temporary, runtime) = test_runtime();
    let psk = provision_alpha(&runtime);
    let context = verify(&runtime, &psk);
    plant_fault(&temporary, ".fault-pre-rename");

    let failed = rotate_principal_credential(
        &runtime,
        "alpha@party",
        agentmux::relay::CredentialDestination::Response,
    )
    .expect_err("pre-rename fault must fail the rotation");
    assert_ne!(failed.code, "internal_store_durability_uncertain");
    assert_ne!(failed.code, "internal_credential_uncertain");
    verify(&runtime, &psk);
    check_context(&runtime, &context)
        .expect("old context still valid after failed rotate");
    remove_fault(&temporary, ".fault-pre-rename");
}

/// A post-publication failure still revokes: the record is effectively gone,
/// outstanding contexts die, and the uncertainty (not success) is reported.
#[test]
fn post_publication_failure_still_revokes() {
    let (temporary, runtime) = test_runtime();
    let log = temporary.path().join("inscriptions.log");
    let _ =
        agentmux::runtime::inscriptions::configure_process_inscriptions(&log);
    let psk = provision_alpha(&runtime);
    let context = verify(&runtime, &psk);
    plant_fault(&temporary, ".fault-dir-sync");

    let failed = drop_principal(&runtime, "alpha@party")
        .expect_err("post-rename fault must surface uncertainty");
    assert_eq!(failed.code, "internal_store_durability_uncertain");
    assert!(
        introspect_principal(&runtime, "alpha@party")
            .expect("introspect")
            .is_none(),
        "the published removal is effective despite the uncertainty"
    );
    check_context(&runtime, &context)
        .expect_err("removed binding invalidates outstanding context");
    let log_body = std::fs::read_to_string(&log).expect("inscription log");
    assert!(
        log_body.contains("\"event\":\"relay.identity.principal_dropped\""),
        "revocation is recorded even on the uncertain path"
    );
    remove_fault(&temporary, ".fault-dir-sync");
}

/// A post-publication rotation failure compensates to the prior binding:
/// the old credential keeps verifying, outstanding contexts stay valid, and
/// the uncertainty (not silent success) is reported. A clean retry then
/// rotates normally, proving crash-durability uncertainty did not wedge the
/// principal.
#[test]
fn post_publication_rotate_compensates_to_prior_binding() {
    let (temporary, runtime) = test_runtime();
    let psk = provision_alpha(&runtime);
    let old = verify(&runtime, &psk);
    plant_fault(&temporary, ".fault-dir-sync");

    let failed = rotate_principal_credential(
        &runtime,
        "alpha@party",
        agentmux::relay::CredentialDestination::Response,
    )
    .expect_err("post-rename fault must surface uncertainty");
    assert!(
        failed.code == "internal_store_durability_uncertain"
            || failed.code == "internal_credential_uncertain",
        "uncertainty, not silent success: {}",
        failed.code
    );
    verify(&runtime, &psk);
    check_context(&runtime, &old)
        .expect("outstanding context survives compensated rotation");
    remove_fault(&temporary, ".fault-dir-sync");
    let rotated = rotate_principal_credential(
        &runtime,
        "alpha@party",
        agentmux::relay::CredentialDestination::Response,
    )
    .expect("retry after uncertainty rotates cleanly");
    let new_psk = rotated.psk.expect("rotated raw PSK");
    check_context(&runtime, &old)
        .expect_err("clean retry invalidates the outstanding context");
    verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "alpha@party".to_string(),
            CredentialSource::DirectPsk(new_psk),
        ),
    )
    .expect("retried rotation is effective");
}

/// An uncertain rotation racing an unrelated provision keeps both effects.
#[test]
fn uncertain_rotate_vs_unrelated_provision_keeps_both_effects() {
    use std::time::{Duration, Instant};

    let (temporary, runtime) = test_runtime();
    let old_psk = provision_alpha(&runtime);
    let old = verify(&runtime, &old_psk);
    plant_fault(&temporary, ".fault-dir-sync");
    let runtime = Arc::new(runtime);
    let barrier = Arc::new(Barrier::new(2));
    let (rot_tx, rot_rx) = std::sync::mpsc::channel::<()>();
    // The provision retries on durability uncertainty: the fault is shared
    // infrastructure, not a rejection of its payload, so a real client would
    // retry rather than drop the registration.
    let provision = std::thread::spawn({
        let runtime = Arc::clone(&runtime);
        let barrier = Arc::clone(&barrier);
        move || {
            barrier.wait();
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                match agentmux::relay::provision_principal(
                    &runtime,
                    ProvisionParams::new("bravo@party".to_string()),
                ) {
                    Ok(credential) => {
                        return credential.psk.expect("one-time PSK");
                    },
                    Err(error)
                        if error.code
                            == "internal_store_durability_uncertain" =>
                    {
                        assert!(
                            Instant::now() < deadline,
                            "unrelated provision must land once the fault clears"
                        );
                        std::thread::sleep(Duration::from_millis(20));
                    },
                    Err(error) => panic!(
                        "unrelated provision failed unexpectedly: {error:?}"
                    ),
                }
            }
        }
    });
    let rotate = std::thread::spawn({
        let runtime = Arc::clone(&runtime);
        let barrier = Arc::clone(&barrier);
        move || {
            barrier.wait();
            let _ = rotate_principal_credential(
                &runtime,
                "alpha@party",
                agentmux::relay::CredentialDestination::Response,
            );
            rot_tx.send(()).ok();
        }
    });
    rot_rx.recv().expect("rotate thread reports");
    remove_fault(&temporary, ".fault-dir-sync");
    rotate.join().expect("rotate thread");
    let bravo_psk = provision.join().expect("provision thread");
    // Compensation held under the transaction boundary: the uncertain
    // rotation restored the prior binding rather than leaving the rotated
    // hash half-published.
    verify(&runtime, &old_psk);
    check_context(&runtime, &old)
        .expect("outstanding context survives the compensated rotation");
    verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "bravo@party".to_string(),
            CredentialSource::DirectPsk(bravo_psk),
        ),
    )
    .expect("unrelated provision verifies");
}

/// Verifies `principal_id` with a fresh one-time credential round-trip and
/// returns the minted context.
fn verify(
    runtime: &RelayRuntime,
    psk: &str,
) -> agentmux::relay::VerifiedPrincipal {
    verify_identity(
        runtime,
        &IdentityDescriptor::new(
            "alpha@party".to_string(),
            CredentialSource::DirectPsk(psk.to_string()),
        ),
    )
    .expect("verify")
}

/// Provisions `principal_id` with an in-response credential, discarding it.
fn provision_alpha_on(runtime: &RelayRuntime, principal_id: &str) -> String {
    let params = ProvisionParams::new(principal_id.to_string());
    provision_principal(runtime, params)
        .expect("provision")
        .psk
        .expect("one-time PSK")
}

/// Plants a confined-layer fault seam beside the store file.
fn plant_fault(temporary: &TempDir, seam: &str) {
    let identity = temporary.path().join("state").join("identity");
    std::fs::create_dir_all(&identity).expect("identity directory");
    std::fs::write(identity.join(seam), b"").expect("plant fault seam");
}

/// Removes a planted fault seam.
fn remove_fault(temporary: &TempDir, seam: &str) {
    let _ = std::fs::remove_file(
        temporary.path().join("state").join("identity").join(seam),
    );
}
