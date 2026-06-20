use serde_json::json;

use crate::{
    configuration::BundleMember,
    envelope::{
        AddressIdentity, EnvelopeRenderInput, ManifestPreamble, PromptBatchSettings,
        batch_envelopes, parse_tokenizer_profile, render_envelope,
    },
    runtime::inscriptions::emit_inscription,
};

use super::super::super::{
    AsyncDeliveryTask, DeliveryPayloadMode, RelayError, SCHEMA_VERSION, bare_session_id,
    canonical_session_id,
};

const PROMPT_TOKENS_MAX_ENVVAR: &str = "AGENTMUX_MAX_PROMPT_TOKENS";
const TOKENIZER_PROFILE_ENVVAR: &str = "AGENTMUX_TOKENIZER_PROFILE";

/// Outcome of preparing a coalesced delivery batch.
///
/// Carries the rendered prompt batches plus a count of how many input tasks
/// contributed envelopes to those batches. Any tail tasks beyond `accepted_len`
/// were peeled because the rendered batches did not fit a single ACP prompt;
/// those tasks are pushed back onto the worker's carry buffer for the next
/// iteration. UI-routed targets never reach this path — they are first-class
/// transports delivered via `UiTransport::mailw` in the worker loop.
pub(super) struct PreparedBatchPayload {
    pub(super) prompt_batches: Vec<String>,
    pub(super) accepted_len: usize,
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

    let rendered = batch
        .iter()
        .map(|task| render_task_envelope(task, target_member, created_at))
        .collect::<Vec<_>>();
    // A transport that accepts at most one prompt batch per dispatch
    // (`can_take_batches() == false`, ACP today) forces the single-batch peel
    // below. Derived from the target's session type — the capability-flag family.
    let single_batch_only = target_member
        .map(|member| !member.target.session_type().can_take_batches())
        .unwrap_or(false);

    let mut accepted_len = batch.len();
    let mut prompt_batches = batch_envelopes(&rendered, head.batch_settings);
    if single_batch_only && prompt_batches.len() > 1 {
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
    Ok(PreparedBatchPayload {
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

/// Prepares the prompt batches for a single raw-input delivery task.
///
/// Only `RawInput` tasks reach this path. Both tmux and ACP raww enqueue onto an
/// async per-target worker, whose batch loop delegates RawInput heads to the
/// single-task path verbatim (RawInput never coalesces). Envelope-mode tasks are
/// produced solely by `send.rs` and route through the async batch path
/// (`prepare_batch_delivery_payload`), never here, so this function renders no
/// envelope and carries no UI short-circuit.
pub(super) fn prepare_delivery_payload(
    task: &AsyncDeliveryTask,
) -> Result<Vec<String>, RelayError> {
    if task.relay_wide_target {
        return Err(super::super::super::relay_error(
            "internal_unexpected_failure",
            "raw delivery tasks do not support ui targets",
            Some(json!({
                "target_session": task.target_session,
            })),
        ));
    }
    Ok(vec![task.message.clone()])
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
