use serde_json::json;

use crate::{
    configuration::BundleMember,
    envelope::{AddressIdentity, ManifestPreamble, PromptBatchSettings, parse_tokenizer_profile},
    runtime::inscriptions::emit_inscription,
    transports::DeliveryMessage,
};

use super::super::super::{
    AsyncDeliveryTask, RelayError, SCHEMA_VERSION, bare_session_id, canonical_session_id,
};

const PROMPT_TOKENS_MAX_ENVVAR: &str = "AGENTMUX_PROMPT_TOKENS_MAX";
const TOKENIZER_PROFILE_ENVVAR: &str = "AGENTMUX_TOKENIZER_PROFILE";

pub(super) fn resolve_target_member(
    task: &AsyncDeliveryTask,
) -> Result<Option<&BundleMember>, RelayError> {
    let target_member = task
        .bundle
        .members
        .iter()
        .find(|member| member.id == task.target_session);
    if target_member.is_none() && !target_is_relay_wide(task) {
        return Err(super::super::super::relay_error(
            "internal_unexpected_failure",
            "resolved target member is missing from bundle configuration",
            Some(json!({"target_session": task.target_session})),
        ));
    }
    Ok(target_member)
}

/// Whether a delivery task targets a relay-wide principal (delivered via the UI
/// stream by principal id) rather than a bundle coder. Derived from the unified
/// registry's binding for the target, falling back to namespace classification
/// when the relay-wide principal has no entry yet (not yet connected). This
/// replaces the `relay_wide_target` flag that the route used to carry through the
/// delivery task.
pub(super) fn target_is_relay_wide(task: &AsyncDeliveryTask) -> bool {
    let principal = canonical_session_id(
        task.target_session.as_str(),
        task.bundle.bundle_name.as_str(),
    );
    super::super::super::stream::registry_target_is_relay_wide(principal.as_str())
        .unwrap_or_else(|| task.bundle.bundle_name == super::super::super::GLOBAL_NAMESPACE)
}

/// Builds the structured, transport-neutral [`DeliveryMessage`] for one task from
/// relay-authored routing and attribution. All party identities carry canonical
/// `session@namespace` ids so recipients in any namespace can derive a reply
/// address. The relay no longer renders pane-envelope text on the delivery path:
/// the receiving transport renders its own representation (coder transports a
/// pane envelope, UI a stream event) from these fields.
pub(super) fn build_delivery_message(
    task: &AsyncDeliveryTask,
    target_member: Option<&BundleMember>,
    created_at: &str,
) -> DeliveryMessage {
    DeliveryMessage {
        body: task.message.clone(),
        created_at: created_at.to_string(),
        namespace: task.bundle.bundle_name.clone(),
        sender: AddressIdentity {
            session_name: canonical_session_id(
                task.sender.id.as_str(),
                task.sender_namespace.as_str(),
            ),
            display_name: task.sender.name.clone(),
        },
        target: AddressIdentity {
            session_name: canonical_target_session(task),
            display_name: target_member.and_then(|member| member.name.clone()),
        },
        cc: co_recipient_parties(task),
        authenticated_identity: task.authenticated_identity.clone(),
    }
}

/// Emits the per-task `relay.send.envelope.metadata` inscription so each task's
/// envelope is independently traceable. The canonical routing/audit metadata is
/// preserved out-of-band from the same structured [`DeliveryMessage`] the
/// transport renders — never injected into pane text.
pub(super) fn emit_envelope_metadata_inscription(message: &DeliveryMessage, message_id: &str) {
    let cc_sessions: Vec<String> = message
        .cc
        .iter()
        .map(|party| party.session_name.clone())
        .collect();
    let manifest = ManifestPreamble {
        schema_version: SCHEMA_VERSION.to_string(),
        message_id: message_id.to_string(),
        namespace: message.namespace.clone(),
        sender_session: message.sender.session_name.clone(),
        target_sessions: vec![message.target.session_name.clone()],
        cc_sessions: if cc_sessions.is_empty() {
            None
        } else {
            Some(cc_sessions)
        },
        created_at: message.created_at.clone(),
    };
    emit_inscription(
        "relay.send.envelope.metadata",
        &json!({
            "schema_version": manifest.schema_version,
            "message_id": manifest.message_id,
            "namespace": manifest.namespace,
            "sender_session": manifest.sender_session,
            "target_sessions": manifest.target_sessions,
            "cc_sessions": manifest.cc_sessions,
            "created_at": manifest.created_at,
        }),
    );
}

/// Canonical `session@namespace` id of the task's own delivery target.
fn canonical_target_session(task: &AsyncDeliveryTask) -> String {
    canonical_session_id(
        task.target_session.as_str(),
        task.bundle.bundle_name.as_str(),
    )
}

/// Canonical ids of the task's co-recipients: the full recipient list minus
/// the task's own target.
pub(super) fn co_recipient_sessions(task: &AsyncDeliveryTask) -> Vec<String> {
    let target_session = canonical_target_session(task);
    task.all_target_sessions
        .iter()
        .filter(|candidate| **candidate != target_session)
        .cloned()
        .collect()
}

/// Builds Cc parties for the task's co-recipients. Members of the delivery
/// bundle contribute their configured display name; co-recipients in other
/// namespaces are absent from this bundle's configuration and carry the
/// canonical id alone.
fn co_recipient_parties(task: &AsyncDeliveryTask) -> Vec<AddressIdentity> {
    co_recipient_sessions(task)
        .into_iter()
        .map(|session| {
            let local_id = bare_session_id(session.as_str(), task.bundle.bundle_name.as_str());
            let display_name = task
                .bundle
                .members
                .iter()
                .find(|member| member.id == local_id)
                .and_then(|member| member.name.clone());
            AddressIdentity {
                session_name: session,
                display_name,
            }
        })
        .collect()
}

pub(in crate::relay) fn prompt_batch_settings() -> PromptBatchSettings {
    let prompt_tokens_max = std::env::var(PROMPT_TOKENS_MAX_ENVVAR)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(PromptBatchSettings::default().prompt_tokens_max);
    let tokenizer_profile = std::env::var(TOKENIZER_PROFILE_ENVVAR)
        .ok()
        .as_deref()
        .and_then(parse_tokenizer_profile)
        .unwrap_or_default();
    PromptBatchSettings {
        prompt_tokens_max,
        tokenizer_profile,
    }
}
