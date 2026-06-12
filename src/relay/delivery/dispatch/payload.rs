use serde_json::json;

use crate::{
    configuration::{BundleMember, SessionType, TargetConfiguration},
    envelope::{
        AddressIdentity, EnvelopeRenderInput, ManifestPreamble, PromptBatchSettings,
        batch_envelopes, parse_tokenizer_profile, render_envelope,
    },
    runtime::inscriptions::emit_inscription,
};

use super::super::super::stream::resolve_registered_session_type;
use super::super::super::{
    AsyncDeliveryTask, DeliveryPayloadMode, RelayError, SCHEMA_VERSION, SendResult,
    bare_session_id, canonical_session_id,
};
use super::super::ui_delivery::deliver_one_target_ui;

const PROMPT_TOKENS_MAX_ENVVAR: &str = "AGENTMUX_MAX_PROMPT_TOKENS";
const TOKENIZER_PROFILE_ENVVAR: &str = "AGENTMUX_TOKENIZER_PROFILE";

pub(super) enum PreparedDeliveryPayload {
    Immediate(SendResult),
    Batched { prompt_batches: Vec<String> },
}

/// Outcome of preparing a coalesced delivery batch.
///
/// `Immediate` short-circuits when the head task routes to UI; batch length
/// is always 1 in that case by construction (UI-routed tasks do not coalesce).
/// `Batched` carries the rendered prompt batches plus a count of how many
/// input tasks contributed envelopes to those batches. Any tail tasks beyond
/// `accepted_len` were peeled because the rendered batches did not fit a
/// single ACP prompt; those tasks are pushed back onto the worker's carry
/// buffer for the next iteration.
pub(super) enum PreparedBatchPayload {
    Immediate(SendResult),
    Batched {
        prompt_batches: Vec<String>,
        accepted_len: usize,
    },
}

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

/// Prepares a coalesced batch of envelope-mode tasks for delivery.
///
/// All tasks in `batch` share `(runtime_directory, bundle_name, target_session)`
/// (guaranteed by the per-target worker key) and identical `payload_mode` and
/// `relay_wide_target` (guaranteed by the worker coalesce predicate). Each task's
/// envelope is rendered with its own sender/cc/message_id, then packed via
/// `batch_envelopes` against the head task's `batch_settings`.
///
/// For ACP targets the underlying transport accepts exactly one prompt batch,
/// so when the packer produces more than one batch this function peels tail
/// tasks back to a single-batch result. The number of tasks that successfully
/// contributed to the returned `prompt_batches` is reported via `accepted_len`;
/// the caller re-queues `batch[accepted_len..]` for the next iteration.
pub(super) fn prepare_batch_delivery_payload(
    batch: &[AsyncDeliveryTask],
    target_member: Option<&BundleMember>,
    created_at: &str,
) -> Result<PreparedBatchPayload, RelayError> {
    debug_assert!(
        !batch.is_empty(),
        "prepare_batch_delivery_payload requires a non-empty batch",
    );
    let head = &batch[0];
    debug_assert!(
        matches!(head.payload_mode, DeliveryPayloadMode::EnvelopeMessage),
        "batched payloads only support envelope-mode tasks; head was {:?}",
        head.payload_mode,
    );

    if should_route_to_ui(head)? {
        // UI heads are non-coalescable (the worker enforces relay_wide_target ==
        // false to coalesce), so this path always has batch.len() == 1.
        debug_assert_eq!(batch.len(), 1, "UI-routed envelope tasks must not coalesce",);
        let cc_sessions = co_recipient_sessions(head);
        return Ok(PreparedBatchPayload::Immediate(deliver_one_target_ui(
            head,
            head.sender.id.as_str(),
            cc_sessions.as_slice(),
            head.target_session.clone(),
            head.message_id.clone(),
            head.message.as_str(),
        )));
    }

    let rendered = batch
        .iter()
        .map(|task| render_task_envelope(task, target_member, created_at))
        .collect::<Vec<_>>();
    let target_is_acp = matches!(
        target_member.map(|member| &member.target),
        Some(TargetConfiguration::Acp(_)),
    );

    let mut accepted_len = batch.len();
    let mut prompt_batches = batch_envelopes(&rendered, head.batch_settings);
    if target_is_acp && prompt_batches.len() > 1 {
        // ACP delivery accepts exactly one prompt batch per dispatch. Peel
        // tail envelopes until the packer produces a single batch; the peeled
        // tasks return to the worker carry buffer. A single envelope that
        // overflows on its own still ships as one batch (we never return an
        // empty accepted_len): if the first envelope alone produces > 1
        // batch, that condition predates batch-drain and remains handled by
        // the existing one-batch debug_assert in `deliver_one_target_acp`.
        while accepted_len > 1 && prompt_batches.len() > 1 {
            accepted_len -= 1;
            prompt_batches = batch_envelopes(&rendered[..accepted_len], head.batch_settings);
        }
    }
    Ok(PreparedBatchPayload::Batched {
        prompt_batches,
        accepted_len,
    })
}

/// Renders one task's envelope without batching, shared by the single-task
/// and batch payload preparers, and emits the per-task
/// `relay.send.envelope.metadata` inscription so each task's envelope is
/// independently traceable. All address identities carry canonical
/// `session@namespace` ids so recipients in any namespace can derive a reply
/// address.
fn render_task_envelope(
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
fn co_recipient_sessions(task: &AsyncDeliveryTask) -> Vec<String> {
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

pub(super) fn prepare_delivery_payload(
    task: &AsyncDeliveryTask,
    target_member: Option<&BundleMember>,
    created_at: &str,
) -> Result<PreparedDeliveryPayload, RelayError> {
    match task.payload_mode {
        DeliveryPayloadMode::EnvelopeMessage => {
            if should_route_to_ui(task)? {
                let cc_sessions = co_recipient_sessions(task);
                return Ok(PreparedDeliveryPayload::Immediate(deliver_one_target_ui(
                    task,
                    task.sender.id.as_str(),
                    cc_sessions.as_slice(),
                    task.target_session.clone(),
                    task.message_id.clone(),
                    task.message.as_str(),
                )));
            }

            let envelope = render_task_envelope(task, target_member, created_at);
            Ok(PreparedDeliveryPayload::Batched {
                prompt_batches: batch_envelopes(&[envelope], task.batch_settings),
            })
        }
        DeliveryPayloadMode::RawInput => {
            if task.relay_wide_target {
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
    if task.relay_wide_target {
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
