use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::json;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::{
    configuration::{
        BundleConfiguration, BundleMember, TargetConfiguration, TmuxTargetConfiguration,
        load_bundle_configuration,
    },
    relay::{AcpLookFreshness, AcpLookSnapshotSource, LookSnapshotPayload},
    runtime::{inscriptions::emit_inscription, paths::tmux_socket_path_for_runtime_directory},
};

use super::authorization::{
    AuthorizationContext, authorize_route, authorize_updown, grant_authorized_ui_sessions,
    has_ui_session, load_authorization_context, permission_max_pending, ui_session_display_name,
};
use super::connection::BundleCatalog;
use super::delivery::{
    QuiescenceOptions, await_acp_worker_prime_for_look, deliver_one_target,
    derive_acp_look_snapshot, enqueue_async_delivery, enqueue_sync_delivery,
    get_acp_worker_snapshot, get_acp_worker_state, prompt_batch_settings,
};
use super::identity::{
    IdentityIntrospectRights, PrincipalType, classify_principal_id, split_principal_id,
};
use super::routing::{
    Addressing, Capability, OperationProfile, ResolvedRoute, ResolvedTarget as RouteTarget,
    requester_home_namespace,
};
use super::stream::{RelayStreamEvent, list_registered_relay_wide_sessions};
use super::tmux::{capture_pane_tail_lines, resolve_active_pane_target};
use super::{
    AsyncDeliveryTask, DeliveryPayloadMode, ListedBundle, ListedBundleState, ListedSession,
    ListedSessionTransport, LookRequestContext, PermissionDecisionRequestContext,
    RawwRequestContext, RelayError, RelayRequest, RelayResponse, RequestPrincipal, SCHEMA_VERSION,
    SendOutcome, SendRequestContext, SendResult, bare_session_id, canonical_session_id, map_config,
    relay_error,
};

mod identity;
mod listing;
mod permissions;

pub(in crate::relay) use listing::handle_list_routed;

const LOOK_LINES_DEFAULT: usize = 120;
const LOOK_LINES_MAX: usize = 1000;
/// Default ACP look window. ACP replay entries are far larger than tmux lines
/// (each can be a full message or tool invocation), so a small default keeps
/// the response under the MCP payload limit while still showing recent context.
const ACP_LOOK_ENTRIES_DEFAULT: usize = 50;

#[derive(Clone, Debug)]
struct SenderIdentity {
    session_id: String,
    display_name: Option<String>,
}

impl SenderIdentity {
    fn from_bundle_member(member: &BundleMember) -> Self {
        Self {
            session_id: member.id.clone(),
            display_name: member.name.clone(),
        }
    }

    fn to_bundle_member(&self) -> BundleMember {
        BundleMember {
            id: self.session_id.clone(),
            name: self.display_name.clone(),
            working_directory: None,
            target: TargetConfiguration::Tmux(TmuxTargetConfiguration {
                start_command: "ui-session".to_string(),
                prompt_readiness: None,
            }),
            coder_session_id: None,
            policy_id: None,
        }
    }
}

pub(super) fn handle_request(
    request: RelayRequest,
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    runtime_directory: &Path,
    principal: Option<RequestPrincipal>,
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
) -> Result<RelayResponse, RelayError> {
    let request = normalize_request_identities(request, bundle.bundle_name.as_str());
    match request {
        RelayRequest::Up => {
            authorize_bundle_principal(bundle, authorization, principal.as_ref())?;
            listing::handle_bundle_up(bundle, runtime_directory)
        }
        RelayRequest::Down => {
            authorize_bundle_principal(bundle, authorization, principal.as_ref())?;
            listing::handle_bundle_down(bundle, runtime_directory)
        }
        RelayRequest::List { requester_session } => listing::handle_list_routed(
            bundle,
            authorization,
            bundle,
            runtime_directory,
            requester_session,
        ),
        RelayRequest::Send {
            request_id,
            requester_session,
            message,
            targets,
            broadcast,
            quiet_window_ms,
            quiescence_timeout_ms,
            acp_turn_timeout_ms,
        } => handle_send(
            bundle,
            authorization,
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
            runtime_directory,
            configuration_root,
            bundle_catalog,
            principal.as_ref(),
        ),
        RelayRequest::Look {
            requester_session,
            target_session,
            lines,
            offset,
        } => handle_look(
            bundle,
            authorization,
            LookRequestContext {
                requester_session,
                target_session,
                lines,
                offset,
            },
            runtime_directory,
            configuration_root,
            bundle_catalog,
            principal.as_ref(),
        ),
        RelayRequest::Raww {
            request_id,
            requester_session,
            target_session,
            text,
            no_enter,
        } => handle_raww(
            bundle,
            authorization,
            RawwRequestContext {
                request_id,
                requester_session,
                target_session,
                text,
                no_enter,
            },
            runtime_directory,
        ),
        RelayRequest::PermissionResolve {
            permission_request_id,
            outcome,
            option_id,
            bundle_name: request_bundle_name,
            ui_session_id,
        } => handle_permission_decision(
            bundle,
            authorization,
            PermissionDecisionRequestContext {
                permission_request_id,
                outcome,
                option_id,
                bundle_name: request_bundle_name,
                ui_session_id,
            },
            runtime_directory,
            principal,
        ),
        RelayRequest::PermissionList {
            bundle_name: request_bundle_name,
        } => permissions::handle_permission_list(
            bundle,
            authorization,
            request_bundle_name,
            runtime_directory,
            principal,
        ),
        RelayRequest::NewPeer { .. }
        | RelayRequest::ChangePsk { .. }
        | RelayRequest::IdentityIntrospect { .. } => Err(relay_error(
            "internal_unexpected_request",
            "relay-wide identity request reached the per-bundle dispatcher",
            None,
        )),
    }
}

/// Returns the set of currently registered relay-wide sessions for a `List`
/// request issued with `namespace = "GLOBAL"`.
///
/// Relay-wide routing has no bundle context, so this bypasses the per-bundle
/// `handle_request` path entirely: it reads the live stream registry and
/// projects each `RegistryKey::RelayWide` entry into a `ListedSession`. The
/// result is shaped as a `RelayResponse::List` over a synthetic `GLOBAL` bundle
/// view so clients reuse the existing list payload. An empty registry yields an
/// empty session set (a `down` bundle), not an error.
pub(super) fn handle_global_list() -> RelayResponse {
    let mut sessions = list_registered_relay_wide_sessions()
        .into_iter()
        .map(|(principal_id, session_type)| ListedSession {
            id: principal_id,
            name: None,
            transport: session_type.into(),
            ready: true,
        })
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| left.id.cmp(&right.id));
    let hosted = !sessions.is_empty();
    let state = if hosted {
        ListedBundleState::Up
    } else {
        ListedBundleState::Down
    };
    RelayResponse::List {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle: ListedBundle {
            id: "GLOBAL".to_string(),
            hosted,
            state,
            startup_health: None,
            state_reason_code: None,
            state_reason: None,
            startup_failure_count: 0,
            recent_startup_failures: Vec::new(),
            principals: sessions,
        },
    }
}

/// Dispatches a relay-wide identity administration request (`new peer`,
/// `change psk`). These bypass the per-bundle `handle_request` path: they
/// operate on the relay-level principal store and authorize against the
/// requester's policy preset relay-wide rather than within a bundle context.
pub(super) fn handle_identity_admin_request(
    request: RelayRequest,
    configuration_root: &Path,
    state_root: &Path,
    requester_principal_id: &str,
) -> Result<RelayResponse, RelayError> {
    match request {
        RelayRequest::NewPeer {
            principal_id,
            scope,
            output_path,
        } => identity::handle_new_peer(
            configuration_root,
            state_root,
            requester_principal_id,
            identity::NewPeerRequestContext {
                principal_id,
                scope,
                output_path,
            },
        ),
        RelayRequest::ChangePsk { principal_id } => identity::handle_change_psk(
            configuration_root,
            state_root,
            requester_principal_id,
            principal_id,
        ),
        _ => Err(relay_error(
            "internal_unexpected_request",
            "non-admin request routed to identity admin dispatcher",
            None,
        )),
    }
}

/// Dispatches a relay-wide `IdentityIntrospect` request. Like the identity admin
/// handlers this bypasses the per-bundle `handle_request` path: introspection
/// reads the relay-level principal store and its target may be a bundle-less
/// principal. The connection gate (the recorded `introspect_rights` and their
/// scope) is enforced inside the handler.
pub(super) fn handle_identity_introspect(
    state_root: &Path,
    principal: &RequestPrincipal,
    target_session: &str,
    bundle_name: Option<&str>,
) -> Result<RelayResponse, RelayError> {
    identity::handle_identity_introspect(state_root, principal, target_session, bundle_name)
}

/// Builds the `identity.snapshot` stream event for a trusted-host connection's
/// registered scope (delivered once, right after Hello). See
/// `identity::build_identity_snapshot_event`.
pub(super) fn build_identity_snapshot_event(
    state_root: &Path,
    host_principal_id: &str,
    rights: &IdentityIntrospectRights,
) -> Result<RelayStreamEvent, RelayError> {
    identity::build_identity_snapshot_event(state_root, host_principal_id, rights)
}

/// Rewrites canonical `session@bundle` identities in an incoming request to
/// their bundle-local form so internal lookups match configured member ids.
fn normalize_request_identities(request: RelayRequest, bundle_name: &str) -> RelayRequest {
    let bare = |id: String| bare_session_id(id.as_str(), bundle_name);
    match request {
        RelayRequest::List { requester_session } => RelayRequest::List {
            requester_session: requester_session.map(bare),
        },
        RelayRequest::Send {
            request_id,
            requester_session,
            message,
            targets,
            broadcast,
            quiet_window_ms,
            quiescence_timeout_ms,
            acp_turn_timeout_ms,
        } => RelayRequest::Send {
            request_id,
            requester_session: bare(requester_session),
            message,
            targets: targets.into_iter().map(bare).collect(),
            broadcast,
            quiet_window_ms,
            quiescence_timeout_ms,
            acp_turn_timeout_ms,
        },
        RelayRequest::Look {
            requester_session,
            target_session,
            lines,
            offset,
        } => RelayRequest::Look {
            requester_session: bare(requester_session),
            target_session: bare(target_session),
            lines,
            offset,
        },
        RelayRequest::Raww {
            request_id,
            requester_session,
            target_session,
            text,
            no_enter,
        } => RelayRequest::Raww {
            request_id,
            requester_session: bare(requester_session),
            target_session: bare(target_session),
            text,
            no_enter,
        },
        request @ (RelayRequest::Up
        | RelayRequest::Down
        | RelayRequest::PermissionResolve { .. }
        | RelayRequest::PermissionList { .. }
        | RelayRequest::NewPeer { .. }
        | RelayRequest::ChangePsk { .. }
        | RelayRequest::IdentityIntrospect { .. }) => request,
    }
}

fn handle_send(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    request: SendRequestContext,
    runtime_directory: &Path,
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
    principal: Option<&RequestPrincipal>,
) -> Result<RelayResponse, RelayError> {
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

    let sender = resolve_sender_identity(
        bundle,
        authorization,
        requester_session.as_str(),
        "requester_session",
    )?;
    let sender_member = sender.to_bundle_member();
    // Verified principal_id of the sender, carried both on the Send response
    // and into each recipient's delivered envelope; `None` for socket-trust.
    let authenticated_identity =
        principal.and_then(|principal| principal.authenticated_identity.clone());

    emit_inscription(
        "relay.send.request",
        &json!({
            "bundle_name": bundle.bundle_name,
            "requester_session": sender.session_id,
            "broadcast": broadcast,
            "target_count": targets.len(),
            "message_length": message.len(),
            "request_id": request_id.clone(),
        }),
    );

    // Resolve every target into a per-namespace delivery group up front, so a
    // single Send can fan out across the sender's bundle, peer bundles, and the
    // relay-wide (`@GLOBAL`) registry. Broadcast stays bundle-scoped.
    let groups = if broadcast {
        vec![DeliveryGroup {
            bundle: bundle.clone(),
            runtime_directory: runtime_directory.to_path_buf(),
            permission_decider_sessions: grant_authorized_ui_sessions(authorization, bundle),
            permission_max_pending: permission_max_pending(authorization),
            targets: bundle
                .members
                .iter()
                .filter(|member| member.id != sender.session_id)
                .map(|member| ResolvedTarget {
                    session_id: member.id.clone(),
                    is_ui: false,
                })
                .collect(),
        }]
    } else {
        resolve_target_groups(
            bundle,
            authorization,
            runtime_directory,
            configuration_root,
            bundle_catalog,
            &targets,
        )?
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
    // the sender's `send` control is resolved in its dispatch bundle, and the
    // required scope tier is the maximum across every resolved target. A
    // cross-bundle (peer) target therefore demands `all:all`; relay-wide
    // (`@GLOBAL`) and same-bundle targets stay at the `all:home` tier.
    let route = ResolvedRoute {
        dispatch_bundle_name: requester_home_namespace(
            sender.session_id.as_str(),
            bundle.bundle_name.as_str(),
        )
        .to_string(),
        requester_session: sender.session_id.clone(),
        targets: groups
            .iter()
            .flat_map(|group| {
                group.targets.iter().map(|target| RouteTarget {
                    bundle_name: group.bundle.bundle_name.clone(),
                    session_id: Some(target.session_id.clone()),
                    relay_wide: target.is_ui,
                })
            })
            .collect(),
    };
    authorize_route(
        bundle,
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
                sender_bundle_name: bundle.bundle_name.clone(),
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
        bundle_name: bundle.bundle_name.clone(),
        request_id,
        requester_session: canonical_session_id(
            sender.session_id.as_str(),
            bundle.bundle_name.as_str(),
        ),
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

fn handle_look(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    request: LookRequestContext,
    runtime_directory: &Path,
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
    principal: Option<&RequestPrincipal>,
) -> Result<RelayResponse, RelayError> {
    let LookRequestContext {
        requester_session,
        target_session,
        lines,
        offset,
    } = request;

    // Validate the caller-supplied window size against the shared bound; the
    // transport-specific default is applied per branch below, since ACP entries
    // and tmux lines have very different size profiles.
    if let Some(requested_lines) = lines
        && !(1..=LOOK_LINES_MAX).contains(&requested_lines)
    {
        return Err(relay_error(
            "validation_invalid_lines",
            "lines must be between 1 and 1000",
            Some(json!({
                "lines": requested_lines,
                "min": 1,
                "max": LOOK_LINES_MAX,
            })),
        ));
    }
    let offset = offset.unwrap_or(0);

    // The requester is always a member of the dispatch (connection) bundle; a
    // session is only ever a member of its home bundle.
    let requester = resolve_sender_identity(
        bundle,
        authorization,
        requester_session.as_str(),
        "requester_session",
    )?;

    // Resolve the target's hosting bundle from any `@<bundle>` suffix. A bare
    // target — or one qualified with the dispatch bundle, already stripped by
    // identity normalization — stays in the dispatch bundle.
    let (target_bundle_name, target_session_id) =
        resolve_look_target_bundle(bundle.bundle_name.as_str(), target_session.as_str())?;
    let cross_bundle = target_bundle_name != bundle.bundle_name;

    // For a peer-bundle target, load its configuration and runtime context from
    // the catalog; same-bundle looks reuse the dispatch bundle as-is.
    let peer_bundle;
    let (look_bundle, look_runtime_directory) = if cross_bundle {
        let Some(paths) = bundle_catalog.lookup(target_bundle_name) else {
            return Err(relay_error(
                "validation_unknown_bundle",
                "look target bundle is not configured on this relay",
                Some(json!({ "bundle_name": target_bundle_name })),
            ));
        };
        peer_bundle = load_bundle_configuration(configuration_root, target_bundle_name)
            .map_err(map_config)?;
        (&peer_bundle, paths.runtime_directory.clone())
    } else {
        (bundle, runtime_directory.to_path_buf())
    };

    let target = look_bundle
        .members
        .iter()
        .find(|member| member.id == target_session_id)
        .ok_or_else(|| {
            relay_error(
                "validation_unknown_target",
                "target_session is not in bundle configuration",
                Some(json!({
                    "target_session": canonical_session_id(target_session_id, target_bundle_name),
                })),
            )
        })?;

    // Authorize through the uniform routing/authorization spine: the requester's
    // `look` control is resolved in its dispatch bundle, and a peer-bundle target
    // raises the required tier to `all:all` while same-bundle inspection of
    // another session needs `all:home` (self-inspection needs only `self`).
    let route = ResolvedRoute {
        dispatch_bundle_name: requester_home_namespace(
            requester.session_id.as_str(),
            bundle.bundle_name.as_str(),
        )
        .to_string(),
        requester_session: requester.session_id.clone(),
        targets: vec![RouteTarget {
            bundle_name: target_bundle_name.to_string(),
            session_id: Some(target_session_id.to_string()),
            relay_wide: false,
        }],
    };
    authorize_route(
        bundle,
        authorization,
        OperationProfile {
            capability: Capability::Look,
            addressing: Addressing::SingleTarget,
        },
        &route,
    )?;

    let snapshot = match &target.target {
        crate::configuration::TargetConfiguration::Tmux(_) => {
            if offset != 0 {
                return Err(relay_error(
                    "validation_offset_unsupported",
                    "offset is only supported for ACP look targets",
                    Some(json!({
                        "target_session": target.id,
                        "offset": offset,
                    })),
                ));
            }
            let requested_lines = lines.unwrap_or(LOOK_LINES_DEFAULT);
            let tmux_socket =
                tmux_socket_path_for_runtime_directory(look_runtime_directory.as_path());
            let pane_target = resolve_active_pane_target(tmux_socket.as_path(), target.id.as_str())
                .map_err(|reason| {
                    relay_error(
                        "internal_unexpected_failure",
                        "failed to resolve active pane for look target",
                        Some(json!({"target_session": target.id, "cause": reason})),
                    )
                })?;
            let snapshot_lines = capture_pane_tail_lines(
                tmux_socket.as_path(),
                pane_target.as_str(),
                requested_lines,
            )
            .map_err(|reason| {
                relay_error(
                    "internal_unexpected_failure",
                    "failed to capture look snapshot",
                    Some(json!({"target_session": target.id, "cause": reason})),
                )
            })?;
            LookSnapshotPayload::Lines { snapshot_lines }
        }
        crate::configuration::TargetConfiguration::Ui
        | crate::configuration::TargetConfiguration::Pubsub => {
            return Err(super::session_type_not_implemented(
                target.id.as_str(),
                target.target.session_type(),
            ));
        }
        crate::configuration::TargetConfiguration::Acp(_) => {
            let prime_timed_out = await_acp_worker_prime_for_look(
                look_bundle,
                target,
                look_runtime_directory.as_path(),
            )
            .map_err(|reason| {
                relay_error(
                    "internal_unexpected_failure",
                    "failed to await ACP worker prime for look",
                    Some(json!({"target_session": target.id, "cause": reason})),
                )
            })?;
            let worker_state = get_acp_worker_state(
                look_bundle.bundle_name.as_str(),
                look_runtime_directory.as_path(),
                target.id.as_str(),
            );
            let worker_snapshot = get_acp_worker_snapshot(
                look_bundle.bundle_name.as_str(),
                look_runtime_directory.as_path(),
                target.id.as_str(),
            );
            let requested_entries = lines.unwrap_or(ACP_LOOK_ENTRIES_DEFAULT);
            let snapshot = derive_acp_look_snapshot(
                worker_state,
                worker_snapshot.as_deref().map(Vec::as_slice),
                requested_entries,
                offset,
                prime_timed_out,
            );
            LookSnapshotPayload::AcpEntriesV1 {
                snapshot_entries: snapshot.snapshot_entries,
                entries_total: snapshot.entries_total,
                returned_entries_count: snapshot.returned_entries_count,
                freshness: snapshot.freshness,
                snapshot_source: snapshot.snapshot_source,
                stale_reason_code: snapshot.stale_reason_code,
                snapshot_age_ms: snapshot.snapshot_age_ms,
            }
        }
    };
    let response = RelayResponse::Look {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle_name: look_bundle.bundle_name.clone(),
        requester_session: canonical_session_id(
            requester.session_id.as_str(),
            bundle.bundle_name.as_str(),
        ),
        target_session: canonical_session_id(target.id.as_str(), look_bundle.bundle_name.as_str()),
        captured_at: time::OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
        authenticated_identity: principal
            .and_then(|principal| principal.authenticated_identity.clone()),
        on_behalf_of: None,
        snapshot,
    };
    if let RelayResponse::Look {
        bundle_name,
        requester_session,
        target_session,
        snapshot,
        ..
    } = &response
    {
        let (
            snapshot_format,
            snapshot_count,
            entries_total,
            freshness_label,
            snapshot_source_label,
            stale_reason_code,
            snapshot_age_ms,
        ) = match snapshot {
            LookSnapshotPayload::Lines { snapshot_lines } => {
                ("lines", snapshot_lines.len(), None, None, None, None, None)
            }
            LookSnapshotPayload::AcpEntriesV1 {
                snapshot_entries,
                entries_total,
                freshness,
                snapshot_source,
                stale_reason_code,
                snapshot_age_ms,
                ..
            } => (
                "acp_entries_v1",
                snapshot_entries.len(),
                Some(*entries_total),
                Some(match freshness {
                    AcpLookFreshness::Fresh => "fresh",
                    AcpLookFreshness::Stale => "stale",
                }),
                Some(match snapshot_source {
                    AcpLookSnapshotSource::LiveBuffer => "live_buffer",
                    AcpLookSnapshotSource::None => "none",
                }),
                stale_reason_code.as_deref(),
                *snapshot_age_ms,
            ),
        };
        emit_inscription(
            "relay.look.response",
            &json!({
                "bundle_name": bundle_name,
                "requester_session": requester_session,
                "target_session": target_session,
                "snapshot_format": snapshot_format,
                "snapshot_count": snapshot_count,
                "entries_total": entries_total,
                "lines_requested": lines,
                "offset": offset,
                "freshness": freshness_label,
                "snapshot_source": snapshot_source_label,
                "stale_reason_code": stale_reason_code,
                "snapshot_age_ms": snapshot_age_ms,
            }),
        );
    }
    Ok(response)
}

fn handle_raww(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    request: RawwRequestContext,
    runtime_directory: &Path,
) -> Result<RelayResponse, RelayError> {
    let RawwRequestContext {
        request_id,
        requester_session,
        target_session,
        text,
        no_enter,
    } = request;

    if target_session.trim().is_empty() {
        return Err(relay_error(
            "validation_invalid_params",
            "target_session must be non-empty",
            Some(json!({
                "field": "target_session",
            })),
        ));
    }
    if text.len() > 32 * 1024 {
        return Err(relay_error(
            "validation_invalid_params",
            "raww text exceeds maximum size of 32 KiB",
            Some(json!({
                "field": "text",
                "max_bytes": 32 * 1024,
                "bytes": text.len(),
            })),
        ));
    }
    let sender = resolve_sender_identity(
        bundle,
        authorization,
        requester_session.as_str(),
        "requester_session",
    )?;
    let target_member = if let Some(member) = bundle
        .members
        .iter()
        .find(|member| member.id == target_session)
    {
        member
    } else if has_ui_session(authorization, target_session.as_str()) {
        return Err(relay_error(
            "validation_invalid_params",
            "raww target class is not supported",
            Some(json!({
                "target_session": target_session,
                "target_class": "ui",
                "supported_target_classes": ["tmux", "acp"],
            })),
        ));
    } else {
        return Err(relay_error(
            "validation_unknown_target",
            "target_session is not a canonical configured target identifier",
            Some(json!({
                "target_session": target_session,
            })),
        ));
    };
    // Raww authorizes through the uniform routing/authz spine like Send and Look:
    // the requester's `raww` control is resolved in its dispatch (home) bundle,
    // and the single target's relationship sets the required tier. A relay-wide
    // (`@GLOBAL`) requester reaching into a bundle is cross-namespace and so
    // requires `all:all`; a same-bundle target stays at `all:home`, and a
    // self-target floors at `self`.
    let route = ResolvedRoute {
        dispatch_bundle_name: requester_home_namespace(
            sender.session_id.as_str(),
            bundle.bundle_name.as_str(),
        )
        .to_string(),
        requester_session: sender.session_id.clone(),
        targets: vec![RouteTarget {
            bundle_name: bundle.bundle_name.clone(),
            session_id: Some(target_member.id.clone()),
            relay_wide: false,
        }],
    };
    authorize_route(
        bundle,
        authorization,
        OperationProfile {
            capability: Capability::Raww,
            addressing: Addressing::SingleTarget,
        },
        &route,
    )?;

    let transport = match &target_member.target {
        TargetConfiguration::Tmux(_) => ListedSessionTransport::Tmux,
        TargetConfiguration::Acp(_) => ListedSessionTransport::Acp,
        TargetConfiguration::Ui | TargetConfiguration::Pubsub => {
            return Err(super::session_type_not_implemented(
                target_member.id.as_str(),
                target_member.target.session_type(),
            ));
        }
    };
    let message_id = Uuid::new_v4().to_string();
    let sender_member = sender.to_bundle_member();
    let permission_decider_sessions = grant_authorized_ui_sessions(authorization, bundle);
    let queue_max_pending = permission_max_pending(authorization);
    let task = AsyncDeliveryTask {
        bundle: bundle.clone(),
        sender_bundle_name: bundle.bundle_name.clone(),
        sender: sender_member,
        // Raw input does not carry verified sender attribution, and its targets
        // are never UI streams.
        authenticated_identity: None,
        all_target_sessions: vec![target_member.id.clone()],
        target_session: target_member.id.clone(),
        target_is_ui: false,
        message: text,
        message_id: message_id.clone(),
        quiescence: QuiescenceOptions::for_sync(None, None, None),
        batch_settings: prompt_batch_settings(),
        runtime_directory: runtime_directory.to_path_buf(),
        completion_sender: None,
        payload_mode: DeliveryPayloadMode::RawInput,
        append_enter: !no_enter,
        permission_decider_sessions,
        permission_max_pending: queue_max_pending,
    };

    let result = match &target_member.target {
        TargetConfiguration::Acp(_) => enqueue_sync_delivery(task)?,
        TargetConfiguration::Tmux(_) => deliver_one_target(&task)?,
        TargetConfiguration::Ui | TargetConfiguration::Pubsub => {
            return Err(super::session_type_not_implemented(
                target_member.id.as_str(),
                target_member.target.session_type(),
            ));
        }
    };
    if result.outcome != SendOutcome::Delivered {
        let reason = result
            .reason
            .unwrap_or_else(|| "raww dispatch failed".to_string());
        let code = if matches!(
            result.reason_code.as_deref(),
            Some("runtime_acp_worker_unavailable")
        ) {
            "runtime_target_unavailable"
        } else {
            "runtime_transport_write_failed"
        };
        return Err(relay_error(
            code,
            "raww dispatch failed",
            Some(json!({
                "target_session": result.target_session,
                "transport": transport,
                "reason": reason,
                "reason_code": result.reason_code,
            })),
        ));
    }

    let details = if transport == ListedSessionTransport::Acp {
        Some(json!({
            "delivery_phase": "accepted_in_progress",
        }))
    } else {
        Some(json!({
            "delivery_phase": "accepted_dispatched",
        }))
    };
    Ok(RelayResponse::Raww {
        schema_version: SCHEMA_VERSION.to_string(),
        status: "accepted".to_string(),
        target_session: canonical_session_id(
            target_member.id.as_str(),
            bundle.bundle_name.as_str(),
        ),
        transport,
        request_id,
        message_id: Some(message_id),
        details,
    })
}

pub(super) fn emit_permission_snapshot_for_ui_registration(
    configuration_root: &Path,
    bundle_name: &str,
    runtime_directory: &Path,
    ui_session_id: &str,
) -> Result<(), RelayError> {
    permissions::emit_permission_snapshot_for_ui_registration(
        configuration_root,
        bundle_name,
        runtime_directory,
        ui_session_id,
    )
}

fn handle_permission_decision(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    request: PermissionDecisionRequestContext,
    runtime_directory: &Path,
    principal: Option<RequestPrincipal>,
) -> Result<RelayResponse, RelayError> {
    permissions::handle_permission_decision(
        bundle,
        authorization,
        request,
        runtime_directory,
        principal,
    )
}

fn authorize_bundle_principal(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    principal: Option<&RequestPrincipal>,
) -> Result<(), RelayError> {
    let principal = principal.ok_or_else(|| {
        relay_error(
            "validation_missing_hello",
            "bundle up/down requests require stream-associated principal identity",
            None,
        )
    })?;
    authorize_updown(bundle, authorization, principal.session_id.as_str())
}

fn resolve_sender_identity(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    sender_session: &str,
    detail_field: &str,
) -> Result<SenderIdentity, RelayError> {
    if let Some(member) = bundle
        .members
        .iter()
        .find(|member| member.id == sender_session)
    {
        return Ok(SenderIdentity::from_bundle_member(member));
    }
    if has_ui_session(authorization, sender_session) {
        return Ok(SenderIdentity {
            session_id: sender_session.to_string(),
            display_name: ui_session_display_name(authorization, sender_session)
                .map(ToString::to_string),
        });
    }
    Err(relay_error(
        "validation_unknown_sender",
        "sender session is not configured",
        Some(json!({
            "field": detail_field,
            "value": sender_session,
        })),
    ))
}

/// Resolves a look target's hosting bundle and bundle-local session id from its
/// principal-id form.
///
/// Mirrors Send's suffix-based routing: a target qualified with a peer bundle
/// (`<session>@<bundle>`) routes to that bundle, while a bare target — or one
/// qualified with the dispatch bundle, already stripped during identity
/// normalization — resolves within the dispatch bundle. Relay-wide namespaces
/// (`@GLOBAL`/`@EXTERNAL`/`@RELAY`) name no inspectable session and are rejected.
fn resolve_look_target_bundle<'a>(
    dispatch_bundle_name: &'a str,
    target_session: &'a str,
) -> Result<(&'a str, &'a str), RelayError> {
    match classify_principal_id(target_session) {
        Some(PrincipalType::Session) => {
            let (session_id, bundle_name) = split_principal_id(target_session)
                .expect("session classification implies a parseable suffix");
            Ok((bundle_name, session_id))
        }
        Some(PrincipalType::User | PrincipalType::Application | PrincipalType::Relay) => {
            let namespace = split_principal_id(target_session)
                .map(|(_, namespace)| namespace)
                .unwrap_or_default();
            Err(relay_error(
                "validation_unsupported_namespace",
                "look target namespace is reserved and names no inspectable session",
                Some(json!({ "target_session": target_session, "namespace": namespace })),
            ))
        }
        None => Ok((dispatch_bundle_name, target_session)),
    }
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

/// Resolves a Send's explicit targets into per-namespace delivery groups,
/// validating every target before any delivery (design D2).
///
/// Each target's `@<namespace>` suffix selects its registry (design D1):
/// `@GLOBAL` rides the dispatch-bundle group as a relay-wide UI target;
/// `@EXTERNAL` / `@RELAY` are reserved and rejected; `@<bundle>` routes to that
/// bundle's group (loaded from the catalog); a bare target resolves within the
/// dispatch bundle. A relay-wide sender with no bundle-qualified target is
/// rejected upstream (`resolve_send_routing_bundle`), so it never reaches here.
/// Unknown targets across any namespace accumulate into a single
/// `validation_unknown_target`.
fn resolve_target_groups(
    sender_bundle: &BundleConfiguration,
    sender_authorization: &AuthorizationContext,
    sender_runtime_directory: &Path,
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
    targets: &[String],
) -> Result<Vec<DeliveryGroup>, RelayError> {
    // Seed the dispatch-bundle group: the session sender's bound bundle, or a
    // relay-wide sender's routing bundle (its first `@<bundle>` target). It
    // hosts bare targets and relay-wide `@GLOBAL` targets in addition to any
    // targets qualified with its own bundle name. A relay-wide sender whose
    // targets are all bare carries no routing bundle and is rejected earlier, at
    // the connection layer (`resolve_send_routing_bundle`); any bare target that
    // reaches here resolves within the dispatch bundle.
    let mut group_order: Vec<String> = vec![sender_bundle.bundle_name.clone()];
    let mut groups_by_bundle: HashMap<String, DeliveryGroup> = HashMap::new();
    groups_by_bundle.insert(
        sender_bundle.bundle_name.clone(),
        DeliveryGroup {
            bundle: sender_bundle.clone(),
            runtime_directory: sender_runtime_directory.to_path_buf(),
            permission_decider_sessions: grant_authorized_ui_sessions(
                sender_authorization,
                sender_bundle,
            ),
            permission_max_pending: permission_max_pending(sender_authorization),
            targets: Vec::new(),
        },
    );

    let mut unknown_targets: Vec<String> = Vec::new();

    for target in targets {
        let requested = target.trim();
        if requested.is_empty() {
            unknown_targets.push(target.clone());
            continue;
        }
        match classify_principal_id(requested) {
            Some(PrincipalType::Application | PrincipalType::Relay) => {
                let namespace = split_principal_id(requested)
                    .map(|(_, namespace)| namespace)
                    .unwrap_or_default();
                return Err(relay_error(
                    "validation_unsupported_namespace",
                    "target namespace is reserved for relay-internal routing",
                    Some(json!({ "target": requested, "namespace": namespace })),
                ));
            }
            Some(PrincipalType::User) => {
                // Relay-wide `@GLOBAL` target: delivered as a UI event keyed off
                // the suffix, so it rides the dispatch-bundle group context.
                if has_ui_session(sender_authorization, requested) {
                    push_target(
                        &mut groups_by_bundle,
                        &sender_bundle.bundle_name,
                        ResolvedTarget {
                            session_id: requested.to_string(),
                            is_ui: true,
                        },
                    );
                } else {
                    unknown_targets.push(target.clone());
                }
            }
            Some(PrincipalType::Session) => {
                let (session_id, bundle_name) = split_principal_id(requested)
                    .expect("session classification implies a parseable suffix");
                match ensure_bundle_group(
                    bundle_name,
                    sender_bundle,
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
                            unknown_targets.push(target.clone());
                        }
                    }
                    Err(BundleGroupError::UnknownBundle) => unknown_targets.push(target.clone()),
                    Err(BundleGroupError::Relay(error)) => return Err(error),
                }
            }
            None => {
                // Bare target (no `@<namespace>` suffix) resolves within the
                // dispatch bundle. Canonical `<id>@<dispatch-bundle>` targets are
                // normalized to this bare form before dispatch, so this also
                // covers a relay-wide sender's targets qualified with its routing
                // bundle.
                let is_member = sender_bundle
                    .members
                    .iter()
                    .any(|member| member.id == requested);
                if is_member || has_ui_session(sender_authorization, requested) {
                    push_target(
                        &mut groups_by_bundle,
                        &sender_bundle.bundle_name,
                        ResolvedTarget {
                            session_id: requested.to_string(),
                            is_ui: !is_member,
                        },
                    );
                } else {
                    unknown_targets.push(target.clone());
                }
            }
        }
    }

    if !unknown_targets.is_empty() {
        return Err(relay_error(
            "validation_unknown_target",
            "one or more targets are not canonical configured target identifiers",
            Some(json!({ "unknown_targets": unknown_targets })),
        ));
    }

    // Preserve target-discovery order and drop the seeded dispatch group when no
    // target landed in it (a relay-wide sender targeting only peer bundles).
    Ok(group_order
        .into_iter()
        .filter_map(|bundle_name| groups_by_bundle.remove(&bundle_name))
        .filter(|group| !group.targets.is_empty())
        .collect())
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
/// configuration and authorization from the catalog when first seen. The
/// dispatch bundle is already seeded; an unconfigured bundle is reported so the
/// caller can fold it into `validation_unknown_target`.
fn ensure_bundle_group(
    bundle_name: &str,
    sender_bundle: &BundleConfiguration,
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
    group_order: &mut Vec<String>,
    groups_by_bundle: &mut HashMap<String, DeliveryGroup>,
) -> Result<(), BundleGroupError> {
    if bundle_name == sender_bundle.bundle_name || groups_by_bundle.contains_key(bundle_name) {
        return Ok(());
    }
    let Some(paths) = bundle_catalog.lookup(bundle_name) else {
        return Err(BundleGroupError::UnknownBundle);
    };
    let bundle = load_bundle_configuration(configuration_root, bundle_name)
        .map_err(|error| BundleGroupError::Relay(map_config(error)))?;
    let authorization =
        load_authorization_context(configuration_root, &bundle).map_err(BundleGroupError::Relay)?;
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
