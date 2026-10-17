use serde_json::json;
use uuid::Uuid;

use crate::relay::delivery::admission::{
    AdmissionTargetKey, admit, canonical_payload_bytes,
    resolve_target_session_type, rollback_admission,
};
use crate::relay::delivery::enqueue_async_delivery;
use crate::relay::handlers::sender::SenderIdentity;
use crate::relay::routing::ResolvedRoute;
use crate::relay::{
    AsyncDeliveryTask, DeliveryPayloadMode, PeerConnectionManager, RelayError,
    RelayResponse, SCHEMA_VERSION, SendOutcome, SendResult, SenderReturnRoute,
    canonical_session_id, relay_error,
};
use crate::runtime::inscriptions::emit_inscription;

use super::cross_relay::{cross_relay_target_label, forward_send_cross_relay};
use super::groups::DeliveryGroup;

/// The parameters `execute_send` needs beyond the spine-supplied route and
/// delivery groups: the resolved sender, its verified identity, the message body,
/// the requester's home namespace, the request/quiescence controls, and the peer
/// connection manager used to forward cross-relay targets. Grouped so the execute
/// stage takes the route, the groups, and one context rather than a long
/// positional argument list.
pub(super) struct SendExecutionContext<'a> {
    pub(super) sender: &'a SenderIdentity,
    pub(super) authenticated_identity: Option<String>,
    /// Identity the sender's connection was admitted under, credential-backed or
    /// not. Stamped as `on_behalf_of` on a cross-relay forward; never read for
    /// local delivery, which attributes from the sender's own member record.
    pub(super) admitted_identity: Option<String>,
    /// Origin attribution for a peer-forwarded (ingress) send; carried into each
    /// delivered envelope and the response. `None` for a locally-originated send.
    pub(super) on_behalf_of: Option<String>,
    /// Return route to the sender for a terminal-outcome receipt; cloned onto each
    /// delivery task. `None` for a relay-wide sender with no home bundle.
    pub(super) sender_return_route: Option<SenderReturnRoute>,
    pub(super) message: &'a str,
    pub(super) home_namespace: &'a str,
    pub(super) request_id: Option<String>,
    pub(super) peer_connection_manager: Option<&'a PeerConnectionManager>,
}

/// Enqueues async delivery to every authorized local target, forwards every
/// cross-relay (bang-path) target to its peer relay, and builds the merged
/// `Send` response. Runs as the spine's `execute` stage, after authorization.
/// Local and cross-relay outcomes are folded into one `results` list; a peer
/// transport or handshake failure surfaces as that target's `failed` outcome
/// rather than failing the whole send.
pub(super) fn execute_send(
    route: &ResolvedRoute,
    groups: Vec<DeliveryGroup>,
    context: SendExecutionContext<'_>,
) -> Result<RelayResponse, RelayError> {
    let SendExecutionContext {
        sender,
        authenticated_identity,
        admitted_identity,
        on_behalf_of,
        sender_return_route,
        message,
        home_namespace,
        request_id,
        peer_connection_manager,
    } = context;
    let sender_member = sender.to_bundle_member();
    let mut results = Vec::with_capacity(route.targets.len());
    // Every task carries the full recipient list so delivered envelopes can show
    // co-recipients elsewhere. Entries are canonical ids: local recipients as
    // `session@namespace`, cross-relay recipients as the `session@bundle!relay`
    // bang-path — bare ids are ambiguous outside their own group.
    let mut all_recipient_sessions = groups
        .iter()
        .flat_map(|group| {
            group.targets.iter().map(|target| {
                canonical_session_id(
                    target.session_id.as_str(),
                    group.bundle.bundle_name.as_str(),
                )
            })
        })
        .collect::<Vec<_>>();
    all_recipient_sessions.extend(
        route
            .targets
            .iter()
            .filter(|target| target.is_cross_relay())
            .map(cross_relay_target_label),
    );
    // Admission precedes every queue entry in this request. Reserving the whole
    // request's quota up front is what makes a refusal clean: a request that
    // cannot be admitted holds nothing and queues nothing, rather than leaving a
    // half-admitted send behind for the caller to reason about.
    let admitted = admit_request_targets(&groups, message)?;
    let mut admitted_index = 0usize;
    for group in &groups {
        for target in &group.targets {
            let message_id = admitted[admitted_index].clone();
            let task = AsyncDeliveryTask {
                bundle: group.bundle.clone(),
                admitted: true,
                sender_namespace: home_namespace.to_string(),
                sender: sender_member.clone(),
                authenticated_identity: authenticated_identity.clone(),
                on_behalf_of: on_behalf_of.clone(),
                all_target_sessions: all_recipient_sessions.clone(),
                target_session: target.session_id.clone(),
                message: message.to_string(),
                message_id: message_id.clone(),
                runtime_directory: group.runtime_directory.clone(),
                payload_mode: DeliveryPayloadMode::EnvelopeMessage,
                append_enter: true,
                choice_decider_sessions: group.choice_decider_sessions.clone(),
                is_receipt: false,
                sender_return_route: sender_return_route.clone(),
            };
            if let Err(error) = enqueue_async_delivery(task) {
                // This entry never became a queue entry, and neither will the
                // ones behind it in this request. Release every reservation with
                // no task to terminalize it; entries already queued are live and
                // release on their own terminal transition.
                for orphaned in &admitted[admitted_index..] {
                    rollback_admission(orphaned);
                }
                return Err(error);
            }
            admitted_index += 1;
            emit_inscription(
                "relay.send.async.queued",
                &json!({
                    "namespace": group.bundle.bundle_name,
                    "sender_session": sender.session_id,
                    "target_session": target.session_id,
                    "message_id": message_id,
                }),
            );
            results.push(SendResult {
                target_session: canonical_session_id(
                    target.session_id.as_str(),
                    group.bundle.bundle_name.as_str(),
                ),
                message_id,
                outcome: SendOutcome::Queued,
                reason_code: None,
                reason: None,
                details: None,
            });
        }
    }
    // Forward each cross-relay target to its peer relay and fold the peer's
    // outcome (or a transport failure) into the merged results. The manager is
    // present here: a cross-relay target on a manager-less entry point was already
    // rejected at the resolve stage.
    for target in route
        .targets
        .iter()
        .filter(|target| target.is_cross_relay())
    {
        let Some(manager) = peer_connection_manager else {
            return Err(relay_error(
                "runtime_cross_relay_unavailable",
                "cross-relay routing is not available on this relay",
                None,
            ));
        };
        results.push(forward_send_cross_relay(
            manager,
            target,
            message,
            request_id.clone(),
            // The identity the origin was admitted under, stamped as
            // on_behalf_of so the peer knows who this relay forwards for.
            admitted_identity.as_deref(),
        ));
    }
    let response = RelayResponse::Send {
        schema_version: SCHEMA_VERSION.to_string(),
        request_id,
        requester_session: canonical_session_id(
            sender.session_id.as_str(),
            home_namespace,
        ),
        sender_display_name: sender.display_name.clone(),
        authenticated_identity: authenticated_identity.clone(),
        on_behalf_of,
        results,
    };
    if let RelayResponse::Send {
        requester_session,
        results,
        ..
    } = &response
    {
        let delivered_count = results
            .iter()
            .filter(|result| result.outcome == SendOutcome::Delivered)
            .count();
        emit_inscription(
            "relay.send.response",
            &json!({
            "namespace": home_namespace,
            "requester_session": requester_session,
            "result_count": results.len(),
            "delivered_count": delivered_count,
            }),
        );
    }
    Ok(response)
}

/// Reserves admission quota for every local target of this request, returning
/// each entry's message id in group-then-target iteration order so the queueing
/// pass can pair them back up.
///
/// All-or-nothing. The first refusal — a Pubsub target, an envelope larger than
/// the target's transport will ever accept, or exhausted quota — releases
/// everything this request already reserved and surfaces the structured error to
/// the caller, so nothing is queued for a send the relay did not accept whole.
///
/// Cross-relay targets are not admitted here: they are forwarded to their peer
/// relay rather than queued locally, and the peer admits them against its own
/// quota.
pub(super) fn admit_request_targets(
    groups: &[DeliveryGroup],
    message: &str,
) -> Result<Vec<String>, RelayError> {
    let canonical_bytes = canonical_payload_bytes(message);
    let mut admitted: Vec<String> = Vec::new();
    for group in groups {
        for target in &group.targets {
            let message_id = Uuid::new_v4().to_string();
            let admission = resolve_target_session_type(
                &group.bundle,
                target.session_id.as_str(),
            )
            .and_then(|session_type| {
                admit(
                    message_id.as_str(),
                    AdmissionTargetKey::new(
                        group.bundle.bundle_name.as_str(),
                        group.runtime_directory.as_path(),
                        target.session_id.as_str(),
                    ),
                    session_type,
                    canonical_bytes,
                )
            });
            match admission {
                Ok(()) => admitted.push(message_id),
                Err(error) => {
                    for reserved in &admitted {
                        rollback_admission(reserved);
                    }
                    return Err(error);
                },
            }
        }
    }
    Ok(admitted)
}
