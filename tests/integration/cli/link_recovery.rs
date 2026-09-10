//! `agentmux link peer` transport-failure recovery: install retries once
//! with the identical PSK, registration retries once with
//! second-claim disambiguation, and ambiguous issuance/paired
//! registration report link-unknown states without rotating. Shares the
//! operation-routing fakes in [`super::link`].

use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::link::*;

#[test]
fn link_peer_retries_install_once_on_transport_failure() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let sides = link_sides();
    let issuer = spawn_operation_fake(
        &issuer_socket(&sides),
        1,
        |_| Some(new_peer_response("bravo-link@RELAY", Some(ISSUER_SECRET))),
        Arc::new(Mutex::new(Vec::new())),
    );
    let installs = Arc::new(AtomicUsize::new(0));
    let destination_log = Arc::new(Mutex::new(Vec::new()));
    let destination = spawn_operation_fake(
        &destination_socket(&sides),
        3,
        {
            let installs = Arc::clone(&installs);
            move |request| match request.get("operation").and_then(Value::as_str) {
                Some("new_peer") => Some(new_peer_response("bravo@RELAY", Some(DISCARDED_MINT))),
                Some("install_peer_credential") => {
                    assert_eq!(
                        request.get("psk").and_then(Value::as_str),
                        Some(ISSUER_SECRET),
                        "retry must carry the identical PSK: {request:?}"
                    );
                    if installs.fetch_add(1, Ordering::SeqCst) == 0 {
                        None
                    } else {
                        Some(install_response(
                            "bravo",
                            "/fake/dest/state/peers/bravo.psk",
                        ))
                    }
                }
                other => panic!("unexpected destination operation: {other:?}"),
            }
        },
        Arc::clone(&destination_log),
    );

    let output = run_link(&sides, &["--alias", "bravo", "--connect-as", "bravo-link"]);
    assert!(
        output.status.success(),
        "install retry link failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        operations(&destination_log)
            .iter()
            .filter(|operation| operation.as_str() == "install_peer_credential")
            .count(),
        2,
        "transport failure must trigger exactly one same-PSK retry"
    );
    assert_no_secret(&output, &[ISSUER_SECRET, DISCARDED_MINT]);
    issuer.join().expect("join issuer fake");
    destination.join().expect("join destination fake");
}

#[test]
fn link_peer_aborts_before_issuance_on_registration_transport_failure() {
    let sides = link_sides();
    let issuer_log = Arc::new(Mutex::new(Vec::new()));
    let issuer = spawn_operation_fake(
        &issuer_socket(&sides),
        0,
        |_| panic!("issuer must not be contacted after a registration transport failure"),
        Arc::clone(&issuer_log),
    );
    let destination = spawn_operation_fake(
        &destination_socket(&sides),
        1,
        |_| None,
        Arc::new(Mutex::new(Vec::new())),
    );

    let output = run_link(&sides, &["--alias", "bravo", "--connect-as", "bravo-link"]);
    assert!(
        !output.status.success(),
        "registration transport failure link must fail"
    );
    assert!(
        operations(&issuer_log).is_empty(),
        "aborted link must not reach issuance"
    );
    issuer.join().expect("join issuer fake");
    destination.join().expect("join destination fake");
}

#[test]
fn link_peer_retries_registration_once_on_transport_failure() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let sides = link_sides();
    let issuer = spawn_operation_fake(
        &issuer_socket(&sides),
        1,
        |_| Some(new_peer_response("bravo-link@RELAY", Some(ISSUER_SECRET))),
        Arc::new(Mutex::new(Vec::new())),
    );
    let registrations = Arc::new(AtomicUsize::new(0));
    let destination = spawn_operation_fake(
        &destination_socket(&sides),
        3,
        {
            let registrations = Arc::clone(&registrations);
            move |request| match request.get("operation").and_then(Value::as_str) {
                Some("new_peer") => {
                    if registrations.fetch_add(1, Ordering::SeqCst) == 0 {
                        None
                    } else {
                        Some(new_peer_response("bravo@RELAY", Some(DISCARDED_MINT)))
                    }
                }
                Some("install_peer_credential") => Some(install_response(
                    "bravo",
                    "/fake/dest/state/peers/bravo.psk",
                )),
                other => panic!("unexpected destination operation: {other:?}"),
            }
        },
        Arc::new(Mutex::new(Vec::new())),
    );

    let output = run_link(&sides, &["--alias", "bravo", "--connect-as", "bravo-link"]);
    assert!(
        output.status.success(),
        "registration retry link failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_no_secret(&output, &[ISSUER_SECRET, DISCARDED_MINT]);
    issuer.join().expect("join issuer fake");
    destination.join().expect("join destination fake");
}

#[test]
fn link_peer_reports_unknown_on_issuance_transport_failure() {
    let sides = link_sides();
    let issuer_log = Arc::new(Mutex::new(Vec::new()));
    let issuer = spawn_operation_fake(
        &issuer_socket(&sides),
        1,
        |request| {
            assert_eq!(
                request.get("operation").and_then(Value::as_str),
                Some("new_peer"),
                "only issuance may precede the unknown report: {request:?}"
            );
            None
        },
        Arc::clone(&issuer_log),
    );
    let destination = spawn_operation_fake(
        &destination_socket(&sides),
        1,
        |request| match request.get("operation").and_then(Value::as_str) {
            Some("new_peer") => Some(new_peer_response("bravo@RELAY", Some(DISCARDED_MINT))),
            other => panic!("install must not follow unknown issuance: {other:?}"),
        },
        Arc::new(Mutex::new(Vec::new())),
    );

    let output = run_link(&sides, &["--alias", "bravo", "--connect-as", "bravo-link"]);
    assert!(!output.status.success(), "unknown issuance link must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("link_issuance_unknown"),
        "must report issuance-unknown for explicit recovery: {stderr}"
    );
    assert_eq!(
        operations(&issuer_log),
        vec!["new_peer".to_string()],
        "no change_psk may follow ambiguous issuance"
    );
    issuer.join().expect("join issuer fake");
    destination.join().expect("join destination fake");
}

#[test]
fn link_peer_paired_reports_unknown_without_rotating() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let sides = link_sides();
    let registrations = Arc::new(AtomicUsize::new(0));
    let issuer_log = Arc::new(Mutex::new(Vec::new()));
    let issuer = spawn_operation_fake(
        &issuer_socket(&sides),
        2,
        {
            let registrations = Arc::clone(&registrations);
            move |request| match request.get("operation").and_then(Value::as_str) {
                Some("new_peer") => {
                    if registrations.fetch_add(1, Ordering::SeqCst) == 0 {
                        None
                    } else {
                        Some(relay_error_response(
                            "validation_principal_exists",
                            "principal_id is already registered",
                        ))
                    }
                }
                other => panic!("no change_psk may follow ambiguous registration: {other:?}"),
            }
        },
        Arc::clone(&issuer_log),
    );
    let destination = spawn_operation_fake(
        &destination_socket(&sides),
        0,
        |_| panic!("destination must not be contacted after unknown registration"),
        Arc::new(Mutex::new(Vec::new())),
    );

    let output = run_link(
        &sides,
        &[
            "--alias",
            "bravo",
            "--connect-as",
            "alpha",
            "--paired",
            "--peer-alias",
            "alpha",
            "--peer-connect-as",
            "bravo",
        ],
    );
    assert!(
        !output.status.success(),
        "unknown registration link must fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("link_registration_unknown"),
        "must report registration-unknown for explicit recovery: {stderr}"
    );
    assert_eq!(
        operations(&issuer_log),
        vec!["new_peer".to_string(), "new_peer".to_string()],
        "ambiguous registration must never rotate"
    );
    issuer.join().expect("join issuer fake");
    destination.join().expect("join destination fake");
}
