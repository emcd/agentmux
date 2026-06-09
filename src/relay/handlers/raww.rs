use std::path::Path;

use serde_json::json;
use uuid::Uuid;

use crate::configuration::{BundleConfiguration, TargetConfiguration};

use super::super::authorization::{
    AuthorizationContext, authorize_route, grant_authorized_ui_sessions, permission_max_pending,
};
use super::super::delivery::{
    QuiescenceOptions, deliver_one_target, enqueue_sync_delivery, prompt_batch_settings,
};
use super::super::routing::{
    Addressing, Capability, OperationProfile, requester_home_namespace, resolve_raww_route,
};
use super::super::{
    AsyncDeliveryTask, DeliveryPayloadMode, ListedSessionTransport, RawwRequestContext, RelayError,
    RelayResponse, SCHEMA_VERSION, SendOutcome, canonical_session_id, relay_error,
    session_type_not_implemented,
};
use super::sender::resolve_sender_identity;

pub(super) fn handle_raww(
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
    // Resolve the target through the shared single-target stage Raww and Look
    // share: a bare (unqualified) target is rejected, and a relay-wide
    // (`@GLOBAL`) or reserved namespace names no session that accepts raw input
    // (`validation_unsupported_namespace`, uniform with Look). The route doubles
    // as the authorization input below. The connection layer routes Raww through
    // the target's own bundle, so the resolved target bundle is this dispatch
    // bundle.
    let route = resolve_raww_route(
        requester_home_namespace(sender.session_id.as_str(), bundle.bundle_name.as_str()),
        sender.session_id.as_str(),
        target_session.as_str(),
    )?;
    let target_route = &route.targets[0];
    let target_bundle_name = target_route.bundle_name.as_str();
    let target_session_id = target_route
        .session_id
        .as_deref()
        .expect("raww target carries a session id");
    let target_member = bundle
        .members
        .iter()
        .find(|member| target_bundle_name == bundle.bundle_name && member.id == target_session_id)
        .ok_or_else(|| {
            relay_error(
                "validation_unknown_target",
                "target_session is not a canonical configured target identifier",
                Some(json!({ "target_session": target_session })),
            )
        })?;
    // Raww authorizes through the uniform routing/authz spine like Send and Look:
    // the requester's `raww` control is resolved in its dispatch (home) bundle,
    // and the single target's relationship sets the required tier. A relay-wide
    // (`@GLOBAL`) requester reaching into a bundle is cross-namespace and so
    // requires `all:all`; a same-bundle target stays at `all:home`, and a
    // self-target floors at `self`. Existence is validated above, so a denial
    // sorts after `validation_unknown_target`.
    authorize_route(
        bundle.bundle_name.as_str(),
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
            return Err(session_type_not_implemented(
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
            return Err(session_type_not_implemented(
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
