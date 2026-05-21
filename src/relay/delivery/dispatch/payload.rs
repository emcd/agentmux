use serde_json::json;

use crate::{
    configuration::{BundleMember, SessionType},
    envelope::{
        AddressIdentity, EnvelopeRenderInput, ManifestPreamble, PromptBatchSettings,
        batch_envelopes, parse_tokenizer_profile, render_envelope,
    },
    runtime::inscriptions::emit_inscription,
};

use super::super::super::stream::resolve_registered_session_type;
use super::super::super::{
    AsyncDeliveryTask, ChatResult, DeliveryPayloadMode, RelayError, SCHEMA_VERSION,
};
use super::super::ui_delivery::deliver_one_target_ui;

const PROMPT_TOKENS_MAX_ENVVAR: &str = "AGENTMUX_MAX_PROMPT_TOKENS";
const TOKENIZER_PROFILE_ENVVAR: &str = "AGENTMUX_TOKENIZER_PROFILE";

pub(super) enum PreparedDeliveryPayload {
    Immediate(ChatResult),
    Batched { prompt_batches: Vec<String> },
}

pub(super) fn resolve_target_member(
    task: &AsyncDeliveryTask,
) -> Result<Option<&BundleMember>, RelayError> {
    let target_member = task
        .bundle
        .members
        .iter()
        .find(|member| member.id == task.target_session);
    if target_member.is_none() && !task.target_is_ui {
        return Err(super::super::super::relay_error(
            "internal_unexpected_failure",
            "resolved target member is missing from bundle configuration",
            Some(json!({"target_session": task.target_session})),
        ));
    }
    Ok(target_member)
}

pub(super) fn prepare_delivery_payload(
    task: &AsyncDeliveryTask,
    target_member: Option<&BundleMember>,
    created_at: &str,
) -> Result<PreparedDeliveryPayload, RelayError> {
    match task.payload_mode {
        DeliveryPayloadMode::EnvelopeMessage => {
            let cc_sessions = task
                .all_target_sessions
                .iter()
                .filter(|candidate| **candidate != task.target_session)
                .cloned()
                .collect::<Vec<_>>();
            let cc_members = task
                .all_target_sessions
                .iter()
                .filter(|candidate| **candidate != task.target_session)
                .filter_map(|session_name| {
                    task.bundle
                        .members
                        .iter()
                        .find(|member| member.id == *session_name)
                })
                .cloned()
                .collect::<Vec<_>>();

            let manifest = ManifestPreamble {
                schema_version: SCHEMA_VERSION.to_string(),
                message_id: task.message_id.clone(),
                bundle_name: task.bundle.bundle_name.clone(),
                sender_session: task.sender.id.clone(),
                target_sessions: vec![task.target_session.clone()],
                cc_sessions: if cc_sessions.is_empty() {
                    None
                } else {
                    Some(cc_sessions.clone())
                },
                created_at: created_at.to_string(),
            };
            emit_inscription(
                "relay.chat.envelope.metadata",
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
            let envelope = render_envelope(&EnvelopeRenderInput {
                manifest,
                from: AddressIdentity {
                    session_name: task.sender.id.clone(),
                    display_name: task.sender.name.clone(),
                },
                to: vec![AddressIdentity {
                    session_name: task.target_session.clone(),
                    display_name: target_member.and_then(|member| member.name.clone()),
                }],
                cc: cc_members
                    .iter()
                    .map(|member| AddressIdentity {
                        session_name: member.id.clone(),
                        display_name: member.name.clone(),
                    })
                    .collect::<Vec<_>>(),
                subject: None,
                body: task.message.clone(),
            });

            if should_route_to_ui(task)? {
                return Ok(PreparedDeliveryPayload::Immediate(deliver_one_target_ui(
                    task,
                    task.sender.id.as_str(),
                    cc_sessions.as_slice(),
                    task.target_session.clone(),
                    task.message_id.clone(),
                    task.message.as_str(),
                )));
            }

            Ok(PreparedDeliveryPayload::Batched {
                prompt_batches: batch_envelopes(&[envelope], task.batch_settings),
            })
        }
        DeliveryPayloadMode::RawInput => {
            if task.target_is_ui {
                return Err(super::super::super::relay_error(
                    "internal_unexpected_failure",
                    "raw delivery tasks do not support ui targets",
                    Some(json!({
                        "target_session": task.target_session,
                    })),
                ));
            }
            Ok(PreparedDeliveryPayload::Batched {
                prompt_batches: vec![task.message.clone()],
            })
        }
    }
}

fn should_route_to_ui(task: &AsyncDeliveryTask) -> Result<bool, RelayError> {
    if task.target_is_ui {
        return Ok(true);
    }
    let resolved_session_type = resolve_registered_session_type(
        task.bundle.bundle_name.as_str(),
        task.target_session.as_str(),
    )
    .map_err(|source| {
        super::super::super::relay_error(
            "internal_unexpected_failure",
            "failed to resolve relay stream session type",
            Some(json!({
                "bundle_name": task.bundle.bundle_name,
                "target_session": task.target_session,
                "cause": source.to_string(),
            })),
        )
    })?;
    Ok(matches!(resolved_session_type, Some(SessionType::Ui)))
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
