use std::path::Path;

use serde_json::json;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::{
    configuration::{
        BundleConfiguration, BundleMember, TargetConfiguration, TmuxTargetConfiguration,
    },
    relay::{AcpLookFreshness, AcpLookSnapshotSource, LookSnapshotPayload},
    runtime::{inscriptions::emit_inscription, paths::tmux_socket_path_for_runtime_directory},
};

use super::authorization::{
    AuthorizationContext, authorize_look, authorize_raww, authorize_send, authorize_updown,
    grant_authorized_ui_sessions, has_ui_session, permission_max_pending, ui_session_display_name,
};
use super::delivery::{
    QuiescenceOptions, await_acp_worker_prime_for_look, deliver_one_target,
    derive_acp_look_snapshot, enqueue_async_delivery, enqueue_sync_delivery,
    get_acp_worker_snapshot, get_acp_worker_state, prompt_batch_settings,
};
use super::tmux::{capture_pane_tail_lines, resolve_active_pane_target};
use super::{
    AsyncDeliveryTask, DeliveryPayloadMode, ListedSessionTransport, LookRequestContext,
    PermissionDecisionRequestContext, RawwRequestContext, RelayError, RelayRequest, RelayResponse,
    RequestPrincipal, SCHEMA_VERSION, SendOutcome, SendRequestContext, SendResult, bare_session_id,
    canonical_session_id, relay_error,
};

mod listing;
mod permissions;

const LOOK_LINES_DEFAULT: usize = 120;
const LOOK_LINES_MAX: usize = 1000;

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
) -> Result<RelayResponse, RelayError> {
    let request = normalize_request_identities(request, bundle.bundle_name.as_str());
    match request {
        RelayRequest::Up => {
            authorize_lifecycle_principal(bundle, authorization, principal.as_ref())?;
            listing::handle_bundle_up(bundle, runtime_directory)
        }
        RelayRequest::Down => {
            authorize_lifecycle_principal(bundle, authorization, principal.as_ref())?;
            listing::handle_bundle_down(bundle, runtime_directory)
        }
        RelayRequest::List { sender_session } => {
            listing::handle_list(bundle, authorization, sender_session, runtime_directory)
        }
        RelayRequest::Send {
            request_id,
            sender_session,
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
                sender_session,
                message,
                targets,
                broadcast,
                quiet_window_ms,
                quiescence_timeout_ms,
                acp_turn_timeout_ms,
            },
            runtime_directory,
        ),
        RelayRequest::Look {
            requester_session,
            target_session,
            lines,
            bundle_name: request_bundle_name,
        } => handle_look(
            bundle,
            authorization,
            LookRequestContext {
                requester_session,
                target_session,
                lines,
                bundle_name: request_bundle_name,
            },
            runtime_directory,
        ),
        RelayRequest::Raww {
            request_id,
            sender_session,
            target_session,
            text,
            no_enter,
            bundle_name: request_bundle_name,
        } => handle_raww(
            bundle,
            authorization,
            RawwRequestContext {
                request_id,
                sender_session,
                target_session,
                text,
                no_enter,
                bundle_name: request_bundle_name,
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
    }
}

/// Rewrites canonical `session@bundle` identities in an incoming request to
/// their bundle-local form so internal lookups match configured member ids.
fn normalize_request_identities(request: RelayRequest, bundle_name: &str) -> RelayRequest {
    let bare = |id: String| bare_session_id(id.as_str(), bundle_name);
    match request {
        RelayRequest::List { sender_session } => RelayRequest::List {
            sender_session: sender_session.map(bare),
        },
        RelayRequest::Send {
            request_id,
            sender_session,
            message,
            targets,
            broadcast,
            quiet_window_ms,
            quiescence_timeout_ms,
            acp_turn_timeout_ms,
        } => RelayRequest::Send {
            request_id,
            sender_session: bare(sender_session),
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
            bundle_name: request_bundle_name,
        } => RelayRequest::Look {
            requester_session: bare(requester_session),
            target_session: bare(target_session),
            lines,
            bundle_name: request_bundle_name,
        },
        RelayRequest::Raww {
            request_id,
            sender_session,
            target_session,
            text,
            no_enter,
            bundle_name: request_bundle_name,
        } => RelayRequest::Raww {
            request_id,
            sender_session: bare(sender_session),
            target_session: bare(target_session),
            text,
            no_enter,
            bundle_name: request_bundle_name,
        },
        request @ (RelayRequest::Up
        | RelayRequest::Down
        | RelayRequest::PermissionResolve { .. }
        | RelayRequest::PermissionList { .. }) => request,
    }
}

fn handle_send(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    request: SendRequestContext,
    runtime_directory: &Path,
) -> Result<RelayResponse, RelayError> {
    let SendRequestContext {
        request_id,
        sender_session,
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
        sender_session.as_str(),
        "sender_session",
    )?;
    let sender_member = sender.to_bundle_member();
    let permission_decider_sessions = grant_authorized_ui_sessions(authorization, bundle);
    let queue_max_pending = permission_max_pending(authorization);

    emit_inscription(
        "relay.send.request",
        &json!({
            "bundle_name": bundle.bundle_name,
            "sender_session": sender.session_id,
            "broadcast": broadcast,
            "target_count": targets.len(),
            "message_length": message.len(),
            "request_id": request_id.clone(),
        }),
    );

    let resolved_targets = if broadcast {
        bundle
            .members
            .iter()
            .filter(|member| member.id != sender.session_id)
            .map(|member| member.id.clone())
            .collect::<Vec<_>>()
    } else {
        resolve_explicit_targets(bundle, authorization, &targets)?
    };

    let mut has_tmux_target = false;
    let mut has_acp_target = false;
    for target_session in &resolved_targets {
        if let Some(target_member) = bundle
            .members
            .iter()
            .find(|member| member.id == *target_session)
        {
            match &target_member.target {
                crate::configuration::TargetConfiguration::Tmux(_) => has_tmux_target = true,
                crate::configuration::TargetConfiguration::Acp(_) => has_acp_target = true,
                crate::configuration::TargetConfiguration::Ui
                | crate::configuration::TargetConfiguration::Pubsub => {}
            }
            continue;
        }
        if has_ui_session(authorization, target_session) {
            continue;
        }
        return Err(relay_error(
            "internal_unexpected_failure",
            "resolved target session has no configured transport",
            Some(json!({"target_session": target_session})),
        ));
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
    authorize_send(
        bundle,
        authorization,
        sender.session_id.as_str(),
        resolved_targets.as_slice(),
    )?;

    let all_target_sessions = resolved_targets.clone();
    let batch_settings = prompt_batch_settings();
    let quiescence =
        QuiescenceOptions::for_async(quiet_window_ms, quiescence_timeout_ms, acp_turn_timeout_ms);
    let mut results = Vec::with_capacity(resolved_targets.len());
    for target_session in resolved_targets {
        let message_id = Uuid::new_v4().to_string();
        let target_is_ui = has_ui_session(authorization, target_session.as_str())
            && bundle
                .members
                .iter()
                .all(|member| member.id != target_session);
        let task = AsyncDeliveryTask {
            bundle: bundle.clone(),
            sender: sender_member.clone(),
            all_target_sessions: all_target_sessions.clone(),
            target_session: target_session.clone(),
            target_is_ui,
            message: message.clone(),
            message_id: message_id.clone(),
            quiescence,
            batch_settings,
            runtime_directory: runtime_directory.to_path_buf(),
            completion_sender: None,
            payload_mode: DeliveryPayloadMode::EnvelopeMessage,
            append_enter: true,
            permission_decider_sessions: permission_decider_sessions.clone(),
            permission_max_pending: queue_max_pending,
        };
        enqueue_async_delivery(task)?;
        emit_inscription(
            "relay.send.async.queued",
            &json!({
                "bundle_name": bundle.bundle_name,
                "sender_session": sender.session_id,
                "target_session": target_session,
                "message_id": message_id,
            }),
        );
        results.push(SendResult {
            target_session,
            message_id,
            outcome: SendOutcome::Queued,
            reason_code: None,
            reason: None,
            details: None,
        });
    }
    let results = results
        .into_iter()
        .map(|mut result| {
            result.target_session =
                canonical_session_id(result.target_session.as_str(), bundle.bundle_name.as_str());
            result
        })
        .collect::<Vec<_>>();
    let response = RelayResponse::Send {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle_name: bundle.bundle_name.clone(),
        request_id,
        sender_session: canonical_session_id(
            sender.session_id.as_str(),
            bundle.bundle_name.as_str(),
        ),
        sender_display_name: sender.display_name.clone(),
        results,
    };
    if let RelayResponse::Send {
        bundle_name,
        sender_session,
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
            "sender_session": sender_session,
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
) -> Result<RelayResponse, RelayError> {
    let LookRequestContext {
        requester_session,
        target_session,
        lines,
        bundle_name: request_bundle_name,
    } = request;
    if let Some(request_bundle_name) = request_bundle_name.as_deref()
        && request_bundle_name != bundle.bundle_name
    {
        return Err(relay_error(
            "validation_cross_bundle_unsupported",
            "look is limited to the associated bundle in MVP",
            Some(json!({
                "associated_bundle_name": bundle.bundle_name,
                "requested_bundle_name": request_bundle_name,
            })),
        ));
    }

    let requested_lines = lines.unwrap_or(LOOK_LINES_DEFAULT);
    if !(1..=LOOK_LINES_MAX).contains(&requested_lines) {
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

    let requester = resolve_sender_identity(
        bundle,
        authorization,
        requester_session.as_str(),
        "requester_session",
    )?;
    let target = bundle
        .members
        .iter()
        .find(|member| member.id == target_session)
        .ok_or_else(|| {
            relay_error(
                "validation_unknown_target",
                "target_session is not in bundle configuration",
                Some(json!({"target_session": target_session})),
            )
        })?;
    authorize_look(
        bundle,
        authorization,
        requester.session_id.as_str(),
        target.id.as_str(),
    )?;

    let snapshot = match &target.target {
        crate::configuration::TargetConfiguration::Tmux(_) => {
            let tmux_socket = tmux_socket_path_for_runtime_directory(runtime_directory);
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
            let prime_timed_out =
                await_acp_worker_prime_for_look(bundle, target, runtime_directory).map_err(
                    |reason| {
                        relay_error(
                            "internal_unexpected_failure",
                            "failed to await ACP worker prime for look",
                            Some(json!({"target_session": target.id, "cause": reason})),
                        )
                    },
                )?;
            let worker_state = get_acp_worker_state(
                bundle.bundle_name.as_str(),
                runtime_directory,
                target.id.as_str(),
            );
            let worker_snapshot = get_acp_worker_snapshot(
                bundle.bundle_name.as_str(),
                runtime_directory,
                target.id.as_str(),
            );
            let snapshot = derive_acp_look_snapshot(
                worker_state,
                worker_snapshot.as_deref().map(Vec::as_slice),
                requested_lines,
                prime_timed_out,
            );
            LookSnapshotPayload::AcpEntriesV1 {
                snapshot_entries: snapshot.snapshot_entries,
                freshness: snapshot.freshness,
                snapshot_source: snapshot.snapshot_source,
                stale_reason_code: snapshot.stale_reason_code,
                snapshot_age_ms: snapshot.snapshot_age_ms,
            }
        }
    };
    let response = RelayResponse::Look {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle_name: bundle.bundle_name.clone(),
        requester_session: canonical_session_id(
            requester.session_id.as_str(),
            bundle.bundle_name.as_str(),
        ),
        target_session: canonical_session_id(target.id.as_str(), bundle.bundle_name.as_str()),
        captured_at: time::OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
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
            freshness_label,
            snapshot_source_label,
            stale_reason_code,
            snapshot_age_ms,
        ) = match snapshot {
            LookSnapshotPayload::Lines { snapshot_lines } => {
                ("lines", snapshot_lines.len(), None, None, None, None)
            }
            LookSnapshotPayload::AcpEntriesV1 {
                snapshot_entries,
                freshness,
                snapshot_source,
                stale_reason_code,
                snapshot_age_ms,
            } => (
                "acp_entries_v1",
                snapshot_entries.len(),
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
                "lines_requested": requested_lines,
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
        sender_session,
        target_session,
        text,
        no_enter,
        bundle_name: request_bundle_name,
    } = request;

    if let Some(request_bundle_name) = request_bundle_name.as_deref()
        && request_bundle_name != bundle.bundle_name
    {
        return Err(relay_error(
            "validation_cross_bundle_unsupported",
            "raww is limited to the associated bundle in MVP",
            Some(json!({
                "associated_bundle_name": bundle.bundle_name,
                "requested_bundle_name": request_bundle_name,
            })),
        ));
    }
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
        sender_session.as_str(),
        "sender_session",
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
    authorize_raww(
        bundle,
        authorization,
        sender.session_id.as_str(),
        target_member.id.as_str(),
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
        sender: sender_member,
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

fn authorize_lifecycle_principal(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    principal: Option<&RequestPrincipal>,
) -> Result<(), RelayError> {
    let principal = principal.ok_or_else(|| {
        relay_error(
            "validation_missing_hello",
            "bundle lifecycle requests require stream-associated principal identity",
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

fn resolve_explicit_targets(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    targets: &[String],
) -> Result<Vec<String>, RelayError> {
    let mut resolved = Vec::with_capacity(targets.len());
    let mut unknown_targets = Vec::new();

    for target in targets {
        let requested = target.trim();
        if requested.is_empty() {
            unknown_targets.push(target.clone());
            continue;
        }
        if let Some(member) = bundle.members.iter().find(|member| member.id == requested) {
            resolved.push(member.id.clone());
            continue;
        }
        if has_ui_session(authorization, requested) {
            resolved.push(requested.to_string());
            continue;
        }
        unknown_targets.push(target.clone());
    }

    if !unknown_targets.is_empty() {
        return Err(relay_error(
            "validation_unknown_target",
            "one or more targets are not canonical configured target identifiers",
            Some(json!({"unknown_targets": unknown_targets})),
        ));
    }
    Ok(resolved)
}
