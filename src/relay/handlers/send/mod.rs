use serde_json::json;
use std::path::Path;
use std::sync::Arc;

use crate::configuration::ConfigurationRoots;
use crate::relay::authorization::{
    RouteAuthorization, reject_cross_relay_ingress,
};
use crate::relay::delivery::DeliveryContext;
use crate::relay::handlers::routed::{
    load_home_context, run_target_operation,
};
use crate::relay::handlers::sender::{
    SenderIdentity, resolve_sender_in_namespace,
};
use crate::relay::identity::{
    PrincipalStore, PrincipalType, classify_principal_id, live_peer_ingress,
    lock_store_transaction,
};
use crate::relay::routing::{
    Addressing, Capability, OperationProfile, ResolvedTarget as RouteTarget,
};
use crate::relay::{
    BundleCatalog, PeerConnectionManager, PeerIngressAuthority,
    RELAY_NAMESPACE, RelayError, RelayRequest, RelayResponse,
    RequestPrincipal, SendRequestContext, bare_session_id, relay_error,
};
use crate::runtime::inscriptions::emit_inscription;

use super::super::test_hooks;

mod cross_relay;
mod execute;
mod groups;
mod route;

use execute::{
    SendExecutionContext, admit_request_targets, execute_send,
    publish_local_deliveries,
};
use route::{
    prepare_send, resolve_send_route_or_broadcast, resolve_sender_return_route,
};

/// Entry point for the namespace-centric send path. Destructures a `Send`
/// request and authorizes/delivers it in the requester's home namespace (its
/// bundle, or `GLOBAL`), without borrowing a peer bundle. Delivery context for
/// every target bundle — the home bundle included — comes from the catalog. See
/// `dispatch_send`.
#[allow(clippy::too_many_arguments)]
pub(in crate::relay) fn handle_send_routed(
    delivery: &Arc<DeliveryContext>,
    home_namespace: &str,
    request: RelayRequest,
    configuration_roots: &ConfigurationRoots,
    bundle_catalog: &BundleCatalog,
    principal: Option<&RequestPrincipal>,
    peer_connection_manager: Option<&PeerConnectionManager>,
    ingress_authority: Option<PeerIngressAuthority<'_>>,
    state_root: &Path,
) -> Result<RelayResponse, RelayError> {
    let RelayRequest::Send {
        request_id,
        requester_session,
        message,
        targets,
        broadcast,
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
        delivery,
        home_namespace,
        SendRequestContext {
            request_id,
            requester_session,
            message,
            targets,
            broadcast,
            on_behalf_of,
        },
        configuration_roots,
        bundle_catalog,
        principal,
        peer_connection_manager,
        ingress_authority,
        state_root,
    )
}

/// Revalidates the sender's credential binding against the live store
/// inside the final admission span: the record must still exist, carry
/// the presented epoch, and be unexpired. Credential-free (socket-trust)
/// senders carry no binding and skip this check; live-stream revocation
/// remains their invalidation mechanism.
fn revalidate_sender_binding(
    state_root: &Path,
    principal: Option<&RequestPrincipal>,
) -> Result<(), RelayError> {
    let Some(principal) = principal else {
        return Ok(());
    };
    let Some(authenticated) = principal.authenticated_identity.as_deref()
    else {
        return Ok(());
    };
    let store = PrincipalStore::load(state_root)?;
    let Some(record) = store.find_by_principal_id(authenticated) else {
        return Err(relay_error(
            "validation_stale_verified_context",
            "principal binding no longer exists under this identity store",
            Some(json!({ "principal_id": authenticated })),
        ));
    };
    let presented = principal.credential_hash.as_deref().unwrap_or("");
    if record.credential_hash != presented {
        return Err(relay_error(
            "validation_stale_verified_context",
            "principal credential binding changed since this context was minted",
            Some(json!({ "principal_id": authenticated })),
        ));
    }
    if record.is_expired(time::OffsetDateTime::now_utc()) {
        return Err(relay_error(
            "runtime_identity_expired",
            "principal credential has expired",
            Some(json!({ "principal_id": authenticated })),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_send(
    delivery: &Arc<DeliveryContext>,
    home_namespace: &str,
    request: SendRequestContext,
    configuration_roots: &ConfigurationRoots,
    bundle_catalog: &BundleCatalog,
    principal: Option<&RequestPrincipal>,
    peer_connection_manager: Option<&PeerConnectionManager>,
    ingress_authority: Option<PeerIngressAuthority<'_>>,
    state_root: &Path,
) -> Result<RelayResponse, RelayError> {
    // A cross-relay ingress request arrives from an authenticated peer relay
    // principal (`<id>@RELAY`): it carries no bundle policy, its home namespace is
    // `RELAY`, and it is authorized by the peer's registered ingress scope rather
    // than a policy tier. Detected from the AUTHENTICATED principal, never the
    // (spoofable) wire `requester_session`.
    let relay_ingress = principal.is_some_and(|principal| {
        classify_principal_id(principal.session_id.as_str())
            == Some(PrincipalType::Relay)
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
    let (home_bundle, authorization) =
        load_home_context(home_namespace, configuration_roots)?;
    let SendRequestContext {
        request_id,
        requester_session,
        message,
        targets,
        broadcast,
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
        let principal = principal.expect(
            "relay ingress is detected from an authenticated principal",
        );
        SenderIdentity::relay_principal(principal.session_id.as_str())
    } else {
        let requester_session =
            bare_session_id(requester_session.as_str(), home_namespace);
        resolve_sender_in_namespace(
            home_bundle.as_ref(),
            &authorization,
            requester_session.as_str(),
            "requester_session",
        )?
    };
    // The route authorization mode: a peer relay is gated per-target by its
    // live ingress scope (deny-by-default), resolved from the current store
    // record under the shared identity-admin serialization and held across
    // authorization and admission below; every other requester by its policy
    // tier resolved in the home bundle.
    // ORDERING: the test-authority gate pauses here — before the final
    // authority resolution below — so a test can commit a scope update
    // mid-flight. Resolution MUST stay after the gate: resolving first
    // would authorize under a grant the update then replaces, admitting
    // under an obsolete decision (verified by negative control: an unlocked
    // pre-gate resolution admits, failing gated_update_before_final_admission_denies).
    let live_ingress = if relay_ingress {
        let peer = principal.expect(
            "relay ingress is detected from an authenticated principal",
        );
        test_hooks::test_authority_gate(peer.session_id.as_str());
        Some(live_peer_ingress(
            ingress_authority,
            peer.session_id.as_str(),
            peer.credential_hash.as_deref(),
        )?)
    } else {
        None
    };
    let route_authorization = if relay_ingress {
        RouteAuthorization::Ingress {
            scope: live_ingress
                .as_ref()
                .and_then(|ingress| ingress.scope.as_deref()),
        }
    } else {
        RouteAuthorization::Policy(&authorization)
    };
    // Verified principal_id of the sender, carried both on the Send response
    // and into each recipient's delivered envelope; `None` for socket-trust.
    let authenticated_identity = principal
        .and_then(|principal| principal.authenticated_identity.clone());
    // The identity this connection was admitted under, credential-backed or not.
    // Cross-relay forwarding attributes the origin from this rather than from
    // the verified identity, so a relay accepting socket-trust still names the
    // sender to a peer. Kept separate from `authenticated_identity` above, which
    // continues to answer whether a credential backed the claim.
    let admitted_identity =
        principal.and_then(|principal| principal.admitted_identity.clone());

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
                && let Some(target) =
                    route.targets.iter().find(|target| target.is_cross_relay())
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
        |route| {
            prepare_send(
                route,
                &authorization,
                configuration_roots,
                bundle_catalog,
            )
        },
        |route, groups| {
            // Final admission gate: park for tests, then hold the
            // admission guard and the store lock across revalidation,
            // commit, and local queue/registry publication. Reservation
            // and publication are one atomic commit against shutdown: a
            // shutdown that published first makes the recheck refuse, and
            // a shutdown that arrives after finds published queue entries
            // and drains them. Only cross-relay forwarding runs outside
            // the span, since peer transport execution must never sit
            // under the shutdown gate or the store lock.
            test_hooks::test_send_admit_gate(
                format!("{}@{home_namespace}", sender.session_id).as_str(),
            );
            let execution = SendExecutionContext {
                delivery,
                sender: &sender,
                authenticated_identity,
                admitted_identity,
                on_behalf_of,
                sender_return_route,
                message: message.as_str(),
                home_namespace,
                request_id,
                peer_connection_manager,
            };
            let local_results = {
                let _admission = delivery.admission_span();
                let store_lock = lock_store_transaction(state_root);
                let _store = store_lock
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if delivery.is_shutting_down() {
                    return Err(relay_error(
                        "runtime_shutting_down",
                        "relay handle is shutting down",
                        None,
                    ));
                }
                revalidate_sender_binding(state_root, principal)?;
                test_hooks::test_send_admit_commit_gate(
                    format!("{}@{home_namespace}", sender.session_id).as_str(),
                );
                if !relay_ingress {
                    // Current grants under the same span: reload the home
                    // bundle and authorization, re-resolve the sender
                    // against the fresh membership, and re-authorize the
                    // whole route, so a narrowing or membership removal
                    // committed while parked still denies.
                    // Relay-ingress keeps its live binding, which the
                    // held admin lock keeps current by construction.
                    let (fresh_bundle, fresh_authorization) =
                        load_home_context(
                            home_namespace,
                            configuration_roots,
                        )?;
                    resolve_sender_in_namespace(
                        fresh_bundle.as_ref(),
                        &fresh_authorization,
                        sender.session_id.as_str(),
                        "requester_session",
                    )?;
                    RouteAuthorization::Policy(&fresh_authorization)
                        .authorize(
                            home_namespace,
                            OperationProfile {
                                capability: Capability::Send,
                                addressing: Addressing::MultiTarget,
                            },
                            route,
                        )?;
                }
                let admitted = admit_request_targets(
                    delivery,
                    &groups,
                    message.as_str(),
                )?;
                test_hooks::test_send_admit_reserved_gate(
                    format!("{}@{home_namespace}", sender.session_id).as_str(),
                );
                publish_local_deliveries(route, &groups, admitted, &execution)?
            };
            execute_send(route, local_results, execution)
        },
    )
}
