use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    configuration::{BundleConfiguration, ConfigurationRoots, load_bundle_configuration},
    runtime::inscriptions::emit_inscription,
};

use super::super::authorization::{
    AuthorizationContext, RouteAuthorization, choose_authorized_ui_sessions, has_ui_session,
    load_authorization_context, reject_cross_relay_ingress,
};
use super::super::connection::BundleCatalog;
use super::super::delivery::{QuiescenceOptions, enqueue_async_delivery};
use super::super::identity::{PrincipalType, classify_principal_id};
use super::super::lifecycle::inject_bundle_state_root;
use super::super::routing::{
    Addressing, Capability, OperationProfile, ResolvedRoute, ResolvedTarget as RouteTarget,
    resolve_send_route,
};
use super::super::{
    AsyncDeliveryTask, DeliveryPayloadMode, GLOBAL_NAMESPACE, PeerConnectionManager,
    RELAY_NAMESPACE, RelayError, RelayRequest, RelayResponse, RequestPrincipal, SCHEMA_VERSION,
    SendOutcome, SendRequestContext, SendResult, SenderReturnRoute, bare_session_id,
    canonical_session_id, map_config, relay_error,
};
use super::routed::{load_home_context, run_target_operation};
use super::sender::{SenderIdentity, resolve_sender_in_namespace};

/// Entry point for the namespace-centric send path. Destructures a `Send`
/// request and authorizes/delivers it in the requester's home namespace (its
/// bundle, or `GLOBAL`), without borrowing a peer bundle. Delivery context for
/// every target bundle — the home bundle included — comes from the catalog. See
/// `dispatch_send`.
pub(in crate::relay) fn handle_send_routed(
    home_namespace: &str,
    request: RelayRequest,
    configuration_roots: &ConfigurationRoots,
    bundle_catalog: &BundleCatalog,
    principal: Option<&RequestPrincipal>,
    peer_connection_manager: Option<&PeerConnectionManager>,
) -> Result<RelayResponse, RelayError> {
    let RelayRequest::Send {
        request_id,
        requester_session,
        message,
        targets,
        broadcast,
        quiet_window_ms,
        on_behalf_of,
    } = request
    else {
        return Err(relay_error(
            "internal_unexpected_request",
            "non-send request routed to the send dispatcher",
            None,
        ));
    };
    handle_send(
        home_namespace,
        SendRequestContext {
            request_id,
            requester_session,
            message,
            targets,
            broadcast,
            quiet_window_ms,
            on_behalf_of,
        },
        configuration_roots,
        bundle_catalog,
        principal,
        peer_connection_manager,
    )
}

fn handle_send(
    home_namespace: &str,
    request: SendRequestContext,
    configuration_roots: &ConfigurationRoots,
    bundle_catalog: &BundleCatalog,
    principal: Option<&RequestPrincipal>,
    peer_connection_manager: Option<&PeerConnectionManager>,
) -> Result<RelayResponse, RelayError> {
    // A cross-relay ingress request arrives from an authenticated peer relay
    // principal (`<id>@RELAY`): it carries no bundle policy, its home namespace is
    // `RELAY`, and it is authorized by the peer's registered ingress scope rather
    // than a policy tier. Detected from the AUTHENTICATED principal, never the
    // (spoofable) wire `requester_session`.
    let relay_ingress = principal.is_some_and(|principal| {
        classify_principal_id(principal.session_id.as_str()) == Some(PrincipalType::Relay)
    });
    let home_namespace = if relay_ingress {
        RELAY_NAMESPACE
    } else {
        home_namespace
    };
    // The sender is identified by its home namespace: a bundle namespace names the
    // home bundle; `GLOBAL`/`RELAY` are relay-wide and carry no bundle. The home
    // authorization context (operator policy, or the bundle's policy) is derived
    // from it.
    let (home_bundle, authorization) = load_home_context(home_namespace, configuration_roots)?;
    let SendRequestContext {
        request_id,
        requester_session,
        message,
        targets,
        broadcast,
        quiet_window_ms,
        on_behalf_of,
    } = request;
    // Honor a peer-forwarded origin attribution only from a relay-principal
    // (ingress) requester; a regular session cannot self-assert on_behalf_of.
    let on_behalf_of = if relay_ingress { on_behalf_of } else { None };

    if message.trim().is_empty() {
        return Err(relay_error(
            "validation_invalid_arguments",
            "message must be non-empty",
            None,
        ));
    }
    if !broadcast && targets.is_empty() {
        return Err(relay_error(
            "validation_empty_targets",
            "targets must contain at least one session",
            None,
        ));
    }
    if broadcast && !targets.is_empty() {
        return Err(relay_error(
            "validation_conflicting_targets",
            "targets must be empty when broadcast=true",
            None,
        ));
    }
    // A peer relay's own principal is the sender for an ingress request (keyed on
    // the authenticated identity); otherwise resolve the requester in its home
    // namespace, stripping any `@<home>` qualifier so internal lookups match.
    let sender = if relay_ingress {
        let principal =
            principal.expect("relay ingress is detected from an authenticated principal");
        SenderIdentity::relay_principal(principal.session_id.as_str())
    } else {
        let requester_session = bare_session_id(requester_session.as_str(), home_namespace);
        resolve_sender_in_namespace(
            home_bundle.as_ref(),
            &authorization,
            requester_session.as_str(),
            "requester_session",
        )?
    };
    // The route authorization mode: a peer relay is gated per-target by its
    // registered ingress scope (deny-by-default); every other requester by its
    // policy tier resolved in the home bundle.
    let route_authorization = if relay_ingress {
        RouteAuthorization::Ingress {
            scope: principal.and_then(|principal| principal.ingress_scope.as_deref()),
        }
    } else {
        RouteAuthorization::Policy(&authorization)
    };
    // Verified principal_id of the sender, carried both on the Send response
    // and into each recipient's delivered envelope; `None` for socket-trust.
    let authenticated_identity =
        principal.and_then(|principal| principal.authenticated_identity.clone());

    // Return route for a terminal-outcome receipt: the sender's *real* home-bundle
    // member (its true transport) plus the sender bundle's runtime directory. Built
    // from the sender's home context, never a target's, so a non-delivered outcome
    // routes a receipt back through the sender's own transport. `None` for a
    // relay-wide sender (`GLOBAL`/`RELAY`), which has no home bundle and — when a
    // UI operator — is served by the existing `delivery_outcome` stream frame.
    let sender_return_route = resolve_sender_return_route(
        home_bundle.as_ref(),
        sender.session_id.as_str(),
        home_namespace,
        bundle_catalog,
    );

    emit_inscription(
        "relay.send.request",
        &json!({
            "namespace": home_namespace,
            "requester_session": sender.session_id,
            "broadcast": broadcast,
            "target_count": targets.len(),
            "message_length": message.len(),
            "request_id": request_id.clone(),
        }),
    );

    // The spine owns resolution and authorization; `resolve_send_route_or_broadcast`
    // builds the config-free route, `prepare_send` assembles the per-namespace
    // delivery groups for the local targets (validating their existence), and
    // `execute_send` enqueues local delivery and forwards cross-relay targets to
    // their peer relays. Authorization runs over the *whole* route (local and
    // cross-relay), so a mixed send atomically requires the highest tier any
    // target demands — a cross-relay (bang-path) target always floors at `all`.
    run_target_operation(
        home_namespace,
        route_authorization,
        OperationProfile {
            capability: Capability::Send,
            addressing: Addressing::MultiTarget,
        },
        || {
            let route = resolve_send_route_or_broadcast(
                broadcast,
                home_namespace,
                &sender,
                home_bundle.as_ref(),
                &targets,
            )?;
            // A peer relay ingress requester may address only plain local targets:
            // reject any cross-relay (bang-path) target before the
            // manager-availability check, so the rejection is deterministic and a
            // peer can never chain onward through this relay to its own peers.
            if relay_ingress
                && let Some(target) = route.targets.iter().find(|target| target.is_cross_relay())
            {
                return Err(reject_cross_relay_ingress(target));
            }
            // Cross-relay forwarding needs the peer connection manager. The
            // non-stream single-bundle entry point supplies none, so a cross-relay
            // target there is reported as unavailable rather than silently dropped
            // (mirrors the Raww path). The stream path always supplies the manager.
            if peer_connection_manager.is_none()
                && route.targets.iter().any(RouteTarget::is_cross_relay)
            {
                return Err(relay_error(
                    "runtime_cross_relay_unavailable",
                    "cross-relay routing is not available on this relay",
                    None,
                ));
            }
            Ok(route)
        },
        |route| prepare_send(route, &authorization, configuration_roots, bundle_catalog),
        |route, groups| {
            execute_send(
                route,
                groups,
                SendExecutionContext {
                    sender: &sender,
                    authenticated_identity,
                    on_behalf_of,
                    sender_return_route,
                    message: message.as_str(),
                    home_namespace,
                    request_id,
                    quiet_window_ms,
                    peer_connection_manager,
                },
            )
        },
    )
}

/// Resolves the sender's own delivery context so a non-delivered terminal
/// outcome can be routed back to it as a terminal-outcome receipt. Returns the
/// sender's *real* home-bundle member (its true transport, not the synthetic
/// `SenderIdentity::to_bundle_member` Tmux stub) plus the sender bundle's runtime
/// directory. `None` when the sender has no home bundle (`GLOBAL`/`RELAY`) or is
/// not a configured bundle member (a registered UI session): those senders learn
/// terminal outcomes through the `delivery_outcome` stream frame, not a coder
/// receipt.
fn resolve_sender_return_route(
    home_bundle: Option<&BundleConfiguration>,
    sender_session: &str,
    home_namespace: &str,
    bundle_catalog: &BundleCatalog,
) -> Option<SenderReturnRoute> {
    let member = home_bundle?
        .members
        .iter()
        .find(|member| member.id == sender_session)?
        .clone();
    let runtime_directory = bundle_catalog
        .lookup(home_namespace)?
        .runtime_directory
        .clone();
    Some(SenderReturnRoute {
        member,
        runtime_directory,
    })
}

/// Builds the config-free [`ResolvedRoute`] for a `Send`: `resolve_send_route`
/// over the supplied targets, or — for a broadcast — every home-bundle member
/// except the sender. Broadcast requires a bundle-bound sender.
fn resolve_send_route_or_broadcast(
    broadcast: bool,
    home_namespace: &str,
    sender: &SenderIdentity,
    home_bundle: Option<&BundleConfiguration>,
    targets: &[String],
) -> Result<ResolvedRoute, RelayError> {
    if !broadcast {
        return resolve_send_route(home_namespace, sender.session_id.as_str(), targets);
    }
    let Some(home_bundle) = home_bundle else {
        return Err(relay_error(
            "validation_invalid_arguments",
            "broadcast requires a bundle-bound sender",
            None,
        ));
    };
    let route_targets = home_bundle
        .members
        .iter()
        .filter(|member| member.id != sender.session_id)
        .map(|member| RouteTarget {
            namespace: home_namespace.to_string(),
            session_id: Some(member.id.clone()),
            relay_id: None,
        })
        .collect();
    Ok(ResolvedRoute {
        dispatch_namespace: home_namespace.to_string(),
        requester_session: sender.session_id.clone(),
        targets: route_targets,
    })
}

/// Assembles the per-namespace delivery groups for a `Send`'s **local** targets.
/// Cross-relay (bang-path) targets are excluded — their foreign bundle is not in
/// this relay's catalog, so they are validated and delivered by the peer-forward
/// path in `execute_send`, never the local catalog. Local target existence is
/// folded into `assemble_delivery_groups`; a broadcast route's targets are
/// home-bundle members and resolve through the catalog like any other
/// bundle-bound target. Runs as the spine's `prepare` stage, before
/// authorization.
fn prepare_send(
    route: &ResolvedRoute,
    authorization: &AuthorizationContext,
    configuration_roots: &ConfigurationRoots,
    bundle_catalog: &BundleCatalog,
) -> Result<Vec<DeliveryGroup>, RelayError> {
    let local_targets = route
        .targets
        .iter()
        .filter(|target| !target.is_cross_relay())
        .cloned()
        .collect::<Vec<_>>();
    assemble_delivery_groups(
        authorization,
        configuration_roots,
        bundle_catalog,
        &local_targets,
    )
}

/// The parameters `execute_send` needs beyond the spine-supplied route and
/// delivery groups: the resolved sender, its verified identity, the message body,
/// the requester's home namespace, the request/quiescence controls, and the peer
/// connection manager used to forward cross-relay targets. Grouped so the execute
/// stage takes the route, the groups, and one context rather than a long
/// positional argument list.
struct SendExecutionContext<'a> {
    sender: &'a SenderIdentity,
    authenticated_identity: Option<String>,
    /// Origin attribution for a peer-forwarded (ingress) send; carried into each
    /// delivered envelope and the response. `None` for a locally-originated send.
    on_behalf_of: Option<String>,
    /// Return route to the sender for a terminal-outcome receipt; cloned onto each
    /// delivery task. `None` for a relay-wide sender with no home bundle.
    sender_return_route: Option<SenderReturnRoute>,
    message: &'a str,
    home_namespace: &'a str,
    request_id: Option<String>,
    quiet_window_ms: Option<u64>,
    peer_connection_manager: Option<&'a PeerConnectionManager>,
}

/// Enqueues async delivery to every authorized local target, forwards every
/// cross-relay (bang-path) target to its peer relay, and builds the merged
/// `Send` response. Runs as the spine's `execute` stage, after authorization.
/// Local and cross-relay outcomes are folded into one `results` list; a peer
/// transport or handshake failure surfaces as that target's `failed` outcome
/// rather than failing the whole send.
fn execute_send(
    route: &ResolvedRoute,
    groups: Vec<DeliveryGroup>,
    context: SendExecutionContext<'_>,
) -> Result<RelayResponse, RelayError> {
    let SendExecutionContext {
        sender,
        authenticated_identity,
        on_behalf_of,
        sender_return_route,
        message,
        home_namespace,
        request_id,
        quiet_window_ms,
        peer_connection_manager,
    } = context;
    let sender_member = sender.to_bundle_member();
    let quiescence = QuiescenceOptions::for_async(quiet_window_ms);
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
    for group in &groups {
        for target in &group.targets {
            let message_id = Uuid::new_v4().to_string();
            let task = AsyncDeliveryTask {
                bundle: group.bundle.clone(),
                sender_namespace: home_namespace.to_string(),
                sender: sender_member.clone(),
                authenticated_identity: authenticated_identity.clone(),
                on_behalf_of: on_behalf_of.clone(),
                all_target_sessions: all_recipient_sessions.clone(),
                target_session: target.session_id.clone(),
                message: message.to_string(),
                message_id: message_id.clone(),
                quiescence,
                runtime_directory: group.runtime_directory.clone(),
                payload_mode: DeliveryPayloadMode::EnvelopeMessage,
                append_enter: true,
                choice_decider_sessions: group.choice_decider_sessions.clone(),
                is_receipt: false,
                sender_return_route: sender_return_route.clone(),
            };
            enqueue_async_delivery(task)?;
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
            quiet_window_ms,
            request_id.clone(),
            // The origin's verified principal_id (None for socket-trust): stamped
            // as on_behalf_of so the peer knows who this relay forwards for.
            authenticated_identity.as_deref(),
        ));
    }
    let response = RelayResponse::Send {
        schema_version: SCHEMA_VERSION.to_string(),
        request_id,
        requester_session: canonical_session_id(sender.session_id.as_str(), home_namespace),
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

/// The origin-facing canonical id for a cross-relay target: the foreign
/// `session@bundle` re-suffixed with its bang-path `!<relay_id>` selector, so the
/// requester's merged results and co-recipient list name *where* the target was
/// forwarded rather than a bare foreign session that would collide with a local
/// bundle.
fn cross_relay_target_label(target: &RouteTarget) -> String {
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
fn forward_send_cross_relay(
    manager: &PeerConnectionManager,
    target: &RouteTarget,
    message: &str,
    quiet_window_ms: Option<u64>,
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
    let Some(requester_session) = manager.presented_principal_id(relay_id) else {
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
        quiet_window_ms,
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
            }
            None => failed_cross_relay_result(
                label,
                "internal_peer_empty_results",
                "peer relay returned no delivery result for the forwarded target",
                None,
            ),
        },
        Ok(RelayResponse::Error { error }) => cross_relay_result_from_error(label, error),
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
fn cross_relay_result_from_error(target_label: String, error: RelayError) -> SendResult {
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
fn failed_cross_relay_result(
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

/// One namespace-scoped delivery group: a target bundle's configuration plus
/// the runtime context and choice deciders used to dispatch its targets.
struct DeliveryGroup {
    bundle: BundleConfiguration,
    runtime_directory: PathBuf,
    choice_decider_sessions: Vec<String>,
    targets: Vec<ResolvedTarget>,
}

/// One validated target within a delivery group. Relay-wide (`@GLOBAL`) targets
/// land in the synthetic `GLOBAL` group; the delivery layer re-derives their
/// stream-vs-coder binding from the unified registry by canonical principal id.
struct ResolvedTarget {
    session_id: String,
}

/// Reason a `@<bundle>` target could not be resolved to a delivery group.
enum BundleGroupError {
    /// The named bundle is not configured on this relay; the caller folds this
    /// into the request's accumulated `validation_unknown_target` set.
    UnknownBundle,
    /// Loading the bundle's configuration or authorization failed.
    Relay(RelayError),
}

/// Assembles per-namespace delivery groups from an already-classified route (the
/// config-free `MultiTarget` resolution stage in `routing.rs`). Validates target
/// existence — bundle membership or a registered UI session — and folds unknown
/// targets into a single `validation_unknown_target`. Every bundle-bound target
/// (the sender's home included) resolves its delivery context from the catalog;
/// relay-wide (`@GLOBAL`) targets are delivered via the registry and land in a
/// synthetic `GLOBAL` group.
fn assemble_delivery_groups(
    home_authorization: &AuthorizationContext,
    configuration_roots: &ConfigurationRoots,
    bundle_catalog: &BundleCatalog,
    route_targets: &[RouteTarget],
) -> Result<Vec<DeliveryGroup>, RelayError> {
    let mut group_order: Vec<String> = Vec::new();
    let mut groups_by_bundle: HashMap<String, DeliveryGroup> = HashMap::new();
    let mut unknown_targets: Vec<String> = Vec::new();

    for target in route_targets {
        let session_id = target.session_id.as_deref().unwrap_or_default();
        if target.is_relay_wide() {
            // Relay-wide `@GLOBAL` target: existence is a registered UI session,
            // resolved from the sender's (operator) authorization context.
            if has_ui_session(home_authorization, session_id) {
                let group_key = ensure_relay_wide_group(&mut group_order, &mut groups_by_bundle);
                push_target(
                    &mut groups_by_bundle,
                    group_key.as_str(),
                    ResolvedTarget {
                        session_id: session_id.to_string(),
                    },
                );
            } else {
                unknown_targets.push(session_id.to_string());
            }
            continue;
        }
        let namespace = target.namespace.as_str();
        match ensure_bundle_group(
            namespace,
            configuration_roots,
            bundle_catalog,
            &mut group_order,
            &mut groups_by_bundle,
        ) {
            Ok(()) => {
                let is_member = groups_by_bundle.get(namespace).is_some_and(|group| {
                    group
                        .bundle
                        .members
                        .iter()
                        .any(|member| member.id == session_id)
                });
                if is_member {
                    push_target(
                        &mut groups_by_bundle,
                        namespace,
                        ResolvedTarget {
                            session_id: session_id.to_string(),
                        },
                    );
                } else {
                    unknown_targets.push(canonical_session_id(session_id, namespace));
                }
            }
            Err(BundleGroupError::UnknownBundle) => {
                unknown_targets.push(canonical_session_id(session_id, namespace));
            }
            Err(BundleGroupError::Relay(error)) => return Err(error),
        }
    }

    if !unknown_targets.is_empty() {
        return Err(relay_error(
            "validation_unknown_target",
            "one or more targets are not canonical configured target identifiers",
            Some(json!({ "unknown_targets": unknown_targets })),
        ));
    }

    // Preserve target-discovery order and drop any seeded group that received no
    // target (e.g. the home group when every target was a peer or relay-wide).
    Ok(group_order
        .into_iter()
        .filter_map(|namespace| groups_by_bundle.remove(&namespace))
        .filter(|group| !group.targets.is_empty())
        .collect())
}

/// Returns the delivery-group key for a relay-wide (`@GLOBAL`) target, seeding
/// the group when absent. Every sender's `@GLOBAL` targets land in the same
/// synthetic `GLOBAL` group whose bundle/runtime are inert — UI delivery routes
/// by the target's principal id through the registry.
fn ensure_relay_wide_group(
    group_order: &mut Vec<String>,
    groups_by_bundle: &mut HashMap<String, DeliveryGroup>,
) -> String {
    let key = GLOBAL_NAMESPACE.to_string();
    if !groups_by_bundle.contains_key(key.as_str()) {
        group_order.push(key.clone());
        groups_by_bundle.insert(
            key.clone(),
            DeliveryGroup {
                bundle: BundleConfiguration {
                    schema_version: SCHEMA_VERSION.to_string(),
                    bundle_name: GLOBAL_NAMESPACE.to_string(),
                    autostart: false,
                    groups: Vec::new(),
                    members: Vec::new(),
                },
                runtime_directory: PathBuf::new(),
                choice_decider_sessions: Vec::new(),
                targets: Vec::new(),
            },
        );
    }
    key
}

/// Appends a resolved target to its bundle group. The group is guaranteed to
/// exist by the time this is called.
fn push_target(
    groups_by_bundle: &mut HashMap<String, DeliveryGroup>,
    namespace: &str,
    target: ResolvedTarget,
) {
    if let Some(group) = groups_by_bundle.get_mut(namespace) {
        group.targets.push(target);
    }
}

/// Ensures a delivery group exists for `namespace`, loading the bundle's
/// configuration and authorization from the catalog when first seen. The home
/// group (when the sender is bundle-bound) and already-seen peers are seeded, so
/// they short-circuit; an unconfigured bundle is reported so the caller can fold
/// it into `validation_unknown_target`.
fn ensure_bundle_group(
    namespace: &str,
    configuration_roots: &ConfigurationRoots,
    bundle_catalog: &BundleCatalog,
    group_order: &mut Vec<String>,
    groups_by_bundle: &mut HashMap<String, DeliveryGroup>,
) -> Result<(), BundleGroupError> {
    if groups_by_bundle.contains_key(namespace) {
        return Ok(());
    }
    let Some(paths) = bundle_catalog.lookup(namespace) else {
        return Err(BundleGroupError::UnknownBundle);
    };
    let mut bundle = load_bundle_configuration(configuration_roots, namespace)
        .map_err(|error| BundleGroupError::Relay(map_config(error)))?;
    // Delivery loads its own copy of the bundle, so it needs the same
    // authoritative overwrite bring-up applies. A Pty member started lazily by
    // this delivery is spawned from exactly these members.
    inject_bundle_state_root(&mut bundle, paths.state_root.as_path());
    let authorization = load_authorization_context(configuration_roots, Some(&bundle))
        .map_err(BundleGroupError::Relay)?;
    let choice_decider_sessions = choose_authorized_ui_sessions(&authorization, &bundle);
    group_order.push(namespace.to_string());
    groups_by_bundle.insert(
        namespace.to_string(),
        DeliveryGroup {
            bundle,
            runtime_directory: paths.runtime_directory.clone(),
            choice_decider_sessions,
            targets: Vec::new(),
        },
    );
    Ok(())
}
