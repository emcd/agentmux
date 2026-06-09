use std::path::Path;

use serde_json::json;
use uuid::Uuid;

use crate::configuration::TargetConfiguration;

use super::super::authorization::{
    authorize_route, grant_authorized_ui_sessions, load_authorization_context,
    permission_max_pending,
};
use super::super::connection::BundleCatalog;
use super::super::delivery::{
    QuiescenceOptions, deliver_one_target, enqueue_sync_delivery, prompt_batch_settings,
};
use super::super::routing::{
    Addressing, Capability, OperationProfile, requester_home_namespace, resolve_raww_route,
};
use super::super::{
    AsyncDeliveryTask, DeliveryPayloadMode, ListedSessionTransport, RelayError, RelayRequest,
    RelayResponse, SCHEMA_VERSION, SendOutcome, bare_session_id, canonical_session_id, relay_error,
    session_type_not_implemented,
};
use super::routed::{load_home_context, resolve_target_bundle};
use super::sender::resolve_sender_in_namespace;

/// Entry point for the namespace-centric raww path. The requester is resolved and
/// authorized in its **home** namespace (`home_namespace`: its bound bundle, or
/// `GLOBAL`); the target's bundle is loaded separately for delivery, so a
/// cross-namespace raww authorizes the sender in its home rather than a borrowed
/// target bundle. See `dispatch_raww`.
pub(in crate::relay) fn handle_raww_routed(
    home_namespace: &str,
    home_runtime_directory: Option<&Path>,
    request: RelayRequest,
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
) -> Result<RelayResponse, RelayError> {
    let RelayRequest::Raww {
        request_id,
        requester_session,
        target_session,
        text,
        no_enter,
    } = request
    else {
        return Err(relay_error(
            "internal_unexpected_request",
            "non-raww request routed to the raww dispatcher",
            None,
        ));
    };

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

    // The requester is identified and authorized in its home namespace (operator
    // policy for `GLOBAL`, or the bundle's policy), never a borrowed target bundle.
    let (home_bundle, authorization) = load_home_context(home_namespace, configuration_root)?;
    let requester_session = bare_session_id(requester_session.as_str(), home_namespace);
    let sender = resolve_sender_in_namespace(
        home_bundle.as_ref(),
        &authorization,
        requester_session.as_str(),
        "requester_session",
    )?;

    // Resolve the target through the shared single-target stage Raww and Look
    // share: a bare (unqualified) target is rejected, and a relay-wide
    // (`@GLOBAL`) or reserved namespace names no session that accepts raw input
    // (`validation_unsupported_namespace`, uniform with Look). The route doubles
    // as the authorization input below.
    let route = resolve_raww_route(
        requester_home_namespace(sender.session_id.as_str(), home_namespace),
        sender.session_id.as_str(),
        target_session.as_str(),
    )?;
    let target_route = &route.targets[0];
    let target_bundle_name = target_route.bundle_name.as_str();
    let target_session_id = target_route
        .session_id
        .as_deref()
        .expect("raww target carries a session id");

    // Load the target's bundle and runtime context separately: a same-namespace
    // target reuses the home bundle, a peer (or any target of a relay-wide
    // requester) is loaded from the catalog. Permission deciders come from the
    // target bundle, where delivery happens.
    let (raww_bundle, raww_runtime_directory) = resolve_target_bundle(
        home_namespace,
        home_bundle.as_ref(),
        home_runtime_directory,
        target_bundle_name,
        configuration_root,
        bundle_catalog,
    )?;
    let target_member = raww_bundle
        .members
        .iter()
        .find(|member| member.id == target_session_id)
        .ok_or_else(|| {
            relay_error(
                "validation_unknown_target",
                "target_session is not a canonical configured target identifier",
                Some(json!({
                    "target_session": canonical_session_id(target_session_id, target_bundle_name),
                })),
            )
        })?;

    // Raww authorizes through the uniform routing/authz spine like Send and Look:
    // the requester's `raww` control is resolved in its home namespace, and the
    // single target's relationship sets the required tier. A cross-namespace
    // requester reaching into a bundle requires `all:all`; a same-namespace target
    // stays at `all:home`, and a self-target floors at `self`. Existence is
    // validated above, so a denial sorts after `validation_unknown_target`.
    authorize_route(
        home_namespace,
        &authorization,
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
    // Permission deciders and the queue bound come from the target bundle's
    // authorization, where delivery is gated.
    let target_authorization = load_authorization_context(configuration_root, Some(&raww_bundle))?;
    let permission_decider_sessions =
        grant_authorized_ui_sessions(&target_authorization, &raww_bundle);
    let queue_max_pending = permission_max_pending(&target_authorization);
    let task = AsyncDeliveryTask {
        bundle: raww_bundle.clone(),
        sender_bundle_name: home_namespace.to_string(),
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
        runtime_directory: raww_runtime_directory,
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
            raww_bundle.bundle_name.as_str(),
        ),
        transport,
        request_id,
        message_id: Some(message_id),
        details,
    })
}
