//! `handshakeHello` version and limits negotiation.
//!
//! Major equality plus minor-window overlap (selected_minor is the min
//! of maxes within the overlap), matched-pairs limit resolution (never
//! same-named min by accident), mandatory `latency_declarations`, and
//! deterministic typed refusal — no degraded operation. The adapter
//! originates the hello and validates the result against its own offer
//! and pinned launch inputs; the result carries no target to compare.

use std::{
    collections::HashMap,
    error::Error,
    fmt::{Display, Formatter},
};

use serde_json::{Value, json};

use super::binding::PinnedLaunch;

/// Adapter-supported protocol window: one major, an inclusive minor
/// range. Majors never negotiate; minors interoperate within overlap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VersionOffer {
    pub major: u32,
    pub min_minor: u16,
    pub max_minor: u16,
}

/// Adapter-side limits proposal: our declarations for the negotiation
/// table. The supervisor resolves the effective values; the adapter
/// verifies the result against this proposal and the contract ranges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitsProposal {
    pub max_frame_bytes: u64,
    pub queue_tx_bytes: u64,
    pub queue_rx_bytes: u64,
    pub max_control_pending: u32,
    pub max_events_per_second: u32,
    pub window_seconds: u32,
    pub max_submits_in_flight: u32,
    pub max_choices_in_flight: u32,
    pub max_looks_in_flight: u32,
    pub drain_wait_ms: u64,
    pub latency_declarations: HashMap<String, u64>,
}

/// Negotiated limits as carried by the hello result (post-resolution).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NegotiatedLimits {
    pub max_frame_bytes: u64,
    pub queue_tx_bytes: u64,
    pub queue_rx_bytes: u64,
    pub max_control_pending: u32,
    pub max_events_per_second: u32,
    pub window_seconds: u32,
    pub max_submits_in_flight: u32,
    pub max_choices_in_flight: u32,
    pub max_looks_in_flight: u32,
    pub drain_wait_ms: u64,
}

/// Handshake failure: version skew or invalid negotiated limits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandshakeError {
    MajorMismatch { offered: u32, selected: u32 },
    NoMinorOverlap,
    MinorOutsideWindow { selected: u16 },
    MissingLatencyDeclarations,
    InvalidLimit { field: String, reason: String },
    MalformedResult { reason: String },
    UnknownProfile { profile: String },
}

impl Display for HandshakeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MajorMismatch { offered, selected } => {
                write!(
                    formatter,
                    "major mismatch: offered {offered}, selected {selected}"
                )
            },
            Self::NoMinorOverlap => {
                write!(formatter, "no minor version overlap with supervisor")
            },
            Self::MinorOutsideWindow { selected } => {
                write!(
                    formatter,
                    "selected minor {selected} is outside the offered window"
                )
            },
            Self::MissingLatencyDeclarations => {
                write!(formatter, "latency declarations are required")
            },
            Self::InvalidLimit { field, reason } => {
                write!(formatter, "invalid limit {field}: {reason}")
            },
            Self::MalformedResult { reason } => {
                write!(formatter, "malformed hello result: {reason}")
            },
            Self::UnknownProfile { profile } => {
                write!(formatter, "unknown adapter profile {profile}")
            },
        }
    }
}

impl Error for HandshakeError {}

/// Computes the selected minor as the min of maxes, valid only within
/// the overlap of both windows.
#[must_use]
pub fn negotiate_minor(
    offer_min: u16,
    offer_max: u16,
    peer_min: u16,
    peer_max: u16,
) -> Option<u16> {
    let selected = offer_max.min(peer_max);
    if selected >= offer_min.max(peer_min) {
        Some(selected)
    } else {
        None
    }
}

/// Validates the supervisor-selected versions against the offer:
/// major equality (majors never negotiate) and selected minor within
/// the offered window. The supervisor's window never arrives in the
/// result, so overlap width is the supervisor's to compute; the adapter
/// verifies the selection names a version it offered.
pub fn validate_selection(
    offer: &VersionOffer,
    selected_major: u32,
    selected_minor: u16,
) -> Result<(), HandshakeError> {
    if selected_major != offer.major {
        return Err(HandshakeError::MajorMismatch {
            offered: offer.major,
            selected: selected_major,
        });
    }
    if selected_minor < offer.min_minor || selected_minor > offer.max_minor {
        return Err(HandshakeError::MinorOutsideWindow {
            selected: selected_minor,
        });
    }
    Ok(())
}

fn invalid_limit(field: &str, reason: impl Into<String>) -> HandshakeError {
    HandshakeError::InvalidLimit {
        field: field.to_string(),
        reason: reason.into(),
    }
}

/// Validates negotiated limits against the proposal and the contract
/// ranges: latency declarations present and positive, caps nonzero
/// (drain_wait_ms may be zero), frame bounds within
/// 4096..=67108864, and negotiated values at or below the proposal
/// where the min-resolution rule applies.
pub fn validate_limits(
    proposal: &LimitsProposal,
    negotiated: &NegotiatedLimits,
) -> Result<(), HandshakeError> {
    if proposal.latency_declarations.is_empty()
        || proposal.latency_declarations.values().any(|v| *v == 0)
    {
        return Err(HandshakeError::MissingLatencyDeclarations);
    }
    if !(4096..=67_108_864).contains(&negotiated.max_frame_bytes) {
        return Err(invalid_limit(
            "max_frame_bytes",
            "must lie within 4096..=67108864",
        ));
    }
    if negotiated.max_frame_bytes > proposal.max_frame_bytes {
        return Err(invalid_limit(
            "max_frame_bytes",
            "negotiated value exceeds the proposal",
        ));
    }
    for (field, value) in [
        (
            "max_control_pending",
            u64::from(negotiated.max_control_pending),
        ),
        (
            "max_events_per_second",
            u64::from(negotiated.max_events_per_second),
        ),
        (
            "max_submits_in_flight",
            u64::from(negotiated.max_submits_in_flight),
        ),
        (
            "max_choices_in_flight",
            u64::from(negotiated.max_choices_in_flight),
        ),
        (
            "max_looks_in_flight",
            u64::from(negotiated.max_looks_in_flight),
        ),
    ] {
        if value == 0 {
            return Err(invalid_limit(
                field,
                "zero cap is invalid, not unlimited",
            ));
        }
    }
    if negotiated.window_seconds == 0 || negotiated.window_seconds > 60 {
        return Err(invalid_limit("window_seconds", "must lie within 1..=60"));
    }
    if negotiated.drain_wait_ms > 10_000 {
        return Err(invalid_limit(
            "drain_wait_ms",
            "must lie within 0..=10000",
        ));
    }
    Ok(())
}

/// Builds the `adapter.handshakeHello` request params from pinned
/// launch inputs, the version offer, and the limits proposal. Only the
/// `generic` and `acp` profiles exist; anything else refuses before
/// any wire traffic.
///
/// # Errors
///
/// Returns [`HandshakeError::UnknownProfile`] for any other profile.
pub fn hello_request(
    pinned: &PinnedLaunch,
    offer: &VersionOffer,
    proposal: &LimitsProposal,
) -> Result<Value, HandshakeError> {
    if pinned.adapter_profile != "generic" && pinned.adapter_profile != "acp" {
        return Err(HandshakeError::UnknownProfile {
            profile: pinned.adapter_profile.clone(),
        });
    }
    Ok(json!({
        "protocol_major": offer.major,
        "protocol_minor": offer.max_minor,
        "target_session": pinned.target_session,
        "adapter_executable": pinned.adapter_executable,
        "adapter_profile": pinned.adapter_profile,
        "supported_minor": {
            "min_minor": offer.min_minor,
            "max_minor": offer.max_minor,
        },
        "limits_proposal": {
            "max_frame_bytes": proposal.max_frame_bytes,
            "queue_tx_bytes": proposal.queue_tx_bytes,
            "queue_rx_bytes": proposal.queue_rx_bytes,
            "max_control_pending": proposal.max_control_pending,
            "max_events_per_second": proposal.max_events_per_second,
            "window_seconds": proposal.window_seconds,
            "max_submits_in_flight": proposal.max_submits_in_flight,
            "max_choices_in_flight": proposal.max_choices_in_flight,
            "max_looks_in_flight": proposal.max_looks_in_flight,
            "drain_wait_ms": proposal.drain_wait_ms,
        },
        "latency_declarations": proposal.latency_declarations,
    }))
}
