use std::path::Path;

use serde_json::json;

use crate::configuration::{BundleConfiguration, load_bundle_configuration};

use super::super::authorization::{
    AuthorizationContext, authorize_grant, authorize_grant_for_list, grant_authorized_ui_sessions,
    load_authorization_context,
};
use super::super::delivery::{
    PendingPermissionRequest, PermissionDecisionKind, PermissionDecisionRequest,
    PermissionEventContext, PermissionResolutionOutcome, emit_permission_snapshot_then_replay,
    list_pending_permission_requests, resolve_permission_request,
};
use super::super::{
    PendingPermissionEntry, PermissionDecisionRequestContext, RelayError, RelayResponse,
    RequestPrincipal, SCHEMA_VERSION, canonical_session_id, map_config, relay_error,
};

pub(super) fn emit_permission_snapshot_for_ui_registration(
    configuration_root: &Path,
    bundle_name: &str,
    runtime_directory: &Path,
    ui_session_id: &str,
) -> Result<(), RelayError> {
    let bundle = load_bundle_configuration(configuration_root, bundle_name).map_err(map_config)?;
    let authorization = load_authorization_context(configuration_root, &bundle)?;
    let authorized_sessions = grant_authorized_ui_sessions(&authorization, &bundle);
    if !authorized_sessions
        .iter()
        .any(|value| value == ui_session_id)
    {
        return Ok(());
    }
    let context = PermissionEventContext {
        runtime_directory: runtime_directory.to_path_buf(),
        bundle_name: bundle.bundle_name.clone(),
        authorized_ui_sessions: authorized_sessions,
    };
    emit_permission_snapshot_then_replay(&context, ui_session_id).map_err(|cause| {
        relay_error(
            "internal_unexpected_failure",
            "failed to replay permission snapshot for ui session",
            Some(json!({
                "bundle_name": bundle.bundle_name,
                "session_id": ui_session_id,
                "cause": cause,
            })),
        )
    })
}

pub(super) fn handle_permission_decision(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    request: PermissionDecisionRequestContext,
    runtime_directory: &Path,
    principal: Option<RequestPrincipal>,
) -> Result<RelayResponse, RelayError> {
    let PermissionDecisionRequestContext {
        permission_request_id,
        outcome,
        option_id,
        bundle_name: request_bundle_name,
        ui_session_id,
    } = request;
    validate_permission_decision_request(
        bundle,
        request_bundle_name.as_deref(),
        ui_session_id.as_ref(),
        permission_request_id.as_str(),
        option_id.as_deref(),
    )?;
    let decision = permission_decision_kind(outcome.as_str(), option_id.as_ref())?;
    let principal = principal.ok_or_else(|| {
        relay_error(
            "validation_missing_hello",
            "permission decisions require stream-associated principal identity",
            None,
        )
    })?;
    authorize_grant(
        bundle,
        authorization,
        principal.session_id.as_str(),
        permission_request_id.as_str(),
    )?;
    let context = PermissionEventContext {
        runtime_directory: runtime_directory.to_path_buf(),
        bundle_name: bundle.bundle_name.clone(),
        authorized_ui_sessions: grant_authorized_ui_sessions(authorization, bundle),
    };
    let outcome = resolve_permission_request(
        &context,
        PermissionDecisionRequest {
            permission_request_id: permission_request_id.clone(),
            option_id: option_id.clone(),
            decision,
            decided_by: canonical_session_id(
                principal.session_id.as_str(),
                bundle.bundle_name.as_str(),
            ),
        },
    )
    .map_err(|cause| map_permission_resolution_error(cause, &permission_request_id, option_id))?;
    let (outcome_label, reason_code, reason_message) = match outcome {
        PermissionResolutionOutcome::Selected { .. } => ("selected".to_string(), None, None),
        PermissionResolutionOutcome::Cancelled {
            reason_code,
            reason,
            ..
        } => ("cancelled".to_string(), Some(reason_code), reason),
    };
    Ok(RelayResponse::PermissionDecision {
        schema_version: SCHEMA_VERSION.to_string(),
        status: "resolved".to_string(),
        permission_request_id,
        outcome: outcome_label,
        reason_code,
        reason: reason_message,
    })
}

pub(super) fn handle_permission_list(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    request_bundle_name: Option<String>,
    runtime_directory: &Path,
    principal: Option<RequestPrincipal>,
) -> Result<RelayResponse, RelayError> {
    if let Some(request_bundle_name) = request_bundle_name.as_deref()
        && request_bundle_name != bundle.bundle_name
    {
        return Err(relay_error(
            "validation_cross_bundle_unsupported",
            "permission list is limited to the associated bundle",
            Some(json!({
                "associated_bundle_name": bundle.bundle_name,
                "requested_bundle_name": request_bundle_name,
            })),
        ));
    }
    let principal = principal.ok_or_else(|| {
        relay_error(
            "validation_missing_hello",
            "permission list requires stream-associated principal identity",
            None,
        )
    })?;
    authorize_grant_for_list(bundle, authorization, principal.session_id.as_str())?;
    let pending = list_pending_permission_requests(runtime_directory).map_err(|cause| {
        relay_error(
            "internal_unexpected_failure",
            "failed to list pending permission requests",
            Some(json!({ "cause": cause })),
        )
    })?;
    let pending_requests = pending
        .into_iter()
        .map(|record| {
            let mut entry = pending_permission_entry_from_record(record);
            entry.target_session =
                canonical_session_id(entry.target_session.as_str(), bundle.bundle_name.as_str());
            entry
        })
        .collect();
    Ok(RelayResponse::PermissionList {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle_name: bundle.bundle_name.clone(),
        pending_requests,
    })
}

fn validate_permission_decision_request(
    bundle: &BundleConfiguration,
    request_bundle_name: Option<&str>,
    ui_session_id: Option<&String>,
    permission_request_id: &str,
    option_id: Option<&str>,
) -> Result<(), RelayError> {
    if let Some(request_bundle_name) = request_bundle_name
        && request_bundle_name != bundle.bundle_name
    {
        return Err(relay_error(
            "validation_cross_bundle_unsupported",
            "permission decisions are limited to the associated bundle in MVP",
            Some(json!({
                "associated_bundle_name": bundle.bundle_name,
                "requested_bundle_name": request_bundle_name,
            })),
        ));
    }
    if ui_session_id.is_some() {
        return Err(relay_error(
            "validation_invalid_params",
            "caller-supplied ui_session_id is not allowed",
            Some(json!({"field": "ui_session_id"})),
        ));
    }
    if permission_request_id.trim().is_empty() {
        return Err(relay_error(
            "validation_invalid_params",
            "permission_request_id must be non-empty",
            Some(json!({"field": "permission_request_id"})),
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

fn permission_decision_kind(
    outcome: &str,
    option_id: Option<&String>,
) -> Result<PermissionDecisionKind, RelayError> {
    match outcome {
        "selected" => {
            if option_id.is_none() {
                return Err(relay_error(
                    "validation_invalid_params",
                    "selected outcome requires explicit option_id",
                    Some(json!({"field": "option_id", "outcome": "selected"})),
                ));
            }
            Ok(PermissionDecisionKind::Selected)
        }
        "cancelled" => {
            if option_id.is_some() {
                return Err(relay_error(
                    "validation_invalid_params",
                    "cancelled outcome must omit option_id",
                    Some(json!({"field": "option_id", "outcome": "cancelled"})),
                ));
            }
            Ok(PermissionDecisionKind::Cancelled)
        }
        _ => Err(relay_error(
            "validation_invalid_params",
            "permission decision outcome must be selected or cancelled",
            Some(json!({"field": "outcome", "value": outcome})),
        )),
    }
}

fn map_permission_resolution_error(
    cause: String,
    permission_request_id: &str,
    option_id: Option<String>,
) -> RelayError {
    if cause == "runtime_permission_request_already_resolved" {
        relay_error(
            "runtime_permission_request_already_resolved",
            "permission request is already resolved",
            Some(json!({"permission_request_id": permission_request_id})),
        )
    } else if cause.starts_with("validation_invalid_params:") {
        let validation_message = cause
            .split_once(':')
            .map(|(_, message)| message.trim().to_string())
            .unwrap_or_else(|| "invalid permission decision parameters".to_string());
        relay_error(
            "validation_invalid_params",
            validation_message.as_str(),
            Some(json!({"field": "option_id", "value": option_id})),
        )
    } else {
        relay_error(
            "internal_unexpected_failure",
            "failed to resolve permission request",
            Some(json!({
                "permission_request_id": permission_request_id,
                "cause": cause,
            })),
        )
    }
}

fn pending_permission_entry_from_record(
    record: PendingPermissionRequest,
) -> PendingPermissionEntry {
    let PendingPermissionRequest {
        message_id,
        permission_request_id,
        target_session,
        requested_kind,
        requested_details,
        enqueued_at,
        ..
    } = record;
    PendingPermissionEntry {
        message_id,
        permission_request_id,
        target_session,
        requested_kind,
        requested_details,
        enqueued_at,
    }
}
