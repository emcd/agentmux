use serde_json::Value;
use uuid::Uuid;

use crate::relay::routing::ResolvedTarget as RouteTarget;
use crate::relay::{
    PeerConnectionManager, RelayError, RelayRequest, RelayResponse,
    SendOutcome, SendResult, canonical_session_id,
};

/// The origin-facing canonical id for a cross-relay target: the foreign
/// `session@bundle` re-suffixed with its bang-path `!<relay_id>` selector, so the
/// requester's merged results and co-recipient list name *where* the target was
/// forwarded rather than a bare foreign session that would collide with a local
/// bundle.
pub(super) fn cross_relay_target_label(target: &RouteTarget) -> String {
    let relay_id = target
        .relay_id
        .as_deref()
        .expect("cross-relay target carries a relay id");
    let foreign_session = canonical_session_id(
        target
            .session_id
            .as_deref()
            .expect("cross-relay send target carries a session id"),
        target.namespace.as_str(),
    );
    format!("{foreign_session}!{relay_id}")
}

/// Forwards one cross-relay `Send` target to its peer relay and maps the outcome
/// to a single [`SendResult`] keyed by the origin-facing bang-path label.
///
/// The forwarded request presents this relay's per-peer `<connect_as>@RELAY`
/// identity as the requester and the peer's *local* `session@bundle` as the sole
/// target (the peer receives a plain local target, not the bang-path). A peer
/// that answers with a `Send` response contributes its own per-target result
/// verbatim (re-labelled to the bang-path); a peer error response or a
/// transport/handshake failure folds into a `failed` result carrying the peer's
/// or manager's reason, so one unreachable peer never fails the requester's other
/// (local or cross-relay) deliveries.
pub(super) fn forward_send_cross_relay(
    manager: &PeerConnectionManager,
    target: &RouteTarget,
    message: &str,
    request_id: Option<String>,
    on_behalf_of: Option<&str>,
) -> SendResult {
    let label = cross_relay_target_label(target);
    let relay_id = target
        .relay_id
        .as_deref()
        .expect("cross-relay target carries a relay id");
    let foreign_session = canonical_session_id(
        target
            .session_id
            .as_deref()
            .expect("cross-relay send target carries a session id"),
        target.namespace.as_str(),
    );
    let Some(requester_session) = manager.presented_principal_id(relay_id)
    else {
        return failed_cross_relay_result(
            label,
            "validation_unknown_peer",
            "no configured peer relay matches the target alias",
            None,
        );
    };
    let forwarded = RelayRequest::Send {
        request_id,
        requester_session,
        message: message.to_string(),
        targets: vec![foreign_session],
        broadcast: false,
        on_behalf_of: on_behalf_of.map(str::to_string),
    };
    match manager.forward(relay_id, &forwarded) {
        Ok(RelayResponse::Send { mut results, .. }) => match results.pop() {
            Some(mut result) => {
                // A single-target forward yields a single result; re-label it so
                // the requester sees the bang-path it addressed, not the peer's
                // local `session@bundle`.
                result.target_session = label;
                result
            },
            None => failed_cross_relay_result(
                label,
                "internal_peer_empty_results",
                "peer relay returned no delivery result for the forwarded target",
                None,
            ),
        },
        Ok(RelayResponse::Error { error }) => {
            cross_relay_result_from_error(label, error)
        },
        Ok(_) => failed_cross_relay_result(
            label,
            "internal_peer_unexpected_response",
            "peer relay returned an unexpected response kind for a forwarded send",
            None,
        ),
        Err(error) => cross_relay_result_from_error(label, error),
    }
}

/// Maps a typed [`RelayError`] the peer returned or the manager raised onto a
/// cross-relay [`SendResult`]. An unreachable/handshake failure
/// (`runtime_peer_unavailable`) becomes the distinct `peer_unavailable` outcome;
/// every other error (a peer rejection such as ingress-denied, an unknown peer,
/// or a missing credential) becomes `failed` carrying the error's reason.
pub(super) fn cross_relay_result_from_error(
    target_label: String,
    error: RelayError,
) -> SendResult {
    let outcome = if error.code == "runtime_peer_unavailable" {
        SendOutcome::PeerUnavailable
    } else {
        SendOutcome::Failed
    };
    SendResult {
        target_session: target_label,
        message_id: Uuid::new_v4().to_string(),
        outcome,
        reason_code: Some(error.code),
        reason: Some(error.message),
        details: error.details,
    }
}

/// Builds a `failed` [`SendResult`] for a cross-relay target with a fresh message
/// id and the supplied reason. Used when forwarding could not produce a peer
/// delivery outcome (transport failure, peer error response, or an internal
/// invariant violation).
pub(super) fn failed_cross_relay_result(
    target_session: String,
    reason_code: &str,
    reason: &str,
    details: Option<Value>,
) -> SendResult {
    SendResult {
        target_session,
        message_id: Uuid::new_v4().to_string(),
        outcome: SendOutcome::Failed,
        reason_code: Some(reason_code.to_string()),
        reason: Some(reason.to_string()),
        details,
    }
}
