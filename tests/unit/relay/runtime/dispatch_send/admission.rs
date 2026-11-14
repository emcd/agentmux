//! Final-admission interleavings for public send dispatch: parked
//! revalidation, exclusion probing, and shutdown serialization.

use super::{
    AdmitGuard, PARK_SERIAL, park_fixture, park_provisioned, send_targets,
};
use agentmux::relay::{
    RelayResponse, SendOperation, dispatch_send, rotate_principal_credential,
};

/// A credential rotation committing mid-flight denies the parked send even
/// though no grant changed: the epoch revalidation fails under the held
/// span, not the earlier check.
#[test]
fn parked_send_denies_rotation_with_unchanged_grants() {
    use agentmux::relay::test_hooks::{SendAdmitGate, arm_send_admit_gate};

    let fixture = park_fixture("home", "rot");
    let sender = format!("alpha@{}", fixture.party);
    let context = park_provisioned(&fixture);
    let _serial = PARK_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (gate, reached, proceed) = SendAdmitGate::arm(&sender);
    arm_send_admit_gate(gate);
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
    rotate_principal_credential(
        &fixture.runtime,
        &sender,
        agentmux::relay::CredentialDestination::Response,
    )
    .expect("rotate mid-flight");
    proceed.send(()).expect("release parked dispatch");
    let response = parked.join().expect("dispatch completes");
    let RelayResponse::Error { error } = response else {
        panic!("rotated send must fail, got {response:?}");
    };
    assert_eq!(
        error.code, "validation_stale_verified_context",
        "mid-flight rotation denies despite unchanged grants: {error:?}"
    );
}

/// A scope narrowing committed mid-flight denies the parked send at the
/// re-authorization step, before any admission.
#[test]
fn parked_send_denies_narrowing_before_admission() {
    use agentmux::relay::test_hooks::{SendAdmitGate, arm_send_admit_gate};

    let fixture = park_fixture("home", "narrow");
    let sender = format!("alpha@{}", fixture.party);
    let context = park_provisioned(&fixture);
    let _serial = PARK_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (gate, reached, proceed) = SendAdmitGate::arm(&sender);
    arm_send_admit_gate(gate);
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
    std::fs::write(
        fixture._temporary.path().join("conf").join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "home"
look = "self"
send = "self"
"#,
    )
    .expect("narrow the send policy mid-flight");
    proceed.send(()).expect("release parked dispatch");
    let response = parked.join().expect("dispatch completes");
    let RelayResponse::Error { error } = response else {
        panic!("narrowed send must fail, got {response:?}");
    };
    assert_eq!(
        error.code, "authorization_forbidden",
        "mid-flight narrowing denies at re-authorization: {error:?}"
    );
}

/// A shutdown committed mid-flight denies the parked send at the gate
/// recheck: no admission, no worker, no effect.
#[test]
fn parked_send_denies_shutdown_before_commit() {
    use agentmux::relay::test_hooks::{SendAdmitGate, arm_send_admit_gate};

    let fixture = park_fixture("home", "shut");
    let sender = format!("alpha@{}", fixture.party);
    let context = park_provisioned(&fixture);
    let _serial = PARK_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (gate, reached, proceed) = SendAdmitGate::arm(&sender);
    arm_send_admit_gate(gate);
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
    fixture.runtime.begin_shutdown();
    proceed.send(()).expect("release parked dispatch");
    let response = parked.join().expect("dispatch completes");
    let RelayResponse::Error { error } = response else {
        panic!("shutdown send must fail, got {response:?}");
    };
    assert_eq!(
        error.code, "runtime_shutting_down",
        "mid-flight shutdown denies before commit: {error:?}"
    );
}

/// The converse holds: a parked dispatch released without any mid-flight
/// change commits normally, with targets queued.
#[test]
fn parked_send_without_interleaving_commits() {
    use agentmux::relay::test_hooks::{SendAdmitGate, arm_send_admit_gate};

    let fixture = park_fixture("home", "clean");
    let sender = format!("alpha@{}", fixture.party);
    let context = park_provisioned(&fixture);
    let _serial = PARK_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (gate, reached, proceed) = SendAdmitGate::arm(&sender);
    arm_send_admit_gate(gate);
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
    proceed.send(()).expect("release parked dispatch");
    let response = parked.join().expect("dispatch completes");
    assert_eq!(
        send_targets(response),
        vec![format!("bravo@{}", fixture.party)],
        "uncontended commit succeeds"
    );
}

/// A membership removal committed mid-flight denies at the sender
/// re-resolution step: the fresh bundle no longer names the sender, so
/// there is no resolved identity to authorize.
#[test]
fn parked_send_denies_membership_removal_before_admission() {
    use agentmux::relay::test_hooks::{
        SendAdmitGate, arm_send_admit_gate, disarm_send_admit_commit_gate,
    };

    let fixture = park_fixture("home", "unmember");
    let sender = format!("alpha@{}", fixture.party);
    let context = park_provisioned(&fixture);
    let _serial = PARK_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (gate, reached, proceed) = SendAdmitGate::arm(&sender);
    arm_send_admit_gate(gate);
    // No commit gate is armed: the hook fires and returns immediately.
    disarm_send_admit_commit_gate();
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
    std::fs::write(
        fixture
            ._temporary
            .path()
            .join("conf")
            .join("bundles")
            .join(format!("{}.toml", fixture.party)),
        r#"
format-version = 1

[[sessions]]
id = "bravo"
name = "Bravo"
directory = "/tmp"
coder = "shell"
"#,
    )
    .expect("remove sender from bundle mid-flight");
    proceed.send(()).expect("release parked dispatch");
    let response = parked.join().expect("dispatch completes");
    let RelayResponse::Error { error } = response else {
        panic!("unmembered sender dispatch must fail, got {response:?}");
    };
    assert_eq!(
        error.code, "validation_unknown_sender",
        "membership removal denies at re-resolution: {error:?}"
    );
}

/// The exclusion probe observes the held span at the pre-commit point:
/// while parked inside the protected region, a would-be exclusive
/// acquisition fails. Removing the guard hold flips this to unheld.
#[test]
fn exclusion_probe_observes_held_span_at_pre_commit_point() {
    use agentmux::relay::test_hooks::{
        SendAdmitGate, arm_send_admit_commit_gate, arm_send_admit_gate,
    };

    let fixture = park_fixture("home", "exclude");
    let sender = format!("alpha@{}", fixture.party);
    let context = park_provisioned(&fixture);
    let _serial = PARK_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (gate, reached, proceed) = SendAdmitGate::arm(&sender);
    arm_send_admit_gate(gate);
    let (commit_gate, commit_reached, commit_proceed) =
        SendAdmitGate::arm(&sender);
    arm_send_admit_commit_gate(commit_gate);
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
        .expect("dispatch parked at pre-commit point");
    assert!(
        fixture
            .runtime
            .delivery_context()
            .admission_exclusion_for_testing(),
        "the span must hold exclusion at the pre-commit point"
    );
    commit_proceed.send(()).expect("release parked commit");
    let response = parked.join().expect("dispatch completes");
    assert!(
        matches!(response, RelayResponse::Send { .. }),
        "uncontended commit succeeds: {response:?}"
    );
}

/// Shutdown committed mid-span waits for the in-flight admission: the
/// already-admitted send commits visibly, then the gate publishes.
/// No deadlock, no dropped admission, no retroactive cancel.
#[test]
fn shutdown_during_span_waits_then_releases_barrier_after_commit() {
    use agentmux::relay::test_hooks::{
        SendAdmitGate, arm_send_admit_commit_gate, arm_send_admit_gate,
    };

    let fixture = park_fixture("home", "shutwait");
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
        .expect("dispatch parked at pre-commit point");
    let shutdown = {
        let runtime = std::sync::Arc::clone(&fixture.runtime);
        std::thread::spawn(move || runtime.begin_shutdown())
    };
    commit_proceed.send(()).expect("release parked commit");
    let response = parked.join().expect("dispatch completes");
    shutdown
        .join()
        .expect("shutdown completes without deadlock");
    assert!(
        matches!(response, RelayResponse::Send { .. }),
        "admitted-before-shutdown commits visibly: {response:?}"
    );
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
        panic!("post-shutdown dispatch must fail, got {refused:?}");
    };
    assert_eq!(
        error.code, "runtime_shutting_down",
        "the gate stays published across the barrier: {error:?}"
    );
}

/// A narrowing committed while parked at the commit gate is observed by
/// the grant re-resolution that follows the gate: the send denies even
/// though every earlier check passed.
#[test]
fn narrowing_after_commit_gate_denies_before_admission() {
    use agentmux::relay::test_hooks::{
        SendAdmitGate, arm_send_admit_commit_gate, arm_send_admit_gate,
    };

    let fixture = park_fixture("home", "late-narrow");
    let sender = format!("alpha@{}", fixture.party);
    let context = park_provisioned(&fixture);
    let _serial = PARK_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (gate, reached, proceed) = SendAdmitGate::arm(&sender);
    arm_send_admit_gate(gate);
    let (commit_gate, commit_reached, commit_proceed) =
        SendAdmitGate::arm(&sender);
    arm_send_admit_commit_gate(commit_gate);
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
        .expect("dispatch parked at pre-commit point");
    std::fs::write(
        fixture._temporary.path().join("conf").join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "home"
look = "self"
send = "self"
"#,
    )
    .expect("narrow the send policy inside the span");
    commit_proceed.send(()).expect("release parked commit");
    let response = parked.join().expect("dispatch completes");
    let RelayResponse::Error { error } = response else {
        panic!("late narrowing must deny, got {response:?}");
    };
    assert_eq!(
        error.code, "authorization_forbidden",
        "post-gate narrowing denies at re-authorization: {error:?}"
    );
}

/// Shutdown requested while a send sits inside the protected span waits
/// for the span instead of releasing past it: the waiter blocks until
/// the span releases the guard, and arrival proves the gate published
/// (the flag store precedes the wait in `signal_shutdown`).
#[test]
fn shutdown_waits_for_in_flight_span_then_releases_barrier() {
    use agentmux::relay::test_hooks::{
        SendAdmitGate, arm_send_admit_commit_gate, arm_send_admit_gate,
        arm_shutdown_wait_hook,
    };
    use std::sync::mpsc;

    let fixture = park_fixture("home", "shutwait");
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
    let (waiting_tx, waiting_rx) = mpsc::channel::<()>();
    arm_shutdown_wait_hook(waiting_tx);
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
        .expect("dispatch parked at pre-commit point");
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let shutdown = {
        let runtime = std::sync::Arc::clone(&fixture.runtime);
        std::thread::spawn(move || {
            runtime.begin_shutdown();
            let _ = done_tx.send(());
        })
    };
    waiting_rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("shutdown arrived at the guard while the span is held");
    // Arrival proves gate publication, not just waiter motion: in
    // `signal_shutdown` the published-flag store precedes the hook fire.
    assert!(
        fixture
            .runtime
            .delivery_context()
            .admission_exclusion_for_testing(),
        "the span must still hold exclusion when the waiter arrives"
    );
    assert!(
        done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .is_err(),
        "the admission barrier must not release while the span is held"
    );
    commit_proceed.send(()).expect("release parked commit");
    let response = parked.join().expect("dispatch completes");
    assert!(
        matches!(response, RelayResponse::Send { .. }),
        "admitted-before-publish commits: {response:?}"
    );
    done_rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the admission barrier releases after the span");
    shutdown.join().expect("shutdown thread joins");
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
        panic!("post-shutdown dispatch must fail, got {refused:?}");
    };
    assert_eq!(
        error.code, "runtime_shutting_down",
        "the gate stays published across the barrier: {error:?}"
    );
}

/// Shutdown arriving after the quota reservation but before queue/registry
/// publication waits out the admission barrier while the reservation is
/// held, then releases it after publication: the reservation is never
/// orphaned by the barrier. Parks at the reserved gate (post-`admit`,
/// pre-`publish`), proves the waiter arrived while the span is held, then
/// releases and requires a committed `Send` whose every result was
/// recorded as published under the span via the keyed hook contract —
/// publication actually executed — followed by barrier release and
/// post-shutdown refusal.
/// Worker-drain-to-zero belongs to the serve-path teardown sequencing
/// (global flag plus bundle stops plus `wait_for_async_delivery_shutdown`)
/// and is not claimed here: `begin_shutdown` proves the admission barrier
/// released, not that workers drained.
#[test]
fn shutdown_after_reservation_waits_for_publication() {
    use agentmux::relay::test_hooks::{
        SendAdmitGate, SendPublishedHook, arm_send_admit_reserved_gate,
        arm_send_published_hook, arm_shutdown_wait_hook,
    };
    use std::sync::mpsc;

    let fixture = park_fixture("home", "shutreserved");
    // Publication evidence comes from the keyed hook contract, not the
    // process-global inscription sink: the spine records every published
    // target under this sender's key, and the test drains exactly those
    // entries. No shared file, no owner lifetime, no cross-test
    // interference — keyed and PARK_SERIAL-serialized like the gates.
    let sender = format!("alpha@{}", fixture.party);
    let context = park_provisioned(&fixture);
    let context_clone = context.clone();
    let _serial = PARK_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (reserved_gate, reserved_reached, reserved_proceed) =
        SendAdmitGate::arm(&sender);
    arm_send_admit_reserved_gate(reserved_gate);
    arm_send_published_hook(SendPublishedHook::arm(&sender));
    let (waiting_tx, waiting_rx) = mpsc::channel::<()>();
    arm_shutdown_wait_hook(waiting_tx);
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
                    targets: vec![format!("alpha@{fixture_party}")],
                    message: "hello".to_string(),
                    broadcast: false,
                    operation_bundle: None,
                },
            )
        })
    };
    reserved_reached
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("dispatch parked after reservation, before publication");
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let shutdown = {
        let runtime = std::sync::Arc::clone(&fixture.runtime);
        std::thread::spawn(move || {
            runtime.begin_shutdown();
            let _ = done_tx.send(());
        })
    };
    waiting_rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("shutdown arrived at the guard while the reservation is held");
    // Arrival proves gate publication, not just waiter motion: in
    // `signal_shutdown` the published-flag store precedes the hook fire,
    // so a received arrival means the gate is set and the waiter is
    // blocked on the guard this span still holds.
    assert!(
        fixture
            .runtime
            .delivery_context()
            .admission_exclusion_for_testing(),
        "the span must still hold exclusion when the waiter arrives"
    );
    assert!(
        done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .is_err(),
        "the admission barrier must not release while the reservation is unpublished"
    );
    reserved_proceed.send(()).expect("release into publication");
    let response = parked.join().expect("dispatch completes");
    let RelayResponse::Send { results, .. } = response else {
        panic!("reserved-before-publish commits: {response:?}");
    };
    let published = agentmux::relay::test_hooks::take_published_sends(&sender);
    assert_eq!(
        results.len(),
        1,
        "every accepted target publishes exactly one queued result"
    );
    let [message_id] = published.as_slice() else {
        panic!(
            "publication records exactly this send's message id, got {published:?}"
        );
    };
    assert_eq!(
        message_id, &results[0].message_id,
        "the recorded publication is this send's admission"
    );
    assert!(
        results
            .iter()
            .all(|result| result.outcome
                == agentmux::relay::SendOutcome::Queued),
        "published work is queued, not dropped: {results:?}"
    );
    done_rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the admission barrier releases after publication");
    shutdown.join().expect("shutdown thread joins");
    let refused = dispatch_send(
        &fixture.runtime,
        &context_clone,
        &fixture.catalog,
        &fixture.peer_manager,
        SendOperation {
            targets: vec![format!("alpha@{}", fixture.party)],
            message: "hello".to_string(),
            broadcast: false,
            operation_bundle: None,
        },
    );
    let RelayResponse::Error { error } = refused else {
        panic!("post-barrier dispatch must fail, got {refused:?}");
    };
    assert_eq!(
        error.code, "runtime_shutting_down",
        "no admission after the barrier published: {error:?}"
    );
}
