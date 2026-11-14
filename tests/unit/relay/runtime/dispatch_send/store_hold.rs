//! Store-hold interleavings for public send dispatch: the new-span
//! store-mutex evidence and the keyed publication-observer lifecycle.
//! Parked fixtures live beside the other dispatch fixtures in the parent
//! module.

use super::{
    AdmitGuard, PARK_SERIAL, park_fixture, park_provisioned, provisioned_psk,
    send_fixture, verified_context,
};
use agentmux::relay::{
    RelayResponse, SendOperation, dispatch_send, rotate_principal_credential,
};

/// Unarmed publication retains nothing: a send committed with no hook
/// installed records no message ids, so ordinary production traffic —
/// socket or public — leaves zero observer state behind. Takes no
/// serial lock and installs nothing: the empty take holds regardless
/// of concurrently armed hooks for other keys.
#[test]
fn unarmed_publication_retains_nothing() {
    use agentmux::relay::test_hooks::take_published_sends;

    let fixture = send_fixture("home");
    let sender = "alpha@party".to_string();
    let psk = provisioned_psk(&fixture.runtime, &sender);
    let context = verified_context(&fixture.runtime, &sender, &psk);
    let response = dispatch_send(
        &fixture.runtime,
        &context,
        &fixture.catalog,
        &fixture.peer_manager,
        SendOperation {
            targets: vec!["bravo@party".to_string()],
            message: "hello".to_string(),
            broadcast: false,
            operation_bundle: None,
        },
    );
    assert!(
        matches!(response, RelayResponse::Send { .. }),
        "unarmed send still commits: {response:?}"
    );
    assert!(
        take_published_sends(&sender).is_empty(),
        "no observer means no retention"
    );
}

/// An armed capture ignores other senders: publications under a
/// non-matching key are dropped without storage, so one test's
/// observer never accumulates another sender's traffic.
#[test]
fn publication_capture_ignores_other_senders() {
    use agentmux::relay::test_hooks::{
        SendPublishedHook, arm_send_published_hook,
        disarm_send_published_hook, take_published_sends,
    };

    let _serial = PARK_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    disarm_send_published_hook();
    let fixture = send_fixture("home");
    arm_send_published_hook(SendPublishedHook::arm("nobody@party"));
    let sender = "alpha@party".to_string();
    let psk = provisioned_psk(&fixture.runtime, &sender);
    let context = verified_context(&fixture.runtime, &sender, &psk);
    let response = dispatch_send(
        &fixture.runtime,
        &context,
        &fixture.catalog,
        &fixture.peer_manager,
        SendOperation {
            targets: vec!["bravo@party".to_string()],
            message: "hello".to_string(),
            broadcast: false,
            operation_bundle: None,
        },
    );
    assert!(
        matches!(response, RelayResponse::Send { .. }),
        "send under a non-matching observer still commits: {response:?}"
    );
    assert!(
        take_published_sends("nobody@party").is_empty(),
        "armed key with no sends captures nothing"
    );
    assert!(
        take_published_sends(&sender).is_empty(),
        "non-matching sends are dropped without storage"
    );
    disarm_send_published_hook();
}

/// A credential rotation racing the protected span blocks on the span's
/// store hold and cannot commit until the send's local publications are
/// complete. Parks after credential revalidation with the store mutex
/// held, proves the rotation mutator reached its lock acquisition (not
/// merely spawned), requires non-completion across a scheduling bound,
/// then releases: the admitted-before-invalidation send keeps its `Send`
/// outcome with its publication recorded under the span, and the next
/// request on the stale context denies on the epoch with grants
/// unchanged. This exercises the shared send spine both entries funnel
/// through, so the hold proven here is the one default-build socket
/// sends take.
#[test]
fn rotation_waits_out_span_store_hold_before_committing() {
    use agentmux::relay::test_hooks::{
        SendAdmitGate, SendPublishedHook, arm_send_admit_commit_gate,
        arm_send_admit_gate, arm_send_published_hook,
        arm_store_mutation_wait_hook,
    };
    use std::sync::mpsc;

    let fixture = park_fixture("home", "storelock");
    let sender = format!("alpha@{}", fixture.party);
    let context = park_provisioned(&fixture);
    let context_clone = context.clone();
    let _serial = PARK_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (gate, reached, proceed) = SendAdmitGate::arm(&sender);
    arm_send_admit_gate(gate);
    let (commit_gate, commit_reached, commit_proceed) =
        SendAdmitGate::arm(&sender);
    arm_send_admit_commit_gate(commit_gate);
    arm_send_published_hook(SendPublishedHook::arm(&sender));
    let _guard = AdmitGuard;

    let parked = {
        let fixture_party = fixture.party.clone();
        let runtime = std::sync::Arc::clone(&fixture.runtime);
        let catalog = fixture.catalog.clone();
        let peer_manager = std::sync::Arc::clone(&fixture.peer_manager);
        std::thread::spawn(move || {
            dispatch_send(
                &runtime,
                &context,
                &catalog,
                &peer_manager,
                SendOperation {
                    targets: vec![format!("bravo@{fixture_party}")],
                    message: "hello".to_string(),
                    broadcast: false,
                    operation_bundle: None,
                },
            )
        })
    };
    reached
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("dispatch parked at final admission");
    proceed.send(()).expect("release into the protected span");
    commit_reached
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("dispatch parked after revalidation with the store held");
    // Arm before spawning: the arrival below must not be missable. The
    // arming is keyed by this runtime's store root and sender: a
    // rotation on any other runtime — or for any other principal —
    // neither signals nor consumes it.
    let (waiting_tx, waiting_rx) = mpsc::channel::<()>();
    arm_store_mutation_wait_hook(
        fixture.runtime.state_root(),
        &sender,
        waiting_tx,
    );
    // Unrelated-runtime rejection first: rotate a principal on a
    // different store root and require silence. This proves the waiter
    // cannot be satisfied by a neighbor mutator in a shared process;
    // the arming survives the mismatch for the real rotation below.
    let unrelated = park_fixture("home", "unrelated");
    let unrelated_sender = format!("alpha@{}", unrelated.party);
    provisioned_psk(&unrelated.runtime, &unrelated_sender);
    rotate_principal_credential(
        &unrelated.runtime,
        &unrelated_sender,
        agentmux::relay::CredentialDestination::Response,
    )
    .expect("unrelated rotation commits on its own free store");
    assert!(
        waiting_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .is_err(),
        "an unrelated runtime's arrival must not satisfy this waiter"
    );
    let (done_tx, done_rx) = mpsc::channel::<bool>();
    let rotating = {
        let runtime = std::sync::Arc::clone(&fixture.runtime);
        let principal = sender.clone();
        std::thread::spawn(move || {
            let committed = rotate_principal_credential(
                &runtime,
                &principal,
                agentmux::relay::CredentialDestination::Response,
            )
            .is_ok();
            let _ = done_tx.send(committed);
        })
    };
    waiting_rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("rotation reached store-lock acquisition while held");
    // Arrival fired immediately before the mutex acquisition, so a live
    // mutator with the acquisition one call away cannot complete across
    // this bound unless the hold fails to exclude it.
    assert!(
        done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .is_err(),
        "rotation must not commit while the span holds the store mutex"
    );
    commit_proceed.send(()).expect("release parked commit");
    let response = parked.join().expect("dispatch completes");
    let RelayResponse::Send { results, .. } = response else {
        panic!("admitted-before-invalidation commits: {response:?}");
    };
    assert_eq!(
        results.len(),
        1,
        "every accepted target publishes exactly one queued result"
    );
    assert!(
        results
            .iter()
            .all(|result| result.outcome
                == agentmux::relay::SendOutcome::Queued),
        "published work is queued, not dropped: {results:?}"
    );
    let published = agentmux::relay::test_hooks::take_published_sends(&sender);
    assert_eq!(
        published,
        vec![results[0].message_id.clone()],
        "publication records exactly this send's admission"
    );
    assert!(
        done_rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("rotation commits after the span releases"),
        "rotation succeeds once the hold releases"
    );
    rotating.join().expect("rotation thread joins");
    // Grants never changed, so this denial is purely the new epoch: the
    // old context went stale exactly when the rotation committed.
    let refused = dispatch_send(
        &fixture.runtime,
        &context_clone,
        &fixture.catalog,
        &fixture.peer_manager,
        SendOperation {
            targets: vec![format!("bravo@{}", fixture.party)],
            message: "hello".to_string(),
            broadcast: false,
            operation_bundle: None,
        },
    );
    let RelayResponse::Error { error } = refused else {
        panic!("stale-epoch dispatch must fail, got {refused:?}");
    };
    assert_eq!(
        error.code, "validation_stale_verified_context",
        "post-rotation context denies on the epoch: {error:?}"
    );
}
