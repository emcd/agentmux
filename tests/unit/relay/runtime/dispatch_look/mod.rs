//! In-process look dispatch through the public handler contract.
//!
//! Mirrors the `dispatch_send` suite shape: fusion matrix, validation
//! rejections, and wiring/brand enforcement over the same spine the
//! socket transport executes.

use agentmux::relay::{
    DispatchWiring, LookOperation, LookSnapshotPayload, RelayResponse,
    dispatch_look,
};

use super::dispatch_fixtures::{
    provisioned_psk, send_fixture, send_fixture_with_coder, verified_context,
};

/// A verified self-look returns a snapshot and pins target, requester,
/// and attribution. The ACP fixture yields the workerless empty
/// baseline with no tmux needed. The target is explicitly qualified
/// while the operation bundle names a different bundle, covering the
/// suffix > parameter branch; lines and a nonzero offset ride the
/// public boundary into the spine.
#[test]
fn verified_self_look_returns_snapshot() {
    let fixture = send_fixture_with_coder("home", "acp");
    let wiring = DispatchWiring::mirrored(&fixture.runtime, &[], true)
        .expect("mirror domain wiring");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);
    let response = dispatch_look(
        &fixture.runtime,
        &context,
        &wiring,
        LookOperation {
            target_session: "alpha@party".to_string(),
            lines: Some(5),
            offset: Some(2),
            operation_bundle: Some("other".to_string()),
        },
    );
    let RelayResponse::Look {
        requester_session,
        target_session,
        authenticated_identity,
        snapshot,
        ..
    } = response
    else {
        panic!("expected a look response, got {response:?}");
    };
    assert_eq!(requester_session, "alpha@party");
    assert_eq!(target_session, "alpha@party");
    assert_eq!(
        authenticated_identity.as_deref(),
        Some("alpha@party"),
        "success carries the verified attribution"
    );
    let LookSnapshotPayload::StructuredEntriesV1 {
        snapshot_entries, ..
    } = snapshot
    else {
        panic!("ACP baseline yields structured entries, got {snapshot:?}");
    };
    assert!(
        snapshot_entries.is_empty(),
        "no recorded updates means an empty snapshot"
    );
}

/// A nonzero offset reaches tmux validation through the public
/// boundary: dropping it would proceed to capture instead of refusing
/// with the offset gate.
#[test]
fn nonzero_offset_reaches_tmux_validation() {
    let fixture = send_fixture("home");
    let wiring = DispatchWiring::mirrored(&fixture.runtime, &[], true)
        .expect("mirror domain wiring");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);
    let response = dispatch_look(
        &fixture.runtime,
        &context,
        &wiring,
        LookOperation {
            target_session: "alpha@party".to_string(),
            lines: None,
            offset: Some(2),
            operation_bundle: None,
        },
    );
    assert_eq!(
        look_error_code(response),
        "validation_offset_unsupported",
        "the public boundary forwards offset instead of dropping it"
    );
}

/// A foreign verified context is rejected before any look work, even
/// with the dispatching runtime's own wiring and an otherwise passing
/// target. This pins this entry's own context check, which shared
/// runtime tests would not detect if dropped here.
#[test]
fn foreign_context_is_rejected_before_look() {
    let fixture = send_fixture_with_coder("home", "acp");
    let other = send_fixture_with_coder("home", "acp");
    let wiring = DispatchWiring::mirrored(&fixture.runtime, &[], true)
        .expect("mirror domain wiring");
    let other_psk = provisioned_psk(&other.runtime, "alpha@party");
    let foreign = verified_context(&other.runtime, "alpha@party", &other_psk);
    let response = dispatch_look(
        &fixture.runtime,
        &foreign,
        &wiring,
        LookOperation {
            target_session: "alpha@party".to_string(),
            lines: Some(5),
            offset: None,
            operation_bundle: None,
        },
    );
    assert_eq!(
        look_error_code(response),
        "validation_foreign_relay_instance",
        "foreign contexts fail before look dispatch"
    );
}

fn look_error_code(response: RelayResponse) -> String {
    let RelayResponse::Error { error } = response else {
        panic!("expected a look error, got {response:?}");
    };
    error.code
}

/// A bare target fuses against the explicit operation bundle. The
/// fixture policy grants `look = "self"`, so a cross-session look is
/// refused — but the refusal names the fused target, proving fusion
/// resolved the namespace before authorization ran.
#[test]
fn bare_target_fuses_against_operation_bundle() {
    let fixture = send_fixture("home");
    let wiring = DispatchWiring::mirrored(&fixture.runtime, &[], true)
        .expect("mirror domain wiring");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);
    let response = dispatch_look(
        &fixture.runtime,
        &context,
        &wiring,
        LookOperation {
            target_session: "carol".to_string(),
            lines: None,
            offset: None,
            operation_bundle: Some("other".to_string()),
        },
    );
    let RelayResponse::Error { error } = response else {
        panic!("cross-session look must be refused, got {response:?}");
    };
    assert_eq!(error.code, "authorization_forbidden");
    assert!(
        format!("{error:?}").contains("carol@other"),
        "refusal names the fused target: {error:?}"
    );
}

/// Without a parameter, a bundle-scoped home is the last default.
#[test]
fn bare_target_falls_back_to_principal_home() {
    let fixture = send_fixture("home");
    let wiring = DispatchWiring::mirrored(&fixture.runtime, &[], true)
        .expect("mirror domain wiring");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);
    let response = dispatch_look(
        &fixture.runtime,
        &context,
        &wiring,
        LookOperation {
            target_session: "bravo".to_string(),
            lines: None,
            offset: None,
            operation_bundle: None,
        },
    );
    let RelayResponse::Error { error } = response else {
        panic!("cross-session look must be refused, got {response:?}");
    };
    assert_eq!(error.code, "authorization_forbidden");
    assert!(
        format!("{error:?}").contains("bravo@party"),
        "refusal names the home-fused target: {error:?}"
    );
}

/// Relay-wide principals must qualify targets explicitly.
#[test]
fn relay_wide_bare_target_is_rejected() {
    let fixture = send_fixture("home");
    let wiring = DispatchWiring::mirrored(&fixture.runtime, &[], true)
        .expect("mirror domain wiring");
    let psk = provisioned_psk(&fixture.runtime, "op@GLOBAL");
    let context = verified_context(&fixture.runtime, "op@GLOBAL", &psk);
    let response = dispatch_look(
        &fixture.runtime,
        &context,
        &wiring,
        LookOperation {
            target_session: "alpha".to_string(),
            lines: None,
            offset: None,
            operation_bundle: None,
        },
    );
    assert_eq!(
        look_error_code(response),
        "validation_unqualified_target",
        "relay-wide bare targets are rejected, not defaulted"
    );
}

/// Window sizes outside 1..=1000 are refused before any capture work.
#[test]
fn out_of_range_lines_are_rejected() {
    let fixture = send_fixture("home");
    let wiring = DispatchWiring::mirrored(&fixture.runtime, &[], true)
        .expect("mirror domain wiring");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);
    let response = dispatch_look(
        &fixture.runtime,
        &context,
        &wiring,
        LookOperation {
            target_session: "bravo@party".to_string(),
            lines: Some(1001),
            offset: None,
            operation_bundle: None,
        },
    );
    assert_eq!(
        look_error_code(response),
        "validation_invalid_lines",
        "oversize windows are refused"
    );
}

/// Unknown sessions sort before authorization.
#[test]
fn unknown_target_is_refused() {
    let fixture = send_fixture("home");
    let wiring = DispatchWiring::mirrored(&fixture.runtime, &[], true)
        .expect("mirror domain wiring");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);
    let response = dispatch_look(
        &fixture.runtime,
        &context,
        &wiring,
        LookOperation {
            target_session: "ghost@party".to_string(),
            lines: None,
            offset: None,
            operation_bundle: None,
        },
    );
    assert_eq!(
        look_error_code(response),
        "validation_unknown_target",
        "unknown sessions are refused"
    );
}

/// Unconfigured bundles fail closed rather than resolving nowhere.
#[test]
fn unknown_bundle_is_refused() {
    let fixture = send_fixture("home");
    let wiring = DispatchWiring::mirrored(&fixture.runtime, &[], true)
        .expect("mirror domain wiring");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);
    let response = dispatch_look(
        &fixture.runtime,
        &context,
        &wiring,
        LookOperation {
            target_session: "bravo@nope".to_string(),
            lines: None,
            offset: None,
            operation_bundle: None,
        },
    );
    assert_eq!(
        look_error_code(response),
        "validation_unknown_bundle",
        "unconfigured bundles fail closed"
    );
}

/// Cross-relay look stays unsupported: no snapshot semantics exist
/// over the peer boundary.
#[test]
fn cross_relay_target_is_refused() {
    let fixture = send_fixture("home");
    let wiring = DispatchWiring::mirrored(&fixture.runtime, &[], true)
        .expect("mirror domain wiring");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);
    let response = dispatch_look(
        &fixture.runtime,
        &context,
        &wiring,
        LookOperation {
            target_session: "bravo@party!peer".to_string(),
            lines: None,
            offset: None,
            operation_bundle: None,
        },
    );
    assert_eq!(
        look_error_code(response),
        "runtime_cross_relay_unsupported",
        "cross-relay look is honestly refused"
    );
}

/// A wiring built for another runtime is refused before any look work.
#[test]
fn cross_runtime_wiring_is_refused() {
    let fixture = send_fixture("home");
    let other = send_fixture("home");
    let other_wiring = DispatchWiring::mirrored(&other.runtime, &[], true)
        .expect("mirror other domain wiring");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);
    let response = dispatch_look(
        &fixture.runtime,
        &context,
        &other_wiring,
        LookOperation {
            target_session: "bravo@party".to_string(),
            lines: None,
            offset: None,
            operation_bundle: None,
        },
    );
    assert_eq!(
        look_error_code(response),
        "validation_wiring_runtime_mismatch",
        "foreign wirings fail closed"
    );
}
