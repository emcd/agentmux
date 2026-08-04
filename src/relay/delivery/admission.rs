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
    time::{Duration, Instant},
};

use serde_json::json;

use super::guard::{
    AttemptId, BatchId, GuardKey, QueueEntryState, SubmissionEvidence, resolve_from_evidence,
};
use crate::configuration::{BundleConfiguration, SessionType};
use crate::relay::DeliveryConfiguration;
use crate::runtime::inscriptions::emit_inscription;
use crate::transports::HandoverDimensions;

use super::super::{RelayError, canonical_session_id, relay_error};

const ERROR_CODE_QUEUE_FULL: &str = "runtime_delivery_queue_full";
const ERROR_CODE_PAYLOAD_TOO_LARGE: &str = "validation_payload_too_large";

const INSCRIPTION_CONFIGURATION_CONFLICT: &str = "relay.delivery.configuration.conflict";
const INSCRIPTION_UNDELIVERED_AGGREGATE: &str = "relay.delivery.undelivered";
const INSCRIPTION_UNDELIVERED_WARNING: &str = "relay.delivery.undelivered.warning";

/// The four admission quota limits, projected out of the `[delivery]` table.
///
/// Carried as a value rather than read from configuration at each call site, so
/// the reservation logic takes its bounds as an argument and stays testable
/// without process-global state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::relay) struct AdmissionLimits {
    pub(in crate::relay) queued_envelopes_max: usize,
    pub(in crate::relay) queued_bytes_max: u64,
    pub(in crate::relay) queued_envelopes_per_target_max: usize,
    pub(in crate::relay) queued_bytes_per_target_max: u64,
}

impl From<DeliveryConfiguration> for AdmissionLimits {
    fn from(configuration: DeliveryConfiguration) -> Self {
        Self {
            queued_envelopes_max: configuration.queued_envelopes_max,
            queued_bytes_max: configuration.queued_bytes_max,
            queued_envelopes_per_target_max: configuration.queued_envelopes_per_target_max,
            queued_bytes_per_target_max: configuration.queued_bytes_per_target_max,
        }
    }
}

impl Default for AdmissionLimits {
    fn default() -> Self {
        Self::from(DeliveryConfiguration::default())
    }
}

/// The relay's resolved `[delivery]` settings, published once at relay startup.
///
/// Admission runs at the request boundary, far from anything holding the startup
/// configuration, and threading nine values through every send path would buy
/// nothing: the table is relay-wide and immutable for the process lifetime.
/// Before startup publishes — in tests, and on any path that never hosts a relay
/// — reads yield the documented defaults, which is what a missing `relay.toml`
/// resolves to anyway.
static DELIVERY_CONFIGURATION: OnceLock<DeliveryConfiguration> = OnceLock::new();

/// Publishes the resolved `[delivery]` table for the process. Called once, during
/// relay startup, before the accept loop can admit anything.
///
/// A second call with a *different* table would mean two startup paths resolved
/// configuration differently, which no caller could detect from the return value
/// of a setter; it is recorded rather than swallowed. A redundant call with the
/// same values is silent, because nothing observable differs.
pub fn configure_delivery(configuration: DeliveryConfiguration) {
    if let Err(rejected) = DELIVERY_CONFIGURATION.set(configuration)
        && DELIVERY_CONFIGURATION.get() != Some(&rejected)
    {
        emit_inscription(
            INSCRIPTION_CONFIGURATION_CONFLICT,
            &json!({
                "detail": "delivery configuration was already published with different values",
            }),
        );
    }
}

/// The published `[delivery]` table, or the documented defaults before startup
/// publishes one.
#[must_use]
pub(in crate::relay) fn delivery_configuration() -> DeliveryConfiguration {
    DELIVERY_CONFIGURATION.get().copied().unwrap_or_default()
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

/// One admitted entry: its reservation, its lifecycle state, and the evidence
/// its guard resolves from. Held until the entry terminalizes.
///
/// State and reservation live on the same record, under the same lock, because
/// the terminal transition and the quota release are one atomic operation. Split
/// across two structures they could drift, and a released reservation on a
/// non-terminal entry is exactly the double-resolution this guard exists to make
/// impossible.
#[derive(Clone, Debug)]
struct AdmittedEntry {
    target: AdmissionTargetKey,
    canonical_bytes: u64,
    /// When the entry was admitted, which is what the undelivered-queue warning
    /// measures. It is a report of how long the relay has been waiting and never
    /// an input to resolution: no code path compares it against a bound to decide
    /// an outcome.
    admitted_at: Instant,
    state: QueueEntryState,
    /// Assigned at authorization and never reassigned; `None` while `Pending`.
    guard: Option<GuardKey>,
    /// Set when the member is handed to a transport, which is the moment a
    /// target-side effect becomes possible. It is the discriminator in the
    /// guard's evidence order between a provable `not_submitted` and an honest
    /// `submission_unknown`.
    handed_to_transport: bool,
    /// The immutable evidence record for the member's packing unit, once its
    /// submission has produced one. Every member outcome is derived from this
    /// rather than from live state, so a panic partway through fan-out resumes
    /// from the recorded result instead of inventing one for the remainder.
    unit_evidence: Option<SubmissionEvidence>,
}

/// Per-target usage. An entry is one envelope, so the count is the number of
/// live entries for that target.
#[derive(Clone, Copy, Debug, Default)]
struct TargetUsage {
    envelopes: usize,
    bytes: u64,
    /// Set when this target's first-crossing warning has been emitted, so a
    /// backlogged target warns once rather than once per queued message.
    ///
    /// Re-arming is structural rather than a separate reset: the whole usage
    /// record is dropped when the target's last entry terminalizes, so a target
    /// that drains and re-accumulates warns again because its flag went with it.
    warned: bool,
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
        AdmissionLimits::from(delivery_configuration()),
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
            admitted_at: Instant::now(),
            state: QueueEntryState::Pending,
            guard: None,
            handed_to_transport: false,
            unit_evidence: None,
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
}

/// Authorizes one member: the `Pending` to `Authorized` transition, minting the
/// batch's identities and creating the member's guard in the same atomic
/// operation.
///
/// This is the model's sole linearization point. It is a relay-local state
/// transition on the relay's own queue entry — not a call, not a handshake, and
/// not dependent on any transport observing anything — which is why it is
/// trivially atomic and why there is no acceptance race to adjudicate.
///
/// Returns the guard key on success. An entry already past `Pending` is not
/// re-authorized and yields `None`: authorization is irrevocable, so a second
/// one would be a second attempt at a member the relay has already committed.
pub(in crate::relay) fn authorize(message_id: &str, batch: BatchId) -> Option<GuardKey> {
    let mut state = lock_ledger().ok()?;
    let entry = state.entries.get_mut(message_id)?;
    if entry.state != QueueEntryState::Pending {
        return None;
    }
    let key = GuardKey {
        batch,
        attempt: AttemptId::mint(),
    };
    entry.state = QueueEntryState::Authorized;
    entry.guard = Some(key);
    Some(key)
}

/// Records that the member has been handed to its transport, which is the point
/// a target-side effect becomes possible.
///
/// Until this is set, the guard can positively prove nothing was written; after
/// it, partial effect cannot be excluded. Recording it is therefore what keeps a
/// lifecycle trigger from over-claiming in either direction.
pub(in crate::relay) fn note_handed_to_transport(message_id: &str) {
    let Ok(mut state) = lock_ledger() else {
        return;
    };
    if let Some(entry) = state.entries.get_mut(message_id) {
        entry.handed_to_transport = true;
    }
}

/// Records the immutable evidence for the member's packing unit, before any
/// member outcome is derived from it.
///
/// Written once; a later record is ignored, because the first one is what any
/// resumed fan-out must agree with.
pub(in crate::relay) fn record_unit_evidence(message_id: &str, evidence: SubmissionEvidence) {
    let Ok(mut state) = lock_ledger() else {
        return;
    };
    if let Some(entry) = state.entries.get_mut(message_id)
        && entry.unit_evidence.is_none()
    {
        entry.unit_evidence = Some(evidence);
    }
}

/// The outcome of attempting the single terminal transition for one member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::relay) enum TerminalTransition {
    /// This caller performed the transition; admission quota was released here
    /// and nowhere else. `evidence` is what the guard's evidence order resolves
    /// the member to when the caller has no outcome of its own, and `guard`
    /// carries the identities the member was authorized under — `None` for a
    /// member terminalized while still `Pending`, which was never authorized and
    /// so has none.
    Won {
        evidence: SubmissionEvidence,
        guard: Option<GuardKey>,
    },
    /// Another caller already terminalized this member. Report nothing: doing so
    /// would be the duplicate resolution the guard exists to prevent.
    AlreadyTerminal,
    /// No reservation is held under this id.
    ///
    /// Deliberately **not** a licence to report. Two very different situations
    /// produce it: a relay-originated receipt, which bypassed admission because
    /// nothing was ever accepted for it, and an admitted member whose terminal
    /// transition another caller already won and cleaned up. The winning
    /// transition removes the entry — it has to, or the ledger would grow by one
    /// record per message the relay ever delivered — so absence cannot
    /// distinguish them on its own.
    ///
    /// Callers MUST resolve the ambiguity from the task, reporting only for work
    /// known to have bypassed admission. Treating absence as reportable is what
    /// let two competing resolvers each emit an outcome for one accepted member.
    NoReservation,
}

/// Attempts the single terminal transition for one member, releasing its
/// admission quota if and only if this caller wins.
///
/// Duplicate completions converge here rather than racing: the state check and
/// the release happen under one lock, so exactly one caller can observe
/// [`TerminalTransition::Won`]. Every lifecycle trigger routes through this same
/// call, and none of them chooses the outcome — the returned `evidence` comes
/// from the guard's one evidence order.
pub(in crate::relay) fn terminalize(message_id: &str) -> TerminalTransition {
    let Ok(mut state) = lock_ledger() else {
        // A poisoned ledger cannot be reasoned about, and reporting an outcome
        // whose uniqueness we cannot establish is worse than reporting none.
        return TerminalTransition::AlreadyTerminal;
    };
    let Some(entry) = state.entries.get(message_id).cloned() else {
        return TerminalTransition::NoReservation;
    };
    if entry.state == QueueEntryState::Terminal {
        return TerminalTransition::AlreadyTerminal;
    }
    state.entries.remove(message_id);
    state.global.envelopes = state.global.envelopes.saturating_sub(1);
    state.global.bytes = state.global.bytes.saturating_sub(entry.canonical_bytes);
    if let Some(usage) = state.per_target.get_mut(&entry.target) {
        usage.envelopes = usage.envelopes.saturating_sub(1);
        usage.bytes = usage.bytes.saturating_sub(entry.canonical_bytes);
        if usage.envelopes == 0 && usage.bytes == 0 {
            state.per_target.remove(&entry.target);
        }
    }
    TerminalTransition::Won {
        evidence: resolve_from_evidence(entry.unit_evidence, entry.handed_to_transport),
        guard: entry.guard,
    }
}

/// Reporting cadence and threshold for the undelivered queue.
///
/// Separate from [`AdmissionLimits`] because these govern what the relay *says*
/// rather than what it *accepts*: no value here can refuse, resolve, or reorder
/// anything. Both come from the same `[delivery]` table, projected apart so the
/// reporting pass cannot reach a quota bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UndeliveredReporting {
    /// How long a target's oldest undelivered entry may age before its
    /// first-crossing warning.
    pub warning: Duration,
    /// Cadence of the periodic aggregate, and the clock the caller drives
    /// [`report_undelivered_queue`] on.
    pub interval: Duration,
}

impl From<DeliveryConfiguration> for UndeliveredReporting {
    fn from(configuration: DeliveryConfiguration) -> Self {
        Self {
            warning: Duration::from_millis(configuration.undelivered_warning_ms),
            interval: Duration::from_millis(configuration.undelivered_report_interval_ms),
        }
    }
}

impl Default for UndeliveredReporting {
    fn default() -> Self {
        Self::from(DeliveryConfiguration::default())
    }
}

/// The undelivered-queue reporting settings the relay is running with.
#[must_use]
pub fn configured_undelivered_reporting() -> UndeliveredReporting {
    UndeliveredReporting::from(delivery_configuration())
}

/// Reports the undelivered queue: one periodic aggregate, plus a first-crossing
/// warning for each target that has newly aged past the threshold.
///
/// This is the only duration-triggered mechanism on the waiting side, and it is
/// sound precisely because elapsing produces a record and nothing else. The pass
/// reads reservations and writes inscriptions; it resolves no entry, releases no
/// quota, and touches no scheduling position. The single piece of state it does
/// write is each target's warned flag, which exists only to suppress a repeat of
/// its own inscription.
pub fn report_undelivered_queue(reporting: UndeliveredReporting) {
    let now = Instant::now();
    let Ok(mut state) = lock_ledger() else {
        return;
    };

    // Undelivered means *waiting*, not merely reserved. An `Authorized` member
    // has been handed over and is executing under the watchdog's bound; counting
    // it here would report work in progress as a backlog, and would age it
    // toward a warning that describes a target not draining when the target is
    // in fact being written to right now. So this pass scopes to `Pending` and
    // computes its own totals rather than reading the all-state quota counters.
    let mut oldest: HashMap<AdmissionTargetKey, Instant> = HashMap::new();
    let mut pending_per_target: HashMap<AdmissionTargetKey, TargetUsage> = HashMap::new();
    let mut pending_envelopes_total: usize = 0;
    let mut pending_bytes_total: u64 = 0;
    for entry in state
        .entries
        .values()
        .filter(|entry| entry.state == QueueEntryState::Pending)
    {
        oldest
            .entry(entry.target.clone())
            .and_modify(|current| {
                if entry.admitted_at < *current {
                    *current = entry.admitted_at;
                }
            })
            .or_insert(entry.admitted_at);
        let usage = pending_per_target.entry(entry.target.clone()).or_default();
        usage.envelopes += 1;
        usage.bytes += entry.canonical_bytes;
        pending_envelopes_total += 1;
        pending_bytes_total += entry.canonical_bytes;
    }

    // An idle relay emits nothing rather than a recurring zero.
    if !pending_per_target.is_empty() {
        let mut targets: Vec<_> = pending_per_target
            .iter()
            .map(|(target, usage)| {
                json!({
                    "namespace": target.namespace,
                    "target_session": target.target_session,
                    "undelivered_envelopes": usage.envelopes,
                    "undelivered_bytes": usage.bytes,
                    "oldest_age_ms": oldest
                        .get(target)
                        .map_or(0, |admitted_at| duration_ms(now, *admitted_at)),
                })
            })
            .collect();
        // Stable ordering: an operator diffing consecutive aggregates should see
        // queue movement, not HashMap iteration order.
        targets.sort_by(|left, right| {
            left["target_session"]
                .as_str()
                .cmp(&right["target_session"].as_str())
        });
        emit_inscription(
            INSCRIPTION_UNDELIVERED_AGGREGATE,
            &json!({
                "undelivered_envelopes_total": pending_envelopes_total,
                "undelivered_bytes_total": pending_bytes_total,
                "target_total": targets.len(),
                "targets": targets,
            }),
        );
    }

    // First-crossing warnings, deduplicated per target rather than per entry: a
    // backlogged target whose entries all cross together is one condition an
    // operator acts on, not one condition per queued message.
    let crossed: Vec<(AdmissionTargetKey, u64)> = oldest
        .into_iter()
        .filter_map(|(target, admitted_at)| {
            let age = now.saturating_duration_since(admitted_at);
            if age < reporting.warning {
                return None;
            }
            let usage = state.per_target.get(&target)?;
            if usage.warned {
                return None;
            }
            Some((target, duration_ms(now, admitted_at)))
        })
        .collect();
    for (target, oldest_age_ms) in crossed {
        let Some(usage) = state.per_target.get_mut(&target) else {
            continue;
        };
        usage.warned = true;
        let undelivered_envelopes = usage.envelopes;
        emit_inscription(
            INSCRIPTION_UNDELIVERED_WARNING,
            &json!({
                "namespace": target.namespace,
                "target_session": target.target_session,
                "undelivered_envelopes": undelivered_envelopes,
                "oldest_age_ms": oldest_age_ms,
                "warning_ms": reporting.warning.as_millis() as u64,
            }),
        );
    }
}

fn duration_ms(now: Instant, since: Instant) -> u64 {
    u64::try_from(now.saturating_duration_since(since).as_millis()).unwrap_or(u64::MAX)
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
