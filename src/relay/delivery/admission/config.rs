//! The published `[delivery]` table, and the quota bounds projected out of it.

use std::sync::OnceLock;

use serde_json::json;

use crate::relay::DeliveryConfiguration;
use crate::runtime::inscriptions::emit_inscription;

const INSCRIPTION_CONFIGURATION_CONFLICT: &str = "relay.delivery.configuration.conflict";

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
