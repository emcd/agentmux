use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::json;
use uuid::Uuid;

use crate::{
    configuration::{BundleConfiguration, TargetConfiguration, load_bundle_configuration},
    runtime::inscriptions::emit_inscription,
};

use super::super::authorization::{
    AuthorizationContext, choices_max_pending, choose_authorized_ui_sessions, has_ui_session,
    load_authorization_context,
};
use super::super::connection::BundleCatalog;
use super::super::delivery::{QuiescenceOptions, enqueue_async_delivery, prompt_batch_settings};
use super::super::routing::{
    Addressing, Capability, OperationProfile, ResolvedRoute, ResolvedTarget as RouteTarget,
    resolve_send_route,
};
use super::super::{
    AsyncDeliveryTask, DeliveryPayloadMode, GLOBAL_NAMESPACE, RelayError, RelayRequest,
    RelayResponse, RequestPrincipal, SCHEMA_VERSION, SendOutcome, SendRequestContext, SendResult,
    bare_session_id, canonical_session_id, map_config, relay_error,
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
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
    principal: Option<&RequestPrincipal>,
) -> Result<RelayResponse, RelayError> {
    let RelayRequest::Send {
        request_id,
        requester_session,
        message,
        targets,
        broadcast,
        quiet_window_ms,
        quiescence_timeout_ms,
        acp_turn_timeout_ms,
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
            quiescence_timeout_ms,
            acp_turn_timeout_ms,
        },
        configuration_root,
        bundle_catalog,
        principal,
    )
}

fn handle_send(
    home_namespace: &str,
    request: SendRequestContext,
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
    principal: Option<&RequestPrincipal>,
) -> Result<RelayResponse, RelayError> {
    // The sender is identified by its home namespace alone: a `GLOBAL` sender is
    // relay-wide and has no bundle, any other namespace names the home bundle. The
    // home authorization context (operator policy for `GLOBAL`, or the bundle's
    // policy) is derived from it.
    let (home_bundle, authorization) = load_home_context(home_namespace, configuration_root)?;
    let SendRequestContext {
        request_id,
        requester_session,
        message,
        targets,
        broadcast,
        quiet_window_ms,
        quiescence_timeout_ms,
        acp_turn_timeout_ms,
    } = request;

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
    if matches!(quiescence_timeout_ms, Some(0)) {
        return Err(relay_error(
            "validation_invalid_quiescence_timeout",
            "quiescence timeout override must be greater than zero milliseconds",
            None,
        ));
    }
    if matches!(acp_turn_timeout_ms, Some(0)) {
        return Err(relay_error(
            "validation_invalid_acp_turn_timeout",
            "ACP turn timeout override must be greater than zero milliseconds",
            None,
        ));
    }
    if quiescence_timeout_ms.is_some() && acp_turn_timeout_ms.is_some() {
        return Err(relay_error(
            "validation_conflicting_timeout_fields",
            "quiescence_timeout_ms and acp_turn_timeout_ms are mutually exclusive",
            None,
        ));
    }

    // The requester is authorized in its home namespace (its bundle, or
    // `GLOBAL`); strip any `@<home>` qualifier so internal lookups match. A
    // relay-wide (`@GLOBAL`) requester keeps its suffix.
    let requester_session = bare_session_id(requester_session.as_str(), home_namespace);
    let sender = resolve_sender_in_namespace(
        home_bundle.as_ref(),
        &authorization,
        requester_session.as_str(),
        "requester_session",
    )?;
    // Verified principal_id of the sender, carried both on the Send response
    // and into each recipient's delivered envelope; `None` for socket-trust.
    let authenticated_identity =
        principal.and_then(|principal| principal.authenticated_identity.clone());

    emit_inscription(
        "relay.send.request",
        &json!({
            "bundle_name": home_namespace,
            "requester_session": sender.session_id,
            "broadcast": broadcast,
            "target_count": targets.len(),
            "message_length": message.len(),
            "request_id": request_id.clone(),
        }),
    );

    // The spine owns resolution and authorization; `resolve_send_route_or_broadcast`
    // builds the config-free route, `prepare_send` assembles the per-namespace
    // delivery groups (validating target existence and transport-vs-timeout), and
    // `execute_send` enqueues delivery.
    run_target_operation(
        home_namespace,
        &authorization,
        OperationProfile {
            capability: Capability::Send,
            addressing: Addressing::MultiTarget,
        },
        || {
            resolve_send_route_or_broadcast(
                broadcast,
                home_namespace,
                &sender,
                home_bundle.as_ref(),
                &targets,
            )
        },
        |route| {
            prepare_send(
                route,
                &authorization,
                configuration_root,
                bundle_catalog,
                quiescence_timeout_ms,
                acp_turn_timeout_ms,
            )
        },
        |route, groups| {
            execute_send(
                route,
                groups,
                &sender,
                authenticated_identity,
                message.as_str(),
                home_namespace,
                request_id,
                quiet_window_ms,
                quiescence_timeout_ms,
                acp_turn_timeout_ms,
            )
        },
    )
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
            bundle_name: home_namespace.to_string(),
            session_id: Some(member.id.clone()),
            relay_wide: false,
        })
        .collect();
    Ok(ResolvedRoute {
        dispatch_namespace: home_namespace.to_string(),
        requester_session: sender.session_id.clone(),
        targets: route_targets,
    })
}

/// Assembles the per-namespace delivery groups for a `Send` and validates that
/// the requested timeout override is compatible with every resolved target's
/// transport. Target existence is folded into `assemble_delivery_groups`; a
/// broadcast route's targets are home-bundle members and resolve through the
/// catalog like any other bundle-bound target. Runs as the spine's `prepare`
/// stage, before authorization.
fn prepare_send(
    route: &ResolvedRoute,
    authorization: &AuthorizationContext,
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
    quiescence_timeout_ms: Option<u64>,
    acp_turn_timeout_ms: Option<u64>,
) -> Result<Vec<DeliveryGroup>, RelayError> {
    let groups = assemble_delivery_groups(
        authorization,
        configuration_root,
        bundle_catalog,
        &route.targets,
    )?;

    // Timeout-vs-transport validation spans every resolved target, regardless of
    // which bundle hosts it. Relay-wide targets carry no tmux/ACP
    // transport and are skipped.
    let mut has_tmux_target = false;
    let mut has_acp_target = false;
    for group in &groups {
        for target in &group.targets {
            if target.relay_wide {
                continue;
            }
            match group
                .bundle
                .members
                .iter()
                .find(|member| member.id == target.session_id)
                .map(|member| &member.target)
            {
                Some(TargetConfiguration::Tmux(_)) => has_tmux_target = true,
                Some(TargetConfiguration::Acp(_)) => has_acp_target = true,
                Some(TargetConfiguration::Ui | TargetConfiguration::Pubsub) => {}
                None => {
                    return Err(relay_error(
                        "internal_unexpected_failure",
                        "resolved target session has no configured transport",
                        Some(json!({ "target_session": target.session_id })),
                    ));
                }
            }
        }
    }

    if quiescence_timeout_ms.is_some() && has_acp_target {
        return Err(relay_error(
            "validation_invalid_timeout_field_for_transport",
            "quiescence_timeout_ms is not valid for ACP targets",
            Some(json!({
                "field": "quiescence_timeout_ms",
                "transport": "acp",
            })),
        ));
    }
    if acp_turn_timeout_ms.is_some() && has_tmux_target {
        return Err(relay_error(
            "validation_invalid_timeout_field_for_transport",
            "acp_turn_timeout_ms is not valid for tmux targets",
            Some(json!({
                "field": "acp_turn_timeout_ms",
                "transport": "tmux",
            })),
        ));
    }
    Ok(groups)
}

/// Enqueues async delivery to every authorized target and builds the `Send`
/// response. Runs as the spine's `execute` stage, after authorization.
#[allow(clippy::too_many_arguments)]
fn execute_send(
    route: &ResolvedRoute,
    groups: Vec<DeliveryGroup>,
    sender: &SenderIdentity,
    authenticated_identity: Option<String>,
    message: &str,
    home_namespace: &str,
    request_id: Option<String>,
    quiet_window_ms: Option<u64>,
    quiescence_timeout_ms: Option<u64>,
    acp_turn_timeout_ms: Option<u64>,
) -> Result<RelayResponse, RelayError> {
    let sender_member = sender.to_bundle_member();
    let batch_settings = prompt_batch_settings();
    let quiescence =
        QuiescenceOptions::for_async(quiet_window_ms, quiescence_timeout_ms, acp_turn_timeout_ms);
    let mut results = Vec::with_capacity(route.targets.len());
    // Every task carries the full recipient list across all delivery groups so
    // delivered envelopes can show co-recipients in other namespaces. Entries
    // are canonical `session@namespace` ids: bare ids are ambiguous outside
    // their own group.
    let all_recipient_sessions = groups
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
    for group in &groups {
        for target in &group.targets {
            let message_id = Uuid::new_v4().to_string();
            let task = AsyncDeliveryTask {
                bundle: group.bundle.clone(),
                sender_bundle_name: home_namespace.to_string(),
                sender: sender_member.clone(),
                authenticated_identity: authenticated_identity.clone(),
                all_target_sessions: all_recipient_sessions.clone(),
                target_session: target.session_id.clone(),
                relay_wide_target: target.relay_wide,
                message: message.to_string(),
                message_id: message_id.clone(),
                quiescence,
                batch_settings,
                runtime_directory: group.runtime_directory.clone(),
                completion_sender: None,
                payload_mode: DeliveryPayloadMode::EnvelopeMessage,
                append_enter: true,
                choice_decider_sessions: group.choice_decider_sessions.clone(),
                choices_max_pending: group.choices_max_pending,
            };
            enqueue_async_delivery(task)?;
            emit_inscription(
                "relay.send.async.queued",
                &json!({
                    "bundle_name": group.bundle.bundle_name,
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
    let response = RelayResponse::Send {
        schema_version: SCHEMA_VERSION.to_string(),
        request_id,
        requester_session: canonical_session_id(sender.session_id.as_str(), home_namespace),
        sender_display_name: sender.display_name.clone(),
        authenticated_identity: authenticated_identity.clone(),
        on_behalf_of: None,
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
            "bundle_name": home_namespace,
            "requester_session": requester_session,
            "result_count": results.len(),
            "delivered_count": delivered_count,
            }),
        );
    }
    Ok(response)
}

/// One namespace-scoped delivery group: a target bundle's configuration plus
/// the runtime context and choice deciders used to dispatch its targets.
struct DeliveryGroup {
    bundle: BundleConfiguration,
    runtime_directory: PathBuf,
    choice_decider_sessions: Vec<String>,
    choices_max_pending: usize,
    targets: Vec<ResolvedTarget>,
}

/// One validated target within a delivery group. `relay_wide` marks relay-wide
/// (`@GLOBAL`) targets, whose registry key is re-derived from the suffix rather
/// than from the group's bundle members.
struct ResolvedTarget {
    session_id: String,
    relay_wide: bool,
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
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
    route_targets: &[RouteTarget],
) -> Result<Vec<DeliveryGroup>, RelayError> {
    let mut group_order: Vec<String> = Vec::new();
    let mut groups_by_bundle: HashMap<String, DeliveryGroup> = HashMap::new();
    let mut unknown_targets: Vec<String> = Vec::new();

    for target in route_targets {
        let session_id = target.session_id.as_deref().unwrap_or_default();
        if target.relay_wide {
            // Relay-wide `@GLOBAL` target: existence is a registered UI session,
            // resolved from the sender's (operator) authorization context.
            if has_ui_session(home_authorization, session_id) {
                let group_key = ensure_relay_wide_group(
                    home_authorization,
                    &mut group_order,
                    &mut groups_by_bundle,
                );
                push_target(
                    &mut groups_by_bundle,
                    group_key.as_str(),
                    ResolvedTarget {
                        session_id: session_id.to_string(),
                        relay_wide: true,
                    },
                );
            } else {
                unknown_targets.push(session_id.to_string());
            }
            continue;
        }
        let bundle_name = target.bundle_name.as_str();
        match ensure_bundle_group(
            bundle_name,
            configuration_root,
            bundle_catalog,
            &mut group_order,
            &mut groups_by_bundle,
        ) {
            Ok(()) => {
                let is_member = groups_by_bundle.get(bundle_name).is_some_and(|group| {
                    group
                        .bundle
                        .members
                        .iter()
                        .any(|member| member.id == session_id)
                });
                if is_member {
                    push_target(
                        &mut groups_by_bundle,
                        bundle_name,
                        ResolvedTarget {
                            session_id: session_id.to_string(),
                            relay_wide: false,
                        },
                    );
                } else {
                    unknown_targets.push(canonical_session_id(session_id, bundle_name));
                }
            }
            Err(BundleGroupError::UnknownBundle) => {
                unknown_targets.push(canonical_session_id(session_id, bundle_name));
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
        .filter_map(|bundle_name| groups_by_bundle.remove(&bundle_name))
        .filter(|group| !group.targets.is_empty())
        .collect())
}

/// Returns the delivery-group key for a relay-wide (`@GLOBAL`) target, seeding
/// the group when absent. Every sender's `@GLOBAL` targets land in the same
/// synthetic `GLOBAL` group whose bundle/runtime are inert — UI delivery routes
/// by the target's principal id through the registry. Only the sender's queue
/// bound (from its home authorization) carries over.
fn ensure_relay_wide_group(
    home_authorization: &AuthorizationContext,
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
                choices_max_pending: choices_max_pending(home_authorization),
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
    bundle_name: &str,
    target: ResolvedTarget,
) {
    if let Some(group) = groups_by_bundle.get_mut(bundle_name) {
        group.targets.push(target);
    }
}

/// Ensures a delivery group exists for `bundle_name`, loading the bundle's
/// configuration and authorization from the catalog when first seen. The home
/// group (when the sender is bundle-bound) and already-seen peers are seeded, so
/// they short-circuit; an unconfigured bundle is reported so the caller can fold
/// it into `validation_unknown_target`.
fn ensure_bundle_group(
    bundle_name: &str,
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
    group_order: &mut Vec<String>,
    groups_by_bundle: &mut HashMap<String, DeliveryGroup>,
) -> Result<(), BundleGroupError> {
    if groups_by_bundle.contains_key(bundle_name) {
        return Ok(());
    }
    let Some(paths) = bundle_catalog.lookup(bundle_name) else {
        return Err(BundleGroupError::UnknownBundle);
    };
    let bundle = load_bundle_configuration(configuration_root, bundle_name)
        .map_err(|error| BundleGroupError::Relay(map_config(error)))?;
    let authorization = load_authorization_context(configuration_root, Some(&bundle))
        .map_err(BundleGroupError::Relay)?;
    let choice_decider_sessions = choose_authorized_ui_sessions(&authorization, &bundle);
    let choices_max_pending = choices_max_pending(&authorization);
    group_order.push(bundle_name.to_string());
    groups_by_bundle.insert(
        bundle_name.to_string(),
        DeliveryGroup {
            bundle,
            runtime_directory: paths.runtime_directory.clone(),
            choice_decider_sessions,
            choices_max_pending,
            targets: Vec::new(),
        },
    );
    Ok(())
}
