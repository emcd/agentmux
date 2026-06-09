use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::json;
use uuid::Uuid;

use crate::{
    configuration::{BundleConfiguration, TargetConfiguration, load_bundle_configuration},
    runtime::inscriptions::emit_inscription,
};

use super::super::authorization::{
    AuthorizationContext, authorize_route, grant_authorized_ui_sessions, has_ui_session,
    load_authorization_context, permission_max_pending, ui_session_display_name,
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
use super::sender::{SenderIdentity, resolve_sender_identity};

/// Entry point for the namespace-centric send path. Destructures a `Send`
/// request and authorizes/delivers it in the requester's home namespace (its
/// bundle, or `GLOBAL`), without borrowing a peer bundle. See `dispatch_send`.
pub(in crate::relay) fn handle_send_routed(
    home_namespace: &str,
    home_runtime_directory: Option<&Path>,
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
        home_runtime_directory,
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
    home_runtime_directory: Option<&Path>,
    request: SendRequestContext,
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
    principal: Option<&RequestPrincipal>,
) -> Result<RelayResponse, RelayError> {
    // The sender is identified by its home namespace alone. A `GLOBAL` sender is
    // relay-wide and has no bundle; any other namespace names the bundle whose
    // configuration backs sender resolution and home-group delivery. The home
    // authorization context (operator policy for `GLOBAL`, or the bundle's
    // policy) is derived from it, so neither the bundle nor its authorization is
    // threaded in as a parameter.
    let home_bundle = if home_namespace == GLOBAL_NAMESPACE {
        None
    } else {
        Some(load_bundle_configuration(configuration_root, home_namespace).map_err(map_config)?)
    };
    let authorization = load_authorization_context(configuration_root, home_bundle.as_ref())?;
    let authorization = &authorization;
    let home_bundle = home_bundle.as_ref();
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
    let sender = match home_bundle {
        Some(home_bundle) => resolve_sender_identity(
            home_bundle,
            authorization,
            requester_session.as_str(),
            "requester_session",
        )?,
        None => {
            // Relay-wide sender: no home bundle; it must be a registered UI
            // (`@GLOBAL`) session, resolved from the operator authorization.
            if has_ui_session(authorization, requester_session.as_str()) {
                SenderIdentity {
                    session_id: requester_session.clone(),
                    display_name: ui_session_display_name(
                        authorization,
                        requester_session.as_str(),
                    )
                    .map(ToString::to_string),
                }
            } else {
                return Err(relay_error(
                    "validation_unknown_sender",
                    "sender session is not configured",
                    Some(json!({
                        "field": "requester_session",
                        "value": requester_session,
                    })),
                ));
            }
        }
    };
    let sender_member = sender.to_bundle_member();
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

    // Build the config-free route (suffix classification, for authorization) and
    // the per-namespace delivery groups (catalog-loaded, for delivery). A single
    // Send fans out across the sender's home bundle, peer bundles, and the
    // relay-wide (`@GLOBAL`) registry. Broadcast stays home-bundle-scoped and
    // requires a bundle-bound sender.
    let (route, groups) = if broadcast {
        let Some(home_bundle) = home_bundle else {
            return Err(relay_error(
                "validation_invalid_arguments",
                "broadcast requires a bundle-bound sender",
                None,
            ));
        };
        let runtime_directory = home_runtime_directory.ok_or_else(|| {
            relay_error(
                "internal_unexpected_failure",
                "bundle-bound sender is missing its runtime directory",
                None,
            )
        })?;
        let broadcast_targets: Vec<ResolvedTarget> = home_bundle
            .members
            .iter()
            .filter(|member| member.id != sender.session_id)
            .map(|member| ResolvedTarget {
                session_id: member.id.clone(),
                is_ui: false,
            })
            .collect();
        let route = ResolvedRoute {
            dispatch_namespace: home_namespace.to_string(),
            requester_session: sender.session_id.clone(),
            targets: broadcast_targets
                .iter()
                .map(|target| RouteTarget {
                    bundle_name: home_namespace.to_string(),
                    session_id: Some(target.session_id.clone()),
                    relay_wide: false,
                })
                .collect(),
        };
        let group = DeliveryGroup {
            bundle: home_bundle.clone(),
            runtime_directory: runtime_directory.to_path_buf(),
            permission_decider_sessions: grant_authorized_ui_sessions(authorization, home_bundle),
            permission_max_pending: permission_max_pending(authorization),
            targets: broadcast_targets,
        };
        (route, vec![group])
    } else {
        let route = resolve_send_route(home_namespace, sender.session_id.as_str(), &targets)?;
        let groups = assemble_delivery_groups(
            home_namespace,
            home_bundle,
            home_runtime_directory,
            authorization,
            configuration_root,
            bundle_catalog,
            &route.targets,
        )?;
        (route, groups)
    };

    // Timeout-vs-transport validation spans every resolved target, regardless of
    // which bundle hosts it. Relay-wide (`is_ui`) targets carry no tmux/ACP
    // transport and are skipped.
    let mut has_tmux_target = false;
    let mut has_acp_target = false;
    for group in &groups {
        for target in &group.targets {
            if target.is_ui {
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
    // Send authorization runs through the uniform routing/authorization spine:
    // the sender's `send` control is resolved in its home namespace, and the
    // required scope tier is the maximum across every resolved target. A
    // cross-namespace (peer-bundle) target therefore demands `all:all`;
    // relay-wide (`@GLOBAL`) and same-namespace targets stay at the `all:home`
    // tier.
    authorize_route(
        home_namespace,
        authorization,
        OperationProfile {
            capability: Capability::Send,
            addressing: Addressing::MultiTarget,
        },
        &route,
    )?;

    let batch_settings = prompt_batch_settings();
    let quiescence =
        QuiescenceOptions::for_async(quiet_window_ms, quiescence_timeout_ms, acp_turn_timeout_ms);
    let mut results = Vec::with_capacity(route.targets.len());
    for group in &groups {
        let group_target_sessions = group
            .targets
            .iter()
            .map(|target| target.session_id.clone())
            .collect::<Vec<_>>();
        for target in &group.targets {
            let message_id = Uuid::new_v4().to_string();
            let task = AsyncDeliveryTask {
                bundle: group.bundle.clone(),
                sender_bundle_name: home_namespace.to_string(),
                sender: sender_member.clone(),
                authenticated_identity: authenticated_identity.clone(),
                all_target_sessions: group_target_sessions.clone(),
                target_session: target.session_id.clone(),
                target_is_ui: target.is_ui,
                message: message.clone(),
                message_id: message_id.clone(),
                quiescence,
                batch_settings,
                runtime_directory: group.runtime_directory.clone(),
                completion_sender: None,
                payload_mode: DeliveryPayloadMode::EnvelopeMessage,
                append_enter: true,
                permission_decider_sessions: group.permission_decider_sessions.clone(),
                permission_max_pending: group.permission_max_pending,
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
        bundle_name: home_namespace.to_string(),
        request_id,
        requester_session: canonical_session_id(sender.session_id.as_str(), home_namespace),
        sender_display_name: sender.display_name.clone(),
        authenticated_identity: authenticated_identity.clone(),
        on_behalf_of: None,
        results,
    };
    if let RelayResponse::Send {
        bundle_name,
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
            "bundle_name": bundle_name,
            "requester_session": requester_session,
            "result_count": results.len(),
            "delivered_count": delivered_count,
            }),
        );
    }
    Ok(response)
}

/// One namespace-scoped delivery group: a target bundle's configuration plus
/// the runtime context and permission deciders used to dispatch its targets.
struct DeliveryGroup {
    bundle: BundleConfiguration,
    runtime_directory: PathBuf,
    permission_decider_sessions: Vec<String>,
    permission_max_pending: usize,
    targets: Vec<ResolvedTarget>,
}

/// One validated target within a delivery group. `is_ui` marks relay-wide
/// (`@GLOBAL`) targets, whose registry key is re-derived from the suffix rather
/// than from the group's bundle members.
struct ResolvedTarget {
    session_id: String,
    is_ui: bool,
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
/// targets into a single `validation_unknown_target`. Each target bundle is
/// loaded from the catalog; relay-wide (`@GLOBAL`) targets are delivered via the
/// registry and ride the sender's home delivery context (or a synthetic `GLOBAL`
/// context when the sender itself is relay-wide and has no home bundle).
fn assemble_delivery_groups(
    home_namespace: &str,
    home_bundle: Option<&BundleConfiguration>,
    home_runtime_directory: Option<&Path>,
    home_authorization: &AuthorizationContext,
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
    route_targets: &[RouteTarget],
) -> Result<Vec<DeliveryGroup>, RelayError> {
    let mut group_order: Vec<String> = Vec::new();
    let mut groups_by_bundle: HashMap<String, DeliveryGroup> = HashMap::new();
    let mut unknown_targets: Vec<String> = Vec::new();

    // Seed the home group from the sender's home bundle when bundle-bound. It
    // hosts same-namespace targets and relay-wide (`@GLOBAL`) targets, attributed
    // to the sender's home namespace. A relay-wide sender has no home bundle; its
    // `@GLOBAL` targets land in a synthetic `GLOBAL` group seeded on demand.
    if let Some(home_bundle) = home_bundle {
        group_order.push(home_namespace.to_string());
        groups_by_bundle.insert(
            home_namespace.to_string(),
            DeliveryGroup {
                bundle: home_bundle.clone(),
                runtime_directory: home_runtime_directory
                    .map(Path::to_path_buf)
                    .unwrap_or_default(),
                permission_decider_sessions: grant_authorized_ui_sessions(
                    home_authorization,
                    home_bundle,
                ),
                permission_max_pending: permission_max_pending(home_authorization),
                targets: Vec::new(),
            },
        );
    }

    for target in route_targets {
        let session_id = target.session_id.as_deref().unwrap_or_default();
        if target.relay_wide {
            // Relay-wide `@GLOBAL` target: existence is a registered UI session,
            // resolved from the sender's (operator) authorization context.
            if has_ui_session(home_authorization, session_id) {
                let group_key = ensure_relay_wide_group(
                    home_namespace,
                    home_bundle.is_some(),
                    home_authorization,
                    &mut group_order,
                    &mut groups_by_bundle,
                );
                push_target(
                    &mut groups_by_bundle,
                    group_key.as_str(),
                    ResolvedTarget {
                        session_id: session_id.to_string(),
                        is_ui: true,
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
                            is_ui: false,
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
/// the group when absent. A bundle-bound sender attributes `@GLOBAL` targets to
/// its home group (already seeded). A relay-wide sender (no home bundle) gets a
/// synthetic `GLOBAL` group whose bundle/runtime are inert — UI delivery routes
/// by the target's principal id through the registry.
fn ensure_relay_wide_group(
    home_namespace: &str,
    has_home_bundle: bool,
    home_authorization: &AuthorizationContext,
    group_order: &mut Vec<String>,
    groups_by_bundle: &mut HashMap<String, DeliveryGroup>,
) -> String {
    if has_home_bundle {
        return home_namespace.to_string();
    }
    let key = home_namespace.to_string();
    if !groups_by_bundle.contains_key(key.as_str()) {
        group_order.push(key.clone());
        groups_by_bundle.insert(
            key.clone(),
            DeliveryGroup {
                bundle: BundleConfiguration {
                    schema_version: SCHEMA_VERSION.to_string(),
                    bundle_name: home_namespace.to_string(),
                    autostart: false,
                    groups: Vec::new(),
                    members: Vec::new(),
                },
                runtime_directory: PathBuf::new(),
                permission_decider_sessions: Vec::new(),
                permission_max_pending: permission_max_pending(home_authorization),
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
    let permission_decider_sessions = grant_authorized_ui_sessions(&authorization, &bundle);
    let permission_max_pending = permission_max_pending(&authorization);
    group_order.push(bundle_name.to_string());
    groups_by_bundle.insert(
        bundle_name.to_string(),
        DeliveryGroup {
            bundle,
            runtime_directory: paths.runtime_directory.clone(),
            permission_decider_sessions,
            permission_max_pending,
            targets: Vec::new(),
        },
    );
    Ok(())
}
