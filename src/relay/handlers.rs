use std::path::Path;

use serde_json::json;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::{configuration::BundleConfiguration, runtime::inscriptions::emit_inscription};

use super::authorization::{AuthorizationContext, authorize_list, authorize_look, authorize_send};
use super::delivery::{
    QuiescenceOptions, aggregate_chat_status, deliver_one_target, enqueue_async_delivery,
    load_acp_snapshot_lines_for_look, prompt_batch_settings,
};
use super::lifecycle::{reconcile_loaded_bundle_for_lifecycle, shutdown_bundle_runtime};
use super::tmux::{capture_pane_tail_lines, resolve_active_pane_target};
use super::{
    AsyncDeliveryTask, ChatDeliveryMode, ChatOutcome, ChatRequestContext, ChatResult, ChatStatus,
    LifecycleBundleResult, LookRequestContext, Recipient, RelayError, RelayRequest, RelayResponse,
    SCHEMA_VERSION, relay_error,
};

const LOOK_LINES_DEFAULT: usize = 120;
const LOOK_LINES_MAX: usize = 1000;

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
    let sender = bundle
        .members
        .iter()
        .find(|member| member.id == sender_session)
        .ok_or_else(|| {
            relay_error(
                "validation_unknown_sender",
                "sender_session is not in bundle configuration",
                Some(json!({"sender_session": sender_session})),
            )
        })?;
    authorize_list(bundle, authorization, sender.id.as_str())?;
    let recipients = bundle
        .members
        .iter()
        .filter(|member| member.id != sender.id)
        .map(|member| Recipient {
            session_name: member.id.clone(),
            display_name: member.name.clone(),
        })
        .collect::<Vec<_>>();

    let response = RelayResponse::List {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle_name: bundle.bundle_name.clone(),
        recipients,
    };
    if let RelayResponse::List {
        bundle_name,
        recipients,
        ..
    } = &response
    {
        emit_inscription(
            "relay.list.response",
            &json!({
                "bundle_name": bundle_name,
                "sender_session": sender.id,
                "recipient_count": recipients.len(),
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

    let sender = bundle
        .members
        .iter()
        .find(|member| member.id == sender_session)
        .cloned()
        .ok_or_else(|| {
            relay_error(
                "validation_unknown_sender",
                "sender_session is not in bundle configuration",
                Some(json!({"sender_session": sender_session})),
            )
        })?;

    emit_inscription(
        "relay.chat.request",
        &json!({
            "bundle_name": bundle.bundle_name,
            "sender_session": sender.id,
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
            .filter(|member| member.id != sender.id)
            .map(|member| member.id.clone())
            .collect::<Vec<_>>()
    } else {
        resolve_explicit_targets(bundle, &targets)?
    };
    authorize_send(
        bundle,
        authorization,
        sender.id.as_str(),
        resolved_targets.as_slice(),
    )?;

    let all_target_sessions = resolved_targets.clone();
    let batch_settings = prompt_batch_settings();
    let (status, results) = match delivery_mode {
        ChatDeliveryMode::Sync => {
            let quiescence = QuiescenceOptions::for_sync(quiet_window_ms, quiescence_timeout_ms);
            let mut results = Vec::with_capacity(resolved_targets.len());
            for target_session in resolved_targets {
                let message_id = Uuid::new_v4().to_string();
                let task = AsyncDeliveryTask {
                    bundle: bundle.clone(),
                    sender: sender.clone(),
                    all_target_sessions: all_target_sessions.clone(),
                    target_session,
                    message: message.clone(),
                    message_id,
                    quiescence,
                    batch_settings,
                    tmux_socket: tmux_socket.to_path_buf(),
                };
                let result = deliver_one_target(&task)?;
                results.push(result);
            }
            (aggregate_chat_status(&results), results)
        }
        ChatDeliveryMode::Async => {
            let quiescence = QuiescenceOptions::for_async(quiet_window_ms, quiescence_timeout_ms);
            let mut results = Vec::with_capacity(resolved_targets.len());
            for target_session in resolved_targets {
                let message_id = Uuid::new_v4().to_string();
                let task = AsyncDeliveryTask {
                    bundle: bundle.clone(),
                    sender: sender.clone(),
                    all_target_sessions: all_target_sessions.clone(),
                    target_session: target_session.clone(),
                    message: message.clone(),
                    message_id: message_id.clone(),
                    quiescence,
                    batch_settings,
                    tmux_socket: tmux_socket.to_path_buf(),
                };
                enqueue_async_delivery(task)?;
                emit_inscription(
                    "relay.chat.async.queued",
                    &json!({
                        "bundle_name": bundle.bundle_name,
                        "sender_session": sender.id,
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
        sender_session: sender.id.clone(),
        sender_display_name: sender.name.clone(),
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

    let requester = bundle
        .members
        .iter()
        .find(|member| member.id == requester_session)
        .ok_or_else(|| {
            relay_error(
                "validation_unknown_sender",
                "requester_session is not in bundle configuration",
                Some(json!({"requester_session": requester_session})),
            )
        })?;
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
        requester.id.as_str(),
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
        crate::configuration::TargetConfiguration::Acp(_) => load_acp_snapshot_lines_for_look(
            // ACP look state is runtime-scoped; the runtime directory is resolved
            // from the socket path anchor.
            tmux_socket,
            target.id.as_str(),
            requested_lines,
        )
        .map_err(|reason| {
            relay_error(
                "internal_unexpected_failure",
                "failed to load ACP look snapshot",
                Some(json!({"target_session": target.id, "cause": reason})),
            )
        })?,
    };
    let captured_at = time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
    let response = RelayResponse::Look {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle_name: bundle.bundle_name.clone(),
        requester_session: requester.id.clone(),
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

fn resolve_explicit_targets(
    bundle: &BundleConfiguration,
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

        let matched_by_name = bundle
            .members
            .iter()
            .filter(|member| member.name.as_deref() == Some(requested))
            .collect::<Vec<_>>();
        match matched_by_name.as_slice() {
            [] => unknown_targets.push(target.clone()),
            [member] => resolved.push(member.id.clone()),
            _ => {
                let matching_sessions = matched_by_name
                    .iter()
                    .map(|member| member.id.clone())
                    .collect::<Vec<_>>();
                return Err(relay_error(
                    "validation_ambiguous_recipient",
                    "target matches multiple configured session names",
                    Some(json!({
                        "target": target,
                        "matching_sessions": matching_sessions,
                    })),
                ));
            }
        }
    }

    if !unknown_targets.is_empty() {
        return Err(relay_error(
            "validation_unknown_recipient",
            "one or more targets are not in bundle configuration",
            Some(json!({"unknown_targets": unknown_targets})),
        ));
    }
    Ok(resolved)
}
