//! The `[delivery]` table: its documented defaults and permitted ranges, the
//! resolved settings, and the cross-key relations enforced at load.
//!
//! Range checking lives beside the constants it checks against so a key name in
//! an error cannot drift from the bound that rejected the value.

use std::path::Path;

use serde::Deserialize;
use serde_json::json;

use crate::relay::{RelayError, relay_error};

/// `[delivery]` defaults and permitted ranges, one triple per key.
///
/// Zero is outside every range deliberately: a zero quota would reject every
/// message and a zero fence-observation budget would declare a negative fence
/// before any executor could be observed, so the two most dangerous
/// misconfigurations would be indistinguishable from an "unlimited" intent. No
/// value denotes unlimited.
const DELIVERY_SUBMISSION_TIMEOUT_MS: DeliveryRange =
    DeliveryRange::new("delivery.submission-timeout-ms", 5_000, 500, 60_000);
const DELIVERY_FENCE_OBSERVATION_TIMEOUT_MS: DeliveryRange =
    DeliveryRange::new("delivery.fence-observation-timeout-ms", 5_000, 100, 60_000);
/// How long a target may be *continuously* unreachable before its still-waiting
/// members resolve.
///
/// Not a readiness bound. A busy target waits forever, because how long a target
/// stays busy is not evidence about it. This measures repeated observations that
/// the target could not be reached at all, which is evidence, and the default is
/// deliberately generous: a bounce asserts something to the sender that a wait
/// does not, so the cost of waiting slightly too long is lower than the cost of
/// claiming a target is gone while it is restarting.
const DELIVERY_UNREACHABLE_DWELL_MS: DeliveryRange =
    DeliveryRange::new("delivery.unreachable-dwell-ms", 30_000, 1_000, 600_000);
const DELIVERY_QUEUED_ENVELOPES_MAX: DeliveryRange =
    DeliveryRange::new("delivery.queued-envelopes-max", 10_000, 1, 1_000_000);
const DELIVERY_QUEUED_BYTES_MAX: DeliveryRange = DeliveryRange::new(
    "delivery.queued-bytes-max",
    268_435_456,
    1_048_576,
    4_294_967_296,
);
const DELIVERY_QUEUED_ENVELOPES_PER_TARGET_MAX: DeliveryRange = DeliveryRange::new(
    "delivery.queued-envelopes-per-target-max",
    1_000,
    1,
    1_000_000,
);
const DELIVERY_QUEUED_BYTES_PER_TARGET_MAX: DeliveryRange = DeliveryRange::new(
    "delivery.queued-bytes-per-target-max",
    33_554_432,
    1_048_576,
    4_294_967_296,
);
const DELIVERY_UNDELIVERED_WARNING_MS: DeliveryRange = DeliveryRange::new(
    "delivery.undelivered-warning-ms",
    1_800_000,
    60_000,
    86_400_000,
);
const DELIVERY_UNDELIVERED_REPORT_INTERVAL_MS: DeliveryRange = DeliveryRange::new(
    "delivery.undelivered-report-interval-ms",
    300_000,
    30_000,
    3_600_000,
);

/// One `[delivery]` key's documented default and permitted range, carried
/// together so the key name in a range error cannot drift from the bound that
/// rejected the value.
#[derive(Clone, Copy, Debug)]
struct DeliveryRange {
    field: &'static str,
    default: u64,
    minimum: u64,
    maximum: u64,
}

impl DeliveryRange {
    const fn new(field: &'static str, default: u64, minimum: u64, maximum: u64) -> Self {
        Self {
            field,
            default,
            minimum,
            maximum,
        }
    }

    /// Resolves one key: an absent value takes the documented default, a present
    /// value must fall within the range. Defaults are not range-checked because
    /// they are defined inside their own bounds.
    fn resolve(self, supplied: Option<u64>, path: &Path) -> Result<u64, RelayError> {
        let Some(value) = supplied else {
            return Ok(self.default);
        };
        if !(self.minimum..=self.maximum).contains(&value) {
            return Err(relay_error(
                "validation_invalid_arguments",
                "relay delivery setting is out of supported range",
                Some(json!({
                    "path": path.display().to_string(),
                    "field": self.field,
                    "value": value,
                    "minimum": self.minimum,
                    "maximum": self.maximum,
                })),
            ));
        }
        Ok(value)
    }
}

/// Raw `[delivery]` table. These keys govern the relay's own queue, scheduling,
/// and reporting rather than any coder's behavior, which is why they live in
/// `relay.toml` and not `coders.toml`.
///
/// Every key is `u64` here regardless of its resolved type: range validation runs
/// in one denomination, and the two envelope counts narrow to `usize` only after
/// their bound has already rejected anything that could not fit.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct RawRelayDeliverySection {
    #[serde(default)]
    submission_timeout_ms: Option<u64>,
    #[serde(default)]
    fence_observation_timeout_ms: Option<u64>,
    #[serde(default)]
    unreachable_dwell_ms: Option<u64>,
    #[serde(default)]
    queued_envelopes_max: Option<u64>,
    #[serde(default)]
    queued_bytes_max: Option<u64>,
    #[serde(default)]
    queued_envelopes_per_target_max: Option<u64>,
    #[serde(default)]
    queued_bytes_per_target_max: Option<u64>,
    #[serde(default)]
    undelivered_warning_ms: Option<u64>,
    #[serde(default)]
    undelivered_report_interval_ms: Option<u64>,
}

/// Resolved `[delivery]` settings: the two supervision bounds, the unreachable
/// dwell, the four admission quotas, and the two undelivered-queue reporting
/// intervals.
///
/// No field here bounds how long an entry waits for a *reachable* target to peek
/// and write it. The relay never waits on a target's behalf — waiting happens
/// inside the owning transport's delivery-loop executor — so such an entry waits
/// indefinitely by design, and the per-target quota, not a clock, is what bounds
/// the consequence. `submission_timeout_ms` bounds the relay's own supervised
/// execution rather than readiness: it runs from the declaration the relay
/// accepted, so a unit still outstanding past it means the executor supervising
/// that unit has run longer than it is allowed to. `unreachable_dwell_ms`
/// is the one field that does resolve a waiting entry, and only on sustained
/// unreachability — an observation repeatedly made, not a guess standing in for
/// one never made.
///
/// The two undelivered-queue fields govern reporting only. Their sole effect on
/// elapse is an inscription; neither influences a member's outcome, releases
/// quota, nor alters scheduling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeliveryConfiguration {
    pub submission_timeout_ms: u64,
    pub fence_observation_timeout_ms: u64,
    pub unreachable_dwell_ms: u64,
    pub queued_envelopes_max: usize,
    pub queued_bytes_max: u64,
    pub queued_envelopes_per_target_max: usize,
    pub queued_bytes_per_target_max: u64,
    pub undelivered_warning_ms: u64,
    pub undelivered_report_interval_ms: u64,
}

impl Default for DeliveryConfiguration {
    fn default() -> Self {
        Self {
            submission_timeout_ms: DELIVERY_SUBMISSION_TIMEOUT_MS.default,
            fence_observation_timeout_ms: DELIVERY_FENCE_OBSERVATION_TIMEOUT_MS.default,
            unreachable_dwell_ms: DELIVERY_UNREACHABLE_DWELL_MS.default,
            queued_envelopes_max: DELIVERY_QUEUED_ENVELOPES_MAX.default as usize,
            queued_bytes_max: DELIVERY_QUEUED_BYTES_MAX.default,
            queued_envelopes_per_target_max: DELIVERY_QUEUED_ENVELOPES_PER_TARGET_MAX.default
                as usize,
            queued_bytes_per_target_max: DELIVERY_QUEUED_BYTES_PER_TARGET_MAX.default,
            undelivered_warning_ms: DELIVERY_UNDELIVERED_WARNING_MS.default,
            undelivered_report_interval_ms: DELIVERY_UNDELIVERED_REPORT_INTERVAL_MS.default,
        }
    }
}

/// Resolves and validates the `[delivery]` table. An absent table yields the
/// documented defaults; every supplied key is range-checked, then the two
/// cross-key relations between per-target and relay-global quota are enforced.
pub(super) fn resolve_delivery_configuration(
    section: Option<RawRelayDeliverySection>,
    path: &Path,
) -> Result<DeliveryConfiguration, RelayError> {
    let section = section.unwrap_or_default();
    let queued_envelopes_max =
        DELIVERY_QUEUED_ENVELOPES_MAX.resolve(section.queued_envelopes_max, path)?;
    let queued_bytes_max = DELIVERY_QUEUED_BYTES_MAX.resolve(section.queued_bytes_max, path)?;
    let queued_envelopes_per_target_max = DELIVERY_QUEUED_ENVELOPES_PER_TARGET_MAX
        .resolve(section.queued_envelopes_per_target_max, path)?;
    let queued_bytes_per_target_max =
        DELIVERY_QUEUED_BYTES_PER_TARGET_MAX.resolve(section.queued_bytes_per_target_max, path)?;
    // A per-target limit above the relay-global one is unreachable — the global
    // check rejects first in both dimensions — so it is always a mistake rather
    // than a permissive setting, and saying so at load beats letting the operator
    // believe the larger number is in force.
    reject_per_target_quota_above_global(
        DELIVERY_QUEUED_ENVELOPES_PER_TARGET_MAX.field,
        queued_envelopes_per_target_max,
        DELIVERY_QUEUED_ENVELOPES_MAX.field,
        queued_envelopes_max,
        path,
    )?;
    reject_per_target_quota_above_global(
        DELIVERY_QUEUED_BYTES_PER_TARGET_MAX.field,
        queued_bytes_per_target_max,
        DELIVERY_QUEUED_BYTES_MAX.field,
        queued_bytes_max,
        path,
    )?;
    Ok(DeliveryConfiguration {
        submission_timeout_ms: DELIVERY_SUBMISSION_TIMEOUT_MS
            .resolve(section.submission_timeout_ms, path)?,
        fence_observation_timeout_ms: DELIVERY_FENCE_OBSERVATION_TIMEOUT_MS
            .resolve(section.fence_observation_timeout_ms, path)?,
        unreachable_dwell_ms: DELIVERY_UNREACHABLE_DWELL_MS
            .resolve(section.unreachable_dwell_ms, path)?,
        queued_envelopes_max: queued_envelopes_max as usize,
        queued_bytes_max,
        queued_envelopes_per_target_max: queued_envelopes_per_target_max as usize,
        queued_bytes_per_target_max,
        undelivered_warning_ms: DELIVERY_UNDELIVERED_WARNING_MS
            .resolve(section.undelivered_warning_ms, path)?,
        undelivered_report_interval_ms: DELIVERY_UNDELIVERED_REPORT_INTERVAL_MS
            .resolve(section.undelivered_report_interval_ms, path)?,
    })
}

/// Rejects a per-target quota that exceeds its relay-global counterpart, naming
/// both keys and both values so the operator sees the relation that failed rather
/// than one bound in isolation.
fn reject_per_target_quota_above_global(
    per_target_field: &'static str,
    per_target_value: u64,
    global_field: &'static str,
    global_value: u64,
    path: &Path,
) -> Result<(), RelayError> {
    if per_target_value <= global_value {
        return Ok(());
    }
    Err(relay_error(
        "validation_invalid_arguments",
        "relay delivery per-target quota exceeds the relay-global quota",
        Some(json!({
            "path": path.display().to_string(),
            "field": per_target_field,
            "value": per_target_value,
            "global_field": global_field,
            "global_value": global_value,
        })),
    ))
}
