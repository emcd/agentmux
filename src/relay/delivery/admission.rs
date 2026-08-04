//! Relay-owned admission control for the async delivery queue.
//!
//! Admission is the first of the four delivery events. It runs at the request
//! boundary, before `queued` is returned, and it is the only place a send is
//! refused for capacity: once an entry is admitted the relay waits for its target
//! indefinitely rather than resolving it on a clock, so the queue's growth has to
//! be bounded here or nowhere.
//!
//! Three refusals live here, and each rejects at the request boundary rather than
//! queueing something that cannot progress:
//!
//! - **Quota** — an entry reserves envelope count and canonical payload bytes
//!   against both a per-target and a relay-global limit. The reservation is
//!   atomic across both: a single lock covers the check and the increment, so two
//!   concurrent sends cannot both observe headroom that only one of them can have.
//! - **Handover dimensions** — an envelope whose canonical payload alone exceeds
//!   what its transport will ever accept is rejected, because queueing it would
//!   park a message no partition could carry.
//! - **Pubsub** — a forward-declared stub with no delivery path is refused
//!   synchronously, so no work is authorized merely to discover it.
//!
//! Reserved quota is released at terminalization and nowhere else. Release is
//! keyed on the entry's message id and is idempotent: an id the ledger never
//! admitted (a relay-originated terminal-outcome receipt, which bypasses
//! admission because nothing accepted it) releases nothing, and a second release
//! of the same entry is a no-op.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use serde_json::json;

use crate::configuration::{BundleConfiguration, SessionType};
use crate::transports::HandoverDimensions;

use super::super::{RelayError, canonical_session_id, relay_error};

/// Relay-global admission quota, envelope count.
const QUEUED_ENVELOPES_MAX: usize = 10_000;
/// Relay-global admission quota, canonical payload bytes.
const QUEUED_BYTES_MAX: u64 = 268_435_456;
/// Per-target admission quota, envelope count.
const QUEUED_ENVELOPES_PER_TARGET_MAX: usize = 1_000;
/// Per-target admission quota, canonical payload bytes.
const QUEUED_BYTES_PER_TARGET_MAX: u64 = 33_554_432;

const ERROR_CODE_QUEUE_FULL: &str = "runtime_delivery_queue_full";
const ERROR_CODE_PAYLOAD_TOO_LARGE: &str = "validation_payload_too_large";

/// The four admission quota limits.
///
/// Carried as a value rather than read from the constants at each call site so
/// the `[delivery]` configuration table can supply them without touching the
/// reservation logic. The constants above are the spec's defaults and are what
/// [`AdmissionLimits::default`] yields until that table lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::relay) struct AdmissionLimits {
    pub(in crate::relay) queued_envelopes_max: usize,
    pub(in crate::relay) queued_bytes_max: u64,
    pub(in crate::relay) queued_envelopes_per_target_max: usize,
    pub(in crate::relay) queued_bytes_per_target_max: u64,
}

impl Default for AdmissionLimits {
    fn default() -> Self {
        Self {
            queued_envelopes_max: QUEUED_ENVELOPES_MAX,
            queued_bytes_max: QUEUED_BYTES_MAX,
            queued_envelopes_per_target_max: QUEUED_ENVELOPES_PER_TARGET_MAX,
            queued_bytes_per_target_max: QUEUED_BYTES_PER_TARGET_MAX,
        }
    }
}

/// Identifies the target a queue entry is admitted against. Same three
/// components as the delivery worker's key, kept as its own type so the ledger
/// does not depend on worker registration: quota is reserved at the request
/// boundary, which can precede the target's worker existing.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(in crate::relay) struct AdmissionTargetKey {
    pub(in crate::relay) namespace: String,
    pub(in crate::relay) runtime_directory: PathBuf,
    pub(in crate::relay) target_session: String,
}

impl AdmissionTargetKey {
    pub(in crate::relay) fn new(
        namespace: &str,
        runtime_directory: &Path,
        target_session: &str,
    ) -> Self {
        Self {
            namespace: namespace.to_string(),
            runtime_directory: runtime_directory.to_path_buf(),
            target_session: target_session.to_string(),
        }
    }
}

/// One admitted entry's reservation, held until the entry terminalizes.
#[derive(Clone, Debug)]
struct AdmittedEntry {
    target: AdmissionTargetKey,
    canonical_bytes: u64,
}

/// Per-target usage. An entry is one envelope, so the count is the number of
/// live entries for that target.
#[derive(Clone, Copy, Debug, Default)]
struct TargetUsage {
    envelopes: usize,
    bytes: u64,
}

#[derive(Default)]
struct LedgerState {
    /// Live reservations by message id. The authoritative record of what is
    /// reserved: the counters below are maintained in the same locked section, so
    /// they cannot drift from it.
    entries: HashMap<String, AdmittedEntry>,
    global: TargetUsage,
    per_target: HashMap<AdmissionTargetKey, TargetUsage>,
}

#[derive(Default)]
pub(in crate::relay) struct AdmissionLedger {
    state: Mutex<LedgerState>,
}

static ADMISSION_LEDGER: OnceLock<AdmissionLedger> = OnceLock::new();

fn ledger() -> &'static AdmissionLedger {
    ADMISSION_LEDGER.get_or_init(AdmissionLedger::default)
}

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
    super::super::stream::registry_target_is_relay_wide(principal.as_str())
        .unwrap_or_else(|| bundle_name == super::super::GLOBAL_NAMESPACE)
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
        AdmissionLimits::default(),
    )
}

pub(in crate::relay) fn admit_with_limits(
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
        return Err(super::super::session_type_not_implemented(
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
    state.entries.insert(
        message_id.to_string(),
        AdmittedEntry {
            target,
            canonical_bytes,
        },
    );
    Ok(())
}

/// Releases the quota an entry reserved at admission.
///
/// Idempotent and safe for an id that was never admitted, which is what lets the
/// single terminal-resolution site call it for every task without discriminating
/// relay-originated receipts from admitted sends.
pub(in crate::relay) fn release(message_id: &str) {
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

fn lock_ledger() -> Result<std::sync::MutexGuard<'static, LedgerState>, RelayError> {
    ledger().state.lock().map_err(|_| {
        relay_error(
            "internal_unexpected_failure",
            "failed to lock the delivery admission ledger",
            None,
        )
    })
}
