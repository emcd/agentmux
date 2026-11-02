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

impl VersionOffer {
    /// The protocol version this reference adapter implements.
    #[must_use]
    pub fn current() -> Self {
        Self {
            major: 2,
            min_minor: 0,
            max_minor: 0,
        }
    }
}

impl LimitsProposal {
    /// Contract table NEGOTIATED-WITH-DEFAULTS values. Latency
    /// declarations come from the deployment and are never defaulted.
    #[must_use]
    pub fn negotiation_defaults(
        latency_declarations: HashMap<String, u64>,
    ) -> Self {
        Self {
            max_frame_bytes: 1_048_576,
            queue_tx_bytes: 16_777_216,
            queue_rx_bytes: 16_777_216,
            max_control_pending: 32,
            max_events_per_second: 100,
            window_seconds: 1,
            max_submits_in_flight: 64,
            max_choices_in_flight: 16,
            max_looks_in_flight: 8,
            drain_wait_ms: 250,
            latency_declarations,
        }
    }
}
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

/// Validated hello result: assigned binding inputs, selected
/// versions, and post-resolution limits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelloResult {
    pub runtime_instance: String,
    pub adapter_generation: u64,
    pub selected_major: u32,
    pub selected_minor: u16,
    pub negotiated: NegotiatedLimits,
}
fn malformed(reason: impl Into<String>) -> HandshakeError {
    HandshakeError::MalformedResult {
        reason: reason.into(),
    }
}

fn get_u64(result: &Value, field: &str) -> Result<u64, HandshakeError> {
    result
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| malformed(format!("missing numeric {field}")))
}

fn get_string(result: &Value, field: &str) -> Result<String, HandshakeError> {
    result
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| malformed(format!("missing string {field}")))
}

fn get_u32(result: &Value, field: &str) -> Result<u32, HandshakeError> {
    let value = get_u64(result, field)?;
    u32::try_from(value)
        .map_err(|_| malformed(format!("{field} out of range")))
}

/// Parses and shape-checks a hello result object (the `result` member
/// of the supervisor's reply). The result carries no target and no
/// binding triple: identity comes from the pinned launch context, and
/// versions/limits are validated separately against the offer.
///
/// # Errors
///
/// Returns [`HandshakeError::MalformedResult`] for a missing or
/// misshapen field.
pub fn parse_hello_result(
    result: &Value,
) -> Result<HelloResult, HandshakeError> {
    let selected_major = get_u32(result, "selected_major")?;
    let minor = get_u64(result, "selected_minor")?;
    let selected_minor = u16::try_from(minor)
        .map_err(|_| malformed("selected_minor out of range"))?;
    // snapshot_b64 is required (empty string when none); v1 records no
    // resume store of its own, so the value is validated and ignored.
    get_string(result, "snapshot_b64")?;
    let limits = result
        .get("negotiated_limits")
        .filter(|limits| limits.is_object())
        .ok_or_else(|| malformed("missing negotiated_limits"))?;
    let get = |field: &str| get_u64(limits, field);
    let get32 = |field: &str| {
        get(field).and_then(|value| {
            u32::try_from(value)
                .map_err(|_| malformed(format!("{field} out of range")))
        })
    };
    Ok(HelloResult {
        runtime_instance: get_string(result, "runtime_instance")?,
        adapter_generation: get_u64(result, "adapter_generation")?,
        selected_major,
        selected_minor,
        negotiated: NegotiatedLimits {
            max_frame_bytes: get("max_frame_bytes")?,
            queue_tx_bytes: get("queue_tx_bytes")?,
            queue_rx_bytes: get("queue_rx_bytes")?,
            max_control_pending: get32("max_control_pending")?,
            max_events_per_second: get32("max_events_per_second")?,
            window_seconds: get32("window_seconds")?,
            max_submits_in_flight: get32("max_submits_in_flight")?,
            max_choices_in_flight: get32("max_choices_in_flight")?,
            max_looks_in_flight: get32("max_looks_in_flight")?,
            drain_wait_ms: get("drain_wait_ms")?,
        },
    })
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

use std::io::BufRead;

use super::Adapter;
use super::binding::{CallKind, Direction, establish_binding};
use super::frame::{BOOTSTRAP_MAX_FRAME_BYTES, parse_frame, read_frame};
use super::lifecycle::Liveness;

impl Adapter<'_> {
    pub(crate) fn handshake<R: BufRead>(
        &mut self,
        reader: &mut R,
    ) -> Result<(), ()> {
        let pinned = PinnedLaunch {
            target_session: self.config.target_session.clone(),
            adapter_profile: self.config.profile.clone(),
            adapter_executable: self.config.executable.clone(),
        };
        let request =
            hello_request(&pinned, &self.config.offer, &self.config.proposal)
                .map_err(|_| ())?;
        let hello_wire = self
            .table
            .mint(Direction::Outbound, CallKind::Handshake, None)
            .map_err(|_| ())?;
        self.write_value(&json!({
            "jsonrpc": "2.0",
            "id": hello_wire.to_string(),
            "method": "adapter.handshakeHello",
            "params": request,
        }))
        .map_err(|_| ())?;
        let bytes = match read_frame(reader, BOOTSTRAP_MAX_FRAME_BYTES) {
            Ok(Some(bytes)) => bytes,
            _ => return Err(()),
        };
        let envelope = parse_frame(&bytes).map_err(|_| ())?;
        if envelope.get("id").and_then(Value::as_str)
            != Some(&hello_wire.to_string())
        {
            return Err(());
        }
        let result = envelope
            .get("result")
            .ok_or(())
            .and_then(|result| parse_hello_result(result).map_err(|_| ()))?;
        validate_selection(
            &self.config.offer,
            result.selected_major,
            result.selected_minor,
        )
        .map_err(|_| ())?;
        validate_limits(&self.config.proposal, &result.negotiated)
            .map_err(|_| ())?;
        self.selected_major = result.selected_major;
        self.selected_minor = result.selected_minor;
        self.inbound_bound = result.negotiated.max_frame_bytes as usize;
        let (binding, _) = establish_binding(
            &pinned,
            result.runtime_instance,
            result.adapter_generation,
        );
        self.table.retire(Direction::Outbound, hello_wire);
        self.binding = Some(binding);
        self.liveness = Some(Liveness::new(self.clock.now_ms(), 60_000));
        for event in &self.config.script.events.clone() {
            self.emit_lifecycle(&event.kind, &event.code);
        }
        Ok(())
    }
}
