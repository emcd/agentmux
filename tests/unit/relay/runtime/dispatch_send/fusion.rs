//! 2.1 fusion matrix and broadcast scoping for public send dispatch.

use super::super::test_runtime;
use super::{
    provisioned_psk, send_fixture, send_fixture_with_members, send_targets,
    verified_context,
};
use agentmux::relay::{
    RelayResponse, SendOperation, dispatch_send, rotate_principal_credential,
};

#[test]
fn explicit_suffix_wins_over_operation_bundle_parameter() {
    let fixture = send_fixture("all");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);

    let response = dispatch_send(
        &fixture.runtime,
        &context,
        &fixture.catalog,
        &fixture.peer_manager,
        SendOperation {
            targets: vec!["carol@other".to_string()],
            message: "hello".to_string(),
            broadcast: false,
            operation_bundle: Some("party".to_string()),
        },
    );
    assert_eq!(
        send_targets(response),
        vec!["carol@other".to_string()],
        "the explicit suffix routes to other despite the party parameter"
    );
}

/// A bare target qualifies against the operation-bundle parameter before
/// the principal home.
#[test]
fn bare_target_defaults_to_operation_bundle_parameter() {
    let fixture = send_fixture("all");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);

    let response = dispatch_send(
        &fixture.runtime,
        &context,
        &fixture.catalog,
        &fixture.peer_manager,
        SendOperation {
            targets: vec!["carol".to_string()],
            message: "hello".to_string(),
            broadcast: false,
            operation_bundle: Some("other".to_string()),
        },
    );
    assert_eq!(
        send_targets(response),
        vec!["carol@other".to_string()],
        "the bare target resolves in the parameter bundle, not the home"
    );
}

/// Without a parameter, a bare target falls back to the bundle-scoped home.
#[test]
fn bare_target_defaults_to_bundle_scoped_home() {
    let fixture = send_fixture("home");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);

    let response = dispatch_send(
        &fixture.runtime,
        &context,
        &fixture.catalog,
        &fixture.peer_manager,
        SendOperation {
            targets: vec!["bravo".to_string()],
            message: "hello".to_string(),
            broadcast: false,
            operation_bundle: None,
        },
    );
    assert_eq!(
        send_targets(response),
        vec!["bravo@party".to_string()],
        "the bare target resolves in the home bundle"
    );
}

/// A relay-wide principal without a resolvable scope is rejected, never
/// defaulted — not even to a home, since relay-wide homes are never bundle
/// scopes.
#[test]
fn bare_target_rejected_for_global_without_resolvable_scope() {
    let fixture = send_fixture("home");
    let psk = provisioned_psk(&fixture.runtime, "op@GLOBAL");
    let context = verified_context(&fixture.runtime, "op@GLOBAL", &psk);

    let response = dispatch_send(
        &fixture.runtime,
        &context,
        &fixture.catalog,
        &fixture.peer_manager,
        SendOperation {
            targets: vec!["bravo".to_string()],
            message: "hello".to_string(),
            broadcast: false,
            operation_bundle: None,
        },
    );
    let RelayResponse::Error { error } = response else {
        panic!("a scopeless relay-wide send must fail, got {response:?}");
    };
    assert_eq!(
        error.code, "validation_unqualified_target",
        "relay-wide bare targets are rejected, not defaulted: {error:?}"
    );
}

/// Broadcast without a parameter is rejected for bundle principals.
#[test]
fn broadcast_without_parameter_rejected_for_bundle_principal() {
    let fixture = send_fixture("home");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);

    let response = dispatch_send(
        &fixture.runtime,
        &context,
        &fixture.catalog,
        &fixture.peer_manager,
        SendOperation {
            targets: Vec::new(),
            message: "hello all".to_string(),
            broadcast: true,
            operation_bundle: None,
        },
    );
    let RelayResponse::Error { error } = response else {
        panic!("parameter-less broadcast must fail, got {response:?}");
    };
    assert_eq!(
        error.code, "validation_invalid_arguments",
        "broadcast requires the parameter: {error:?}"
    );
}

/// Broadcast without a parameter is rejected for relay-wide principals.
#[test]
fn broadcast_without_parameter_rejected_for_global_principal() {
    let fixture = send_fixture("home");
    let psk = provisioned_psk(&fixture.runtime, "op@GLOBAL");
    let context = verified_context(&fixture.runtime, "op@GLOBAL", &psk);

    let response = dispatch_send(
        &fixture.runtime,
        &context,
        &fixture.catalog,
        &fixture.peer_manager,
        SendOperation {
            targets: Vec::new(),
            message: "hello all".to_string(),
            broadcast: true,
            operation_bundle: None,
        },
    );
    let RelayResponse::Error { error } = response else {
        panic!("parameter-less broadcast must fail, got {response:?}");
    };
    assert_eq!(
        error.code, "validation_invalid_arguments",
        "broadcast requires the parameter: {error:?}"
    );
}

/// Broadcast with a parameter fans out to the scope bundle's members except
/// the sender.
#[test]
fn broadcast_with_parameter_fans_out_to_scope_members() {
    let fixture = send_fixture("home");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);

    let response = dispatch_send(
        &fixture.runtime,
        &context,
        &fixture.catalog,
        &fixture.peer_manager,
        SendOperation {
            targets: Vec::new(),
            message: "hello all".to_string(),
            broadcast: true,
            operation_bundle: Some("party".to_string()),
        },
    );
    assert_eq!(
        send_targets(response),
        vec!["bravo@party".to_string()],
        "broadcast reaches scope members except the sender"
    );
}

/// A foreign runtime's context authorizes nothing here.
#[test]
fn cross_instance_context_is_rejected_by_public_dispatch() {
    let fixture = send_fixture("home");
    let (_other_temporary, other_runtime) = test_runtime();
    let psk = provisioned_psk(&other_runtime, "alpha@party");
    let foreign = verified_context(&other_runtime, "alpha@party", &psk);

    let response = dispatch_send(
        &fixture.runtime,
        &foreign,
        &fixture.catalog,
        &fixture.peer_manager,
        SendOperation {
            targets: vec!["bravo@party".to_string()],
            message: "hello".to_string(),
            broadcast: false,
            operation_bundle: None,
        },
    );
    let RelayResponse::Error { error } = response else {
        panic!("foreign context dispatch must fail, got {response:?}");
    };
    assert_eq!(
        error.code, "validation_foreign_relay_instance",
        "cross-instance context is rejected: {error:?}"
    );
}

/// Rotating the binding invalidates the outstanding context at dispatch:
/// no resurrection, no stale send.
#[test]
fn rotated_binding_invalidates_outstanding_context_at_dispatch() {
    let fixture = send_fixture("home");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);
    rotate_principal_credential(
        &fixture.runtime,
        "alpha@party",
        agentmux::relay::CredentialDestination::Response,
    )
    .expect("rotate credential");

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
    let RelayResponse::Error { error } = response else {
        panic!("stale context dispatch must fail, got {response:?}");
    };
    assert_eq!(
        error.code, "validation_stale_verified_context",
        "rotation invalidates the outstanding context: {error:?}"
    );
}

/// The public handler checks the context before routing: a missing sender
/// record fails closed at the boundary, not at membership resolution.
#[test]
fn unknown_sender_fails_at_context_check() {
    let fixture = send_fixture("home");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);
    agentmux::relay::drop_principal(&fixture.runtime, "alpha@party")
        .expect("drop sender");

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
    let RelayResponse::Error { error } = response else {
        panic!("dropped sender dispatch must fail, got {response:?}");
    };
    assert_eq!(
        error.code, "validation_stale_verified_context",
        "drop invalidates the outstanding context: {error:?}"
    );
}

/// Broadcast with a foreign scope fans out to that scope's members while
/// authorization still runs in the requester home namespace: the
/// all-scope policy admits the cross-bundle fan-out.
#[test]
fn broadcast_with_foreign_scope_fans_out_under_home_authorization() {
    let fixture = send_fixture("all");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);

    let response = dispatch_send(
        &fixture.runtime,
        &context,
        &fixture.catalog,
        &fixture.peer_manager,
        SendOperation {
            targets: Vec::new(),
            message: "hello other".to_string(),
            broadcast: true,
            operation_bundle: Some("other".to_string()),
        },
    );
    assert_eq!(
        send_targets(response),
        vec!["carol@other".to_string()],
        "broadcast scope comes solely from the parameter"
    );
}
/// Broadcast carrying targets conflicts with the fan-out contract: the
/// existing send spine rejects that shape before expansion, and the public
/// handler must preserve the refusal instead of silently dropping the
/// caller-supplied list.
#[test]
fn broadcast_with_targets_is_rejected_without_fanout() {
    let fixture = send_fixture("home");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);

    for operation_bundle in
        [Some("party".to_string()), Some("other".to_string()), None]
    {
        let response = dispatch_send(
            &fixture.runtime,
            &context,
            &fixture.catalog,
            &fixture.peer_manager,
            SendOperation {
                targets: vec!["bravo@party".to_string()],
                message: "hello".to_string(),
                broadcast: true,
                operation_bundle,
            },
        );
        let RelayResponse::Error { error } = response else {
            panic!("conflicting broadcast must fail, got {response:?}");
        };
        assert_eq!(
            error.code, "validation_conflicting_targets",
            "broadcast with targets is refused: {error:?}"
        );
    }
}

/// A same-named member of a foreign scope is a different principal: only a
/// same-home member naming the sender itself is excluded from fan-out.
#[test]
fn foreign_scope_keeps_same_named_member_in_fanout() {
    let fixture = send_fixture_with_members("all", &[("alpha", "Namesake")]);
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);

    let response = dispatch_send(
        &fixture.runtime,
        &context,
        &fixture.catalog,
        &fixture.peer_manager,
        SendOperation {
            targets: Vec::new(),
            message: "hello other".to_string(),
            broadcast: true,
            operation_bundle: Some("other".to_string()),
        },
    );
    assert_eq!(
        send_targets(response),
        vec!["alpha@other".to_string(), "carol@other".to_string()],
        "the foreign namesake is a different principal and must receive"
    );
}
