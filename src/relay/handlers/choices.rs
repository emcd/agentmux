use std::path::Path;

use serde_json::json;

use crate::configuration::{BundleConfiguration, ConfigurationRoots, load_bundle_configuration};

use super::super::authorization::{
    AuthorizationContext, authorize_choose, authorize_choose_for_list,
    choose_authorized_ui_sessions, load_authorization_context,
};
use super::super::delivery::{
    ChoiceDecisionKind, ChoiceDecisionRequest, ChoiceEventContext, ChoiceResolutionOutcome,
    PendingChoiceRequest, emit_choices_snapshot_then_replay, list_pending_choice_requests,
    resolve_choice_request,
};
use super::super::{
    ChoiceDecisionRequestContext, PendingChoiceEntry, RelayError, RelayResponse, RequestPrincipal,
    SCHEMA_VERSION, canonical_session_id, map_config, relay_error,
};

pub(super) fn emit_choices_snapshot_for_ui_registration(
    configuration_roots: &ConfigurationRoots,
    namespace: &str,
    runtime_directory: &Path,
    ui_session_id: &str,
) -> Result<(), RelayError> {
    let bundle = load_bundle_configuration(configuration_roots, namespace).map_err(map_config)?;
    let authorization = load_authorization_context(configuration_roots, Some(&bundle))?;
    let authorized_sessions = choose_authorized_ui_sessions(&authorization, &bundle);
    if !authorized_sessions
        .iter()
        .any(|value| value == ui_session_id)
    {
        return Ok(());
    }
    let context = ChoiceEventContext {
        runtime_directory: runtime_directory.to_path_buf(),
        namespace: bundle.bundle_name.clone(),
        authorized_ui_sessions: authorized_sessions,
    };
    emit_choices_snapshot_then_replay(&context, ui_session_id).map_err(|cause| {
        relay_error(
            "internal_unexpected_failure",
            "failed to replay choices snapshot for ui session",
            Some(json!({
                "namespace": bundle.bundle_name,
                "session_id": ui_session_id,
                "cause": cause,
            })),
        )
    })
}

pub(super) fn handle_choices_pick(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    request: ChoiceDecisionRequestContext,
    runtime_directory: &Path,
    principal: Option<RequestPrincipal>,
) -> Result<RelayResponse, RelayError> {
    let ChoiceDecisionRequestContext {
        choice_request_id,
        outcome,
        option_id,
    } = request;
    validate_choice_decision_request(choice_request_id.as_str(), option_id.as_deref())?;
    let decision = choice_decision_kind(outcome.as_str(), option_id.as_ref())?;
    let principal = principal.ok_or_else(|| {
        relay_error(
            "validation_missing_hello",
            "choice decisions require stream-associated principal identity",
            None,
        )
    })?;
    authorize_choose(
        bundle,
        authorization,
        principal.session_id.as_str(),
        choice_request_id.as_str(),
    )?;
    let context = ChoiceEventContext {
        runtime_directory: runtime_directory.to_path_buf(),
        namespace: bundle.bundle_name.clone(),
        authorized_ui_sessions: choose_authorized_ui_sessions(authorization, bundle),
    };
    let outcome = resolve_choice_request(
        &context,
        ChoiceDecisionRequest {
            choice_request_id: choice_request_id.clone(),
            option_id: option_id.clone(),
            decision,
            decided_by: canonical_session_id(
                principal.session_id.as_str(),
                bundle.bundle_name.as_str(),
            ),
        },
    )
    .map_err(|cause| map_choice_resolution_error(cause, &choice_request_id, option_id))?;
    let (outcome_label, reason_code, reason_message, decided_by) = match outcome {
        ChoiceResolutionOutcome::Selected { decided_by, .. } => {
            ("selected".to_string(), None, None, decided_by)
        }
        ChoiceResolutionOutcome::Cancelled {
            decided_by,
            reason_code,
            reason,
        } => (
            "cancelled".to_string(),
            Some(reason_code),
            reason,
            decided_by,
        ),
    };
    Ok(RelayResponse::ChoicesPick {
        schema_version: SCHEMA_VERSION.to_string(),
        status: "resolved".to_string(),
        choice_request_id,
        outcome: outcome_label,
        decided_by: Some(decided_by),
        reason_code,
        reason: reason_message,
    })
}

pub(super) fn handle_choices_list(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    runtime_directory: &Path,
    principal: Option<RequestPrincipal>,
) -> Result<RelayResponse, RelayError> {
    let principal = principal.ok_or_else(|| {
        relay_error(
            "validation_missing_hello",
            "choices list requires stream-associated principal identity",
            None,
        )
    })?;
    authorize_choose_for_list(bundle, authorization, principal.session_id.as_str())?;
    let pending = list_pending_choice_requests(runtime_directory).map_err(|cause| {
        relay_error(
            "internal_unexpected_failure",
            "failed to list pending choice requests",
            Some(json!({ "cause": cause })),
        )
    })?;
    let pending_requests = pending
        .into_iter()
        .map(|record| {
            let mut entry = pending_choice_entry_from_record(record);
            entry.target_session =
                canonical_session_id(entry.target_session.as_str(), bundle.bundle_name.as_str());
            entry
        })
        .collect();
    Ok(RelayResponse::ChoicesList {
        schema_version: SCHEMA_VERSION.to_string(),
        pending_requests,
    })
}

fn validate_choice_decision_request(
    choice_request_id: &str,
    option_id: Option<&str>,
) -> Result<(), RelayError> {
    if choice_request_id.trim().is_empty() {
        return Err(relay_error(
            "validation_invalid_params",
            "choice_request_id must be non-empty",
            Some(json!({"field": "choice_request_id"})),
        ));
    }
    if let Some(option_id) = option_id
        && option_id.trim().is_empty()
    {
        return Err(relay_error(
            "validation_invalid_params",
            "option_id must be non-empty when provided",
            Some(json!({"field": "option_id"})),
        ));
    }
    Ok(())
}

fn choice_decision_kind(
    outcome: &str,
    option_id: Option<&String>,
) -> Result<ChoiceDecisionKind, RelayError> {
    match outcome {
        "selected" => {
            if option_id.is_none() {
                return Err(relay_error(
                    "validation_invalid_params",
                    "selected outcome requires explicit option_id",
                    Some(json!({"field": "option_id", "outcome": "selected"})),
                ));
            }
            Ok(ChoiceDecisionKind::Selected)
        }
        "cancelled" => {
            if option_id.is_some() {
                return Err(relay_error(
                    "validation_invalid_params",
                    "cancelled outcome must omit option_id",
                    Some(json!({"field": "option_id", "outcome": "cancelled"})),
                ));
            }
            Ok(ChoiceDecisionKind::Cancelled)
        }
        _ => Err(relay_error(
            "validation_invalid_params",
            "choice decision outcome must be selected or cancelled",
            Some(json!({"field": "outcome", "value": outcome})),
        )),
    }
}

fn map_choice_resolution_error(
    cause: String,
    choice_request_id: &str,
    option_id: Option<String>,
) -> RelayError {
    if cause == "runtime_choices_request_already_resolved" {
        relay_error(
            "runtime_choices_request_already_resolved",
            "choice request is already resolved",
            Some(json!({"choice_request_id": choice_request_id})),
        )
    } else if cause.starts_with("validation_invalid_params:") {
        let validation_message = cause
            .split_once(':')
            .map(|(_, message)| message.trim().to_string())
            .unwrap_or_else(|| "invalid choice decision parameters".to_string());
        relay_error(
            "validation_invalid_params",
            validation_message.as_str(),
            Some(json!({"field": "option_id", "value": option_id})),
        )
    } else {
        relay_error(
            "internal_unexpected_failure",
            "failed to resolve choice request",
            Some(json!({
                "choice_request_id": choice_request_id,
                "cause": cause,
            })),
        )
    }
}

fn pending_choice_entry_from_record(record: PendingChoiceRequest) -> PendingChoiceEntry {
    let PendingChoiceRequest {
        message_id,
        choice_request_id,
        target_session,
        requested_kind,
        requested_details,
        enqueued_at,
        ..
    } = record;
    PendingChoiceEntry {
        message_id,
        choice_request_id,
        target_session,
        requested_kind,
        requested_details,
        enqueued_at,
    }
}
