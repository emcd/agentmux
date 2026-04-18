use std::path::Path;

use serde_json::json;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::{
    configuration::{
        BundleConfiguration, BundleMember, TargetConfiguration, TmuxTargetConfiguration,
    },
    runtime::inscriptions::emit_inscription,
};

use super::authorization::{
    AuthorizationContext, authorize_list, authorize_look, authorize_send, has_ui_session,
    ui_session_display_name,
};
use super::delivery::{
    QuiescenceOptions, aggregate_chat_status, deliver_one_target, enqueue_async_delivery,
    enqueue_sync_delivery, load_acp_snapshot_lines_for_look, prompt_batch_settings,
    refresh_acp_snapshot_for_look,
};
use super::lifecycle::{reconcile_loaded_bundle_for_lifecycle, shutdown_bundle_runtime};
use super::tmux::{capture_pane_tail_lines, resolve_active_pane_target};
use super::{
    AsyncDeliveryTask, ChatDeliveryMode, ChatOutcome, ChatRequestContext, ChatResult, ChatStatus,
    LifecycleBundleResult, ListedBundle, ListedBundleState, ListedSession, ListedSessionTransport,
    LookRequestContext, RelayError, RelayRequest, RelayResponse, SCHEMA_VERSION, relay_error,
};

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
    tmux_socket: &Path,
) -> Result<RelayResponse, RelayError> {
    match request {
        RelayRequest::Up => handle_lifecycle_up(bundle, tmux_socket),
        RelayRequest::Down => handle_lifecycle_down(bundle, tmux_socket),
        RelayRequest::List { sender_session } => handle_list(bundle, authorization, sender_session),
        RelayRequest::Chat {
            request_id,
            sender_session,
            message,
            targets,
            broadcast,
            delivery_mode,
            quiet_window_ms,
            quiescence_timeout_ms,
            acp_turn_timeout_ms,
        } => handle_chat(
            bundle,
            authorization,
            ChatRequestContext {
                request_id,
                sender_session,
                message,
                targets,
                broadcast,
                delivery_mode,
                quiet_window_ms,
                quiescence_timeout_ms,
                acp_turn_timeout_ms,
            },
            tmux_socket,
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
            tmux_socket,
        ),
    }
}

fn handle_lifecycle_up(
    bundle: &BundleConfiguration,
    tmux_socket: &Path,
) -> Result<RelayResponse, RelayError> {
    let report = reconcile_loaded_bundle_for_lifecycle(bundle, tmux_socket)?;
    let changed = report.bootstrap_session.is_some()
        || !report.created_sessions.is_empty()
        || !report.pruned_sessions.is_empty();
    let bundle_result = if changed {
        LifecycleBundleResult {
            bundle_name: bundle.bundle_name.clone(),
            outcome: "hosted".to_string(),
            reason_code: None,
            reason: None,
        }
    } else {
        LifecycleBundleResult {
            bundle_name: bundle.bundle_name.clone(),
            outcome: "skipped".to_string(),
            reason_code: Some("already_hosted".to_string()),
            reason: Some("bundle runtime is already hosted".to_string()),
        }
    };
    Ok(RelayResponse::Lifecycle {
        schema_version: SCHEMA_VERSION.to_string(),
        action: "up".to_string(),
        bundles: vec![bundle_result],
        changed_bundle_count: usize::from(changed),
        skipped_bundle_count: usize::from(!changed),
        failed_bundle_count: 0,
        changed_any: changed,
    })
}

fn handle_lifecycle_down(
    bundle: &BundleConfiguration,
    tmux_socket: &Path,
) -> Result<RelayResponse, RelayError> {
    let report = shutdown_bundle_runtime(tmux_socket)?;
    let changed = !report.pruned_sessions.is_empty() || report.killed_tmux_server;
    let bundle_result = if changed {
        LifecycleBundleResult {
            bundle_name: bundle.bundle_name.clone(),
            outcome: "unhosted".to_string(),
            reason_code: None,
            reason: None,
        }
    } else {
        LifecycleBundleResult {
            bundle_name: bundle.bundle_name.clone(),
            outcome: "skipped".to_string(),
            reason_code: Some("already_unhosted".to_string()),
            reason: Some("bundle runtime is already unhosted".to_string()),
        }
    };
    Ok(RelayResponse::Lifecycle {
        schema_version: SCHEMA_VERSION.to_string(),
        action: "down".to_string(),
        bundles: vec![bundle_result],
        changed_bundle_count: usize::from(changed),
        skipped_bundle_count: usize::from(!changed),
        failed_bundle_count: 0,
        changed_any: changed,
    })
}

fn handle_list(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    sender_session: Option<String>,
) -> Result<RelayResponse, RelayError> {
    let sender_session = sender_session.ok_or_else(|| {
        relay_error(
            "validation_unknown_sender",
            "sender_session is required for list authorization",
            None,
        )
    })?;
    let sender = resolve_sender_identity(
        bundle,
        authorization,
        sender_session.as_str(),
        "sender_session",
    )?;
    authorize_list(bundle, authorization, sender.session_id.as_str())?;
    let sessions = bundle
        .members
        .iter()
        .map(|member| ListedSession {
            id: member.id.clone(),
            name: member.name.clone(),
            transport: match member.target {
                TargetConfiguration::Tmux(_) => ListedSessionTransport::Tmux,
                TargetConfiguration::Acp(_) => ListedSessionTransport::Acp,
            },
        })
        .collect::<Vec<_>>();

    let response = RelayResponse::List {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle: ListedBundle {
            id: bundle.bundle_name.clone(),
            state: ListedBundleState::Up,
            state_reason_code: None,
            state_reason: None,
            sessions,
        },
    };
    if let RelayResponse::List { bundle, .. } = &response {
        emit_inscription(
            "relay.list.response",
            &json!({
                "bundle_name": bundle.id,
                "sender_session": sender.session_id,
                "state": bundle.state,
                "session_count": bundle.sessions.len(),
            }),
        );
    }
    Ok(response)
}

fn handle_chat(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    request: ChatRequestContext,
    tmux_socket: &Path,
) -> Result<RelayResponse, RelayError> {
    let ChatRequestContext {
        request_id,
        sender_session,
        message,
        targets,
        broadcast,
        delivery_mode,
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

    emit_inscription(
        "relay.chat.request",
        &json!({
            "bundle_name": bundle.bundle_name,
            "sender_session": sender.session_id,
            "broadcast": broadcast,
            "delivery_mode": delivery_mode,
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
    let (status, results) = match delivery_mode {
        ChatDeliveryMode::Sync => {
            let quiescence = QuiescenceOptions::for_sync(
                quiet_window_ms,
                quiescence_timeout_ms,
                acp_turn_timeout_ms,
            );
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
                    target_session,
                    target_is_ui,
                    message: message.clone(),
                    message_id,
                    quiescence,
                    batch_settings,
                    tmux_socket: tmux_socket.to_path_buf(),
                    completion_sender: None,
                };
                let result = if task.target_is_ui {
                    deliver_one_target(&task)?
                } else {
                    let target_member = bundle
                        .members
                        .iter()
                        .find(|member| member.id == task.target_session)
                        .ok_or_else(|| {
                            relay_error(
                                "internal_unexpected_failure",
                                "resolved target member is missing from bundle configuration",
                                Some(json!({"target_session": task.target_session})),
                            )
                        })?;
                    match &target_member.target {
                        crate::configuration::TargetConfiguration::Acp(_) => {
                            enqueue_sync_delivery(task)?
                        }
                        crate::configuration::TargetConfiguration::Tmux(_) => {
                            deliver_one_target(&task)?
                        }
                    }
                };
                results.push(result);
            }
            (aggregate_chat_status(&results), results)
        }
        ChatDeliveryMode::Async => {
            let quiescence = QuiescenceOptions::for_async(
                quiet_window_ms,
                quiescence_timeout_ms,
                acp_turn_timeout_ms,
            );
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
                    tmux_socket: tmux_socket.to_path_buf(),
                    completion_sender: None,
                };
                enqueue_async_delivery(task)?;
                emit_inscription(
                    "relay.chat.async.queued",
                    &json!({
                        "bundle_name": bundle.bundle_name,
                        "sender_session": sender.session_id,
                        "target_session": target_session,
                        "message_id": message_id,
                    }),
                );
                results.push(ChatResult {
                    target_session,
                    message_id,
                    outcome: ChatOutcome::Queued,
                    reason_code: None,
                    reason: None,
                    details: None,
                });
            }
            (ChatStatus::Accepted, results)
        }
    };

    let response = RelayResponse::Chat {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle_name: bundle.bundle_name.clone(),
        request_id,
        sender_session: sender.session_id.clone(),
        sender_display_name: sender.display_name.clone(),
        delivery_mode,
        status,
        results,
    };
    if let RelayResponse::Chat {
        bundle_name,
        sender_session,
        status,
        results,
        ..
    } = &response
    {
        let delivered_count = results
            .iter()
            .filter(|result| result.outcome == ChatOutcome::Delivered)
            .count();
        emit_inscription(
            "relay.chat.response",
            &json!({
            "bundle_name": bundle_name,
            "sender_session": sender_session,
            "delivery_mode": delivery_mode,
            "status": status,
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
    tmux_socket: &Path,
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

    let snapshot_lines = match &target.target {
        crate::configuration::TargetConfiguration::Tmux(_) => {
            let pane_target =
                resolve_active_pane_target(tmux_socket, target.id.as_str()).map_err(|reason| {
                    relay_error(
                        "internal_unexpected_failure",
                        "failed to resolve active pane for look target",
                        Some(json!({"target_session": target.id, "cause": reason})),
                    )
                })?;
            capture_pane_tail_lines(tmux_socket, pane_target.as_str(), requested_lines).map_err(
                |reason| {
                    relay_error(
                        "internal_unexpected_failure",
                        "failed to capture look snapshot",
                        Some(json!({"target_session": target.id, "cause": reason})),
                    )
                },
            )?
        }
        crate::configuration::TargetConfiguration::Acp(_) => {
            // Refresh from ACP session replay before reading retained snapshot
            // so look reflects current ACP pane state deterministically.
            if let Err(reason) = refresh_acp_snapshot_for_look(tmux_socket, target) {
                emit_inscription(
                    "relay.look.acp_refresh_failed",
                    &json!({
                        "bundle_name": bundle.bundle_name,
                        "target_session": target.id,
                        "reason": reason,
                    }),
                );
            }
            // ACP look state is runtime-scoped; the runtime directory is resolved
            // from the socket path anchor.
            load_acp_snapshot_lines_for_look(tmux_socket, target.id.as_str(), requested_lines)
                .map_err(|reason| {
                    relay_error(
                        "internal_unexpected_failure",
                        "failed to load ACP look snapshot",
                        Some(json!({"target_session": target.id, "cause": reason})),
                    )
                })?
        }
    };
    let captured_at = time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
    let response = RelayResponse::Look {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle_name: bundle.bundle_name.clone(),
        requester_session: requester.session_id.clone(),
        target_session: target.id.clone(),
        captured_at,
        snapshot_lines,
    };
    if let RelayResponse::Look {
        bundle_name,
        requester_session,
        target_session,
        snapshot_lines,
        ..
    } = &response
    {
        emit_inscription(
            "relay.look.response",
            &json!({
                "bundle_name": bundle_name,
                "requester_session": requester_session,
                "target_session": target_session,
                "snapshot_line_count": snapshot_lines.len(),
                "lines_requested": requested_lines,
            }),
        );
    }
    Ok(response)
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
