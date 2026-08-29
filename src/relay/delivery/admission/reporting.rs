//! What the relay *says* about the undelivered queue.
//!
//! This is the only duration-triggered mechanism on the waiting side, and it is
//! sound precisely because elapsing produces a record and nothing else: the pass
//! reads reservations and writes inscriptions, resolves no entry, releases no
//! quota, and touches no scheduling position.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use serde_json::json;

use crate::relay::DeliveryConfiguration;
use crate::runtime::inscriptions::emit_inscription;

use super::super::guard::QueueEntryState;
use super::config::delivery_configuration;
use super::ledger::{AdmissionTargetKey, TargetUsage, lock_ledger};

const INSCRIPTION_UNDELIVERED_AGGREGATE: &str = "relay.delivery.undelivered";
const INSCRIPTION_UNDELIVERED_WARNING: &str = "relay.delivery.undelivered.warning";

/// Reporting cadence and threshold for the undelivered queue.
///
/// Separate from [`AdmissionLimits`](super::config::AdmissionLimits) because
/// these govern what the relay *says* rather than what it *accepts*: no value
/// here can refuse, resolve, or reorder anything. Both come from the same
/// `[delivery]` table, projected apart so the reporting pass cannot reach a quota
/// bound.
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
    //
    // The count carried here is the waiting one, taken from the same `Pending`
    // tally the aggregate reports. `per_target` is the quota counter: it is
    // incremented at admission and decremented only at release, so it counts
    // authorized members too, and reading it here would make the warning
    // contradict the aggregate beside it — announcing a target as further behind
    // than it is by counting exactly the members being written to. The warned
    // flag still lives on `per_target`, because suppression has to survive the
    // pass that set it.
    let crossed: Vec<(AdmissionTargetKey, u64, usize)> = oldest
        .into_iter()
        .filter_map(|(target, admitted_at)| {
            let age = now.saturating_duration_since(admitted_at);
            if age < reporting.warning {
                return None;
            }
            if state.per_target.get(&target)?.warned {
                return None;
            }
            let waiting = pending_per_target.get(&target)?.envelopes;
            Some((target, duration_ms(now, admitted_at), waiting))
        })
        .collect();
    for (target, oldest_age_ms, undelivered_envelopes) in crossed {
        let Some(usage) = state.per_target.get_mut(&target) else {
            continue;
        };
        usage.warned = true;
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
