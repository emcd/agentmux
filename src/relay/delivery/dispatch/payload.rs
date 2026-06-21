use serde_json::json;

use crate::{
    configuration::BundleMember,
    envelope::{
        AddressIdentity, EnvelopeRenderInput, ManifestPreamble, PromptBatchSettings,
        parse_tokenizer_profile, render_envelope,
    },
    runtime::inscriptions::emit_inscription,
};

use super::super::super::{
    AsyncDeliveryTask, RelayError, SCHEMA_VERSION, bare_session_id, canonical_session_id,
};

const PROMPT_TOKENS_MAX_ENVVAR: &str = "AGENTMUX_MAX_PROMPT_TOKENS";
const TOKENIZER_PROFILE_ENVVAR: &str = "AGENTMUX_TOKENIZER_PROFILE";

pub(super) fn resolve_target_member(
    task: &AsyncDeliveryTask,
) -> Result<Option<&BundleMember>, RelayError> {
    let target_member = task
        .bundle
        .members
        .iter()
        .find(|member| member.id == task.target_session);
    if target_member.is_none() && !task.relay_wide_target {
        return Err(super::super::super::relay_error(
            "internal_unexpected_failure",
            "resolved target member is missing from bundle configuration",
            Some(json!({"target_session": task.target_session})),
        ));
    }
    Ok(target_member)
}

/// Renders one task's envelope and emits the per-task
/// `relay.send.envelope.metadata` inscription so each task's envelope is
/// independently traceable. All address identities carry canonical
/// `session@namespace` ids so recipients in any namespace can derive a reply
/// address. The worker calls this per task before submitting via `mailw`; each
/// transport's internal delivery task combines the contiguous rendered envelopes
/// (respecting its own token budget) into a turn.
pub(super) fn render_task_envelope(
    task: &AsyncDeliveryTask,
    target_member: Option<&BundleMember>,
    created_at: &str,
) -> String {
    let sender_session =
        canonical_session_id(task.sender.id.as_str(), task.sender_bundle_name.as_str());
    let target_session = canonical_target_session(task);
    let cc_sessions = co_recipient_sessions(task);

    let manifest = ManifestPreamble {
        schema_version: SCHEMA_VERSION.to_string(),
        message_id: task.message_id.clone(),
        bundle_name: task.bundle.bundle_name.clone(),
        sender_session: sender_session.clone(),
        target_sessions: vec![target_session.clone()],
        cc_sessions: if cc_sessions.is_empty() {
            None
        } else {
            Some(cc_sessions)
        },
        created_at: created_at.to_string(),
    };
    emit_inscription(
        "relay.send.envelope.metadata",
        &json!({
            "schema_version": manifest.schema_version,
            "message_id": manifest.message_id,
            "bundle_name": manifest.bundle_name,
            "sender_session": manifest.sender_session,
            "target_sessions": manifest.target_sessions,
            "cc_sessions": manifest.cc_sessions,
            "created_at": manifest.created_at,
        }),
    );
    render_envelope(&EnvelopeRenderInput {
        manifest,
        from: AddressIdentity {
            session_name: sender_session,
            display_name: task.sender.name.clone(),
        },
        to: vec![AddressIdentity {
            session_name: target_session,
            display_name: target_member.and_then(|member| member.name.clone()),
        }],
        cc: co_recipient_addresses(task),
        subject: None,
        body: task.message.clone(),
    })
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

/// Builds Cc addresses for the task's co-recipients. Members of the delivery
/// bundle contribute their configured display name; co-recipients in other
/// namespaces are absent from this bundle's configuration and carry the
/// canonical id alone.
fn co_recipient_addresses(task: &AsyncDeliveryTask) -> Vec<AddressIdentity> {
    co_recipient_sessions(task)
        .into_iter()
        .map(|session_name| {
            let local_id = bare_session_id(session_name.as_str(), task.bundle.bundle_name.as_str());
            let display_name = task
                .bundle
                .members
                .iter()
                .find(|member| member.id == local_id)
                .and_then(|member| member.name.clone());
            AddressIdentity {
                session_name,
                display_name,
            }
        })
        .collect()
}

pub(in crate::relay) fn prompt_batch_settings() -> PromptBatchSettings {
    let max_prompt_tokens = std::env::var(PROMPT_TOKENS_MAX_ENVVAR)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(PromptBatchSettings::default().max_prompt_tokens);
    let tokenizer_profile = std::env::var(TOKENIZER_PROFILE_ENVVAR)
        .ok()
        .as_deref()
        .and_then(parse_tokenizer_profile)
        .unwrap_or_default();
    PromptBatchSettings {
        max_prompt_tokens,
        tokenizer_profile,
    }
}
