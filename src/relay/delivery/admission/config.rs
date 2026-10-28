//! The resolved `[delivery]` table, and the quota bounds projected out of it.

use crate::relay::DeliveryConfiguration;

/// The four admission quota limits, projected out of the `[delivery]` table.
///
/// Carried as a value rather than read from configuration at each call site, so
/// the reservation logic takes its bounds as an argument and stays testable
/// without process-global state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AdmissionLimits {
    pub(super) queued_envelopes_max: usize,
    pub(super) queued_bytes_max: u64,
    pub(super) queued_envelopes_per_target_max: usize,
    pub(super) queued_bytes_per_target_max: u64,
}

impl From<DeliveryConfiguration> for AdmissionLimits {
    fn from(configuration: DeliveryConfiguration) -> Self {
        Self {
            queued_envelopes_max: configuration.queued_envelopes_max,
            queued_bytes_max: configuration.queued_bytes_max,
            queued_envelopes_per_target_max: configuration
                .queued_envelopes_per_target_max,
            queued_bytes_per_target_max: configuration
                .queued_bytes_per_target_max,
        }
    }
}

impl Default for AdmissionLimits {
    fn default() -> Self {
        Self::from(DeliveryConfiguration::default())
    }
}
