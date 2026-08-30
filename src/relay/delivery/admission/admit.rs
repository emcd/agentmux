//! The reservation itself: what a target is, what an entry is charged, the three
//! refusals, and the one rollback.

use std::time::Instant;

use serde_json::json;

use crate::configuration::{BundleConfiguration, SessionType};
use crate::relay::{RelayError, canonical_session_id, relay_error};
use crate::transports::HandoverDimensions;

use super::super::guard::QueueEntryState;
use super::config::{AdmissionLimits, delivery_configuration};
use super::ledger::{AdmissionTargetKey, AdmittedEntry, TargetUsage, lock_ledger};

const ERROR_CODE_QUEUE_FULL: &str = "runtime_delivery_queue_full";
const ERROR_CODE_PAYLOAD_TOO_LARGE: &str = "validation_payload_too_large";

/// Whether a target is delivered as a relay-wide principal over the UI stream
/// rather than as a bundle coder. Derived from the unified registry's binding,
/// falling back to namespace classification when the relay-wide principal has no
/// entry yet (not yet connected).
///
/// Lives here because admission is the first point that must know which transport
/// will deliver an entry; the delivery worker's task-shaped wrapper delegates to
/// it so the two cannot disagree about a target's transport kind.
#[must_use]
pub(in crate::relay) fn target_is_relay_wide(bundle_name: &str, target_session: &str) -> bool {
    let principal = canonical_session_id(target_session, bundle_name);
    crate::relay::stream::registry_target_is_relay_wide(principal.as_str())
        .unwrap_or_else(|| bundle_name == crate::relay::GLOBAL_NAMESPACE)
}

/// The transport kind that will deliver to this target.
///
/// A relay-wide principal is served by the UI stream and has no bundle member; a
/// bundle coder declares its own type. A configured target that is neither is a
/// resolution defect rather than a caller error, and says so.
pub(in crate::relay) fn resolve_target_session_type(
    bundle: &BundleConfiguration,
    target_session: &str,
) -> Result<SessionType, RelayError> {
    if target_is_relay_wide(bundle.bundle_name.as_str(), target_session) {
        return Ok(SessionType::Ui);
    }
    bundle
        .members
        .iter()
        .find(|member| member.id == target_session)
        .map(|member| member.target.session_type())
        .ok_or_else(|| {
            relay_error(
                "internal_unexpected_failure",
                "resolved target member is missing from bundle configuration",
                Some(json!({"target_session": target_session})),
            )
        })
}

/// The canonical payload size the relay charges an entry, in bytes.
///
/// Canonical means the payload the relay already holds — the message body — not
/// the text a transport will render for its target. Rendered size is
/// transport-specific and unknown here, and charging it would make admission
/// depend on the packing decision admission exists to precede.
#[must_use]
pub(in crate::relay) fn canonical_payload_bytes(message: &str) -> u64 {
    message.len() as u64
}

/// Admits one entry for one target, reserving its quota.
///
/// Returns the structured rejection for a Pubsub target, an envelope larger than
/// the transport can ever accept, or an exhausted quota. On any rejection nothing
/// is reserved and no entry is recorded.
pub(in crate::relay) fn admit(
    message_id: &str,
    target: AdmissionTargetKey,
    session_type: SessionType,
    canonical_bytes: u64,
) -> Result<(), RelayError> {
    admit_with_limits(
        message_id,
        target,
        session_type,
        canonical_bytes,
        AdmissionLimits::from(delivery_configuration()),
    )
}

pub(super) fn admit_with_limits(
    message_id: &str,
    target: AdmissionTargetKey,
    session_type: SessionType,
    canonical_bytes: u64,
    limits: AdmissionLimits,
) -> Result<(), RelayError> {
    // A session type with no delivery path is refused before any quota is
    // considered: nothing is accepted, so nothing may be reserved, queued, or
    // authorized on its behalf.
    let Some(dimensions) = HandoverDimensions::for_session_type(session_type) else {
        return Err(crate::relay::session_type_not_implemented(
            target.target_session.as_str(),
            session_type,
        ));
    };
    if canonical_bytes > dimensions.canonical_bytes_max {
        return Err(relay_error(
            ERROR_CODE_PAYLOAD_TOO_LARGE,
            "message exceeds the target transport's maximum handover size",
            Some(json!({
                "target_session": target.target_session,
                "session_type": session_type,
                "canonical_bytes": canonical_bytes,
                "canonical_bytes_max": dimensions.canonical_bytes_max,
            })),
        ));
    }

    let mut state = lock_ledger()?;
    let target_usage = state.per_target.get(&target).copied().unwrap_or_default();
    if let Some(exhausted) =
        first_exhausted_limit(&state.global, &target_usage, canonical_bytes, limits)
    {
        return Err(queue_full_error(
            &target,
            canonical_bytes,
            exhausted,
            &state.global,
            &target_usage,
            limits,
        ));
    }

    state.global.envelopes += 1;
    state.global.bytes += canonical_bytes;
    let entry = state.per_target.entry(target.clone()).or_default();
    entry.envelopes += 1;
    entry.bytes += canonical_bytes;
    // The position is taken here, under the same lock as the reservation, because
    // admission is what puts an entry in its target's order. Two concurrent sends
    // that both find headroom are linearized by this lock, and the numbers they
    // leave with are the order they were linearized in.
    let mailbox = state.mailboxes.entry(target.clone()).or_default();
    let sequence = mailbox.next_sequence;
    mailbox.next_sequence = sequence.next();
    state.entries.insert(
        message_id.to_string(),
        AdmittedEntry {
            target,
            canonical_bytes,
            admitted_at: Instant::now(),
            sequence,
            state: QueueEntryState::Queued,
            guard: None,
            unit: None,
        },
    );
    Ok(())
}

/// Rolls back a reservation made moments ago, before the entry was authorized.
///
/// Admission is the one reversible event in the model, and this is that
/// reversal: it exists so a request that is refused partway through can undo the
/// reservations it already made rather than admitting a fraction of itself. It
/// is **not** a terminal transition — nothing is resolved, no outcome is
/// reported, and no receipt is owed, because nothing was ever committed.
///
/// Idempotent, and safe for an id that was never admitted.
pub(in crate::relay) fn rollback_admission(message_id: &str) {
    let Ok(mut state) = lock_ledger() else {
        return;
    };
    let Some(entry) = state.entries.remove(message_id) else {
        return;
    };
    state.global.envelopes = state.global.envelopes.saturating_sub(1);
    state.global.bytes = state.global.bytes.saturating_sub(entry.canonical_bytes);
    if let Some(usage) = state.per_target.get_mut(&entry.target) {
        usage.envelopes = usage.envelopes.saturating_sub(1);
        usage.bytes = usage.bytes.saturating_sub(entry.canonical_bytes);
        if usage.envelopes == 0 && usage.bytes == 0 {
            state.per_target.remove(&entry.target);
        }
    }
    // The position the entry was given is retired rather than handed back.
    // Rewinding the counter would hand a live position to a second entry
    // whenever a concurrent admission had already taken the next one, so the
    // position is instead recorded as one that will never be served — which is
    // what lets the cursor move over it. Left merely absent it would be
    // indistinguishable from a position still waiting for its payload, and the
    // cursor would stall behind it, parking every entry the target received
    // afterwards.
    if let Some(mailbox) = state.mailboxes.get_mut(&entry.target) {
        mailbox.retire(entry.sequence);
    }
}

/// Which of the four limits an admission would breach, checked in a fixed order
/// so the rejection names one cause rather than whichever the arithmetic reached
/// first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExhaustedLimit {
    TargetEnvelopes,
    TargetBytes,
    GlobalEnvelopes,
    GlobalBytes,
}

impl ExhaustedLimit {
    fn scope(self) -> &'static str {
        match self {
            Self::TargetEnvelopes | Self::TargetBytes => "target",
            Self::GlobalEnvelopes | Self::GlobalBytes => "relay",
        }
    }

    fn component(self) -> &'static str {
        match self {
            Self::TargetEnvelopes | Self::GlobalEnvelopes => "envelopes",
            Self::TargetBytes | Self::GlobalBytes => "bytes",
        }
    }
}

fn first_exhausted_limit(
    global: &TargetUsage,
    target: &TargetUsage,
    canonical_bytes: u64,
    limits: AdmissionLimits,
) -> Option<ExhaustedLimit> {
    if target.envelopes + 1 > limits.queued_envelopes_per_target_max {
        return Some(ExhaustedLimit::TargetEnvelopes);
    }
    if target.bytes + canonical_bytes > limits.queued_bytes_per_target_max {
        return Some(ExhaustedLimit::TargetBytes);
    }
    if global.envelopes + 1 > limits.queued_envelopes_max {
        return Some(ExhaustedLimit::GlobalEnvelopes);
    }
    if global.bytes + canonical_bytes > limits.queued_bytes_max {
        return Some(ExhaustedLimit::GlobalBytes);
    }
    None
}

fn queue_full_error(
    target: &AdmissionTargetKey,
    canonical_bytes: u64,
    exhausted: ExhaustedLimit,
    global: &TargetUsage,
    target_usage: &TargetUsage,
    limits: AdmissionLimits,
) -> RelayError {
    relay_error(
        ERROR_CODE_QUEUE_FULL,
        "delivery queue admission quota is exhausted",
        Some(json!({
            "target_session": target.target_session,
            "scope": exhausted.scope(),
            "component": exhausted.component(),
            "canonical_bytes": canonical_bytes,
            "queued_envelopes": target_usage.envelopes,
            "queued_bytes": target_usage.bytes,
            "queued_envelopes_per_target_max": limits.queued_envelopes_per_target_max,
            "queued_bytes_per_target_max": limits.queued_bytes_per_target_max,
            "queued_envelopes_total": global.envelopes,
            "queued_bytes_total": global.bytes,
            "queued_envelopes_max": limits.queued_envelopes_max,
            "queued_bytes_max": limits.queued_bytes_max,
        })),
    )
}
