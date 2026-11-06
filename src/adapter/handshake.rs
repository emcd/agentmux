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
    sync::{atomic::Ordering, mpsc::Receiver},
};

use serde_json::{Value, json};

use super::Adapter;
use super::ReadEvent;
use super::binding::{CallKind, Direction, PinnedLaunch, establish_binding};
use super::choice::ChoiceGate;
use super::frame::parse_frame;
use super::lifecycle::Liveness;

/// Decodes standard base64; `None` is not valid base64.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    STANDARD.decode(text).ok()
}

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
    // resume store of its own, so the value is validated as base64
    // and ignored. Ignoring resume contents does not make invalid
    // encoding well-formed: a non-base64 snapshot refuses the
    // binding.
    let snapshot_b64 = get_string(result, "snapshot_b64")?;
    if base64_decode(&snapshot_b64).is_none() {
        return Err(malformed("snapshot_b64 is not base64"));
    }
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
    for (field, value, proposed) in [
        (
            "max_control_pending",
            u64::from(negotiated.max_control_pending),
            u64::from(proposal.max_control_pending),
        ),
        (
            "max_events_per_second",
            u64::from(negotiated.max_events_per_second),
            u64::from(proposal.max_events_per_second),
        ),
        (
            "max_submits_in_flight",
            u64::from(negotiated.max_submits_in_flight),
            u64::from(proposal.max_submits_in_flight),
        ),
        (
            "max_choices_in_flight",
            u64::from(negotiated.max_choices_in_flight),
            u64::from(proposal.max_choices_in_flight),
        ),
        (
            "max_looks_in_flight",
            u64::from(negotiated.max_looks_in_flight),
            u64::from(proposal.max_looks_in_flight),
        ),
        (
            "queue_tx_bytes",
            negotiated.queue_tx_bytes,
            proposal.queue_tx_bytes,
        ),
        (
            "queue_rx_bytes",
            negotiated.queue_rx_bytes,
            proposal.queue_rx_bytes,
        ),
    ] {
        if value == 0 {
            return Err(invalid_limit(
                field,
                "zero cap is invalid, not unlimited",
            ));
        }
        if value > proposed {
            return Err(invalid_limit(
                field,
                "negotiated value exceeds the proposal",
            ));
        }
    }
    if negotiated.max_control_pending == 0
        || negotiated.max_control_pending > 1024
    {
        return Err(invalid_limit(
            "max_control_pending",
            "must lie within 1..=1024",
        ));
    }
    if negotiated.max_events_per_second == 0
        || negotiated.max_events_per_second > 10_000
    {
        return Err(invalid_limit(
            "max_events_per_second",
            "must lie within 1..=10000",
        ));
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

impl Adapter<'_> {
    pub(crate) fn handshake(
        &mut self,
        rx: &Receiver<ReadEvent>,
        start_ms: u64,
    ) -> Result<(), ()> {
        let pinned = PinnedLaunch {
            target_session: self.config.target_session.clone(),
            adapter_profile: self.config.profile.clone(),
            adapter_executable: self.config.executable.clone(),
        };
        let request =
            hello_request(&pinned, &self.config.offer, &self.config.proposal)
                .map_err(|_| ())?;
        let hello_wire =
            self.table.mint(CallKind::Handshake, None).map_err(|_| ())?;
        // The binding-establishing frame rides the control lane: it is
        // never queued behind data, so the hello request is always the
        // first wire frame under every schedule (same-lane FIFO from
        // an empty lane), and bootstrap traffic never consumes the
        // data budget it is about to negotiate.
        self.write_control(&json!({
            "jsonrpc": "2.0",
            "id": hello_wire.to_string(),
            "method": "adapter.handshakeHello",
            "params": request,
        }))
        .map_err(|_| ())?;
        let release_gate = |adapter: &mut Self| {
            if let Some(gate) = &adapter.gate_tx {
                let _ = gate.send(());
            }
        };
        let deadline =
            start_ms.saturating_add(self.config.handshake_timeout_ms);
        let envelope = loop {
            // Observe the writer while waiting for the hello result:
            // a dead transport fails the handshake now rather than
            // parking it until the handshake timeout. The wait is
            // sliced on the wall clock (not the virtual clock, which
            // replay may freeze) so a dead peer surfaces within
            // milliseconds while expiry still follows the virtual
            // deadline.
            self.drain_writer_events();
            if self.failed {
                return Err(());
            }
            if self.drain_expired() {
                self.record(
                    "drain_timeout",
                    "blocked write exceeded the drain wait",
                );
                return Err(());
            }
            // The deadline is checked on every iteration, independent
            // of receive outcome: a flood of parseable-but-wrong
            // frames is still silence from the binding peer, and a
            // matching result that arrives overdue is already
            // expired. Neither postpones the timeout.
            if self.clock.now_ms() >= deadline {
                return Err(());
            }
            match rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(ReadEvent::Frame(bytes)) => {
                    // Release the reader's RX queue accounting for
                    // every acquired frame, kept or discarded.
                    self.rx_queued.fetch_sub(
                        bytes
                            .len()
                            .min(self.rx_queued.load(Ordering::Relaxed)),
                        Ordering::AcqRel,
                    );
                    // A matching result that arrives overdue is
                    // already expired: recheck the deadline on
                    // receipt, not just on slices, so a wrong-id
                    // frame that starts a fresh 50ms slice cannot
                    // smuggle a late hello past the timeout.
                    if self.clock.now_ms() >= deadline {
                        return Err(());
                    }
                    let envelope = parse_frame(&bytes).map_err(|_| ())?;
                    if envelope.get("id").and_then(Value::as_str)
                        == Some(&hello_wire.to_string())
                    {
                        break envelope;
                    }
                    self.record(
                        "unexpected_pre_bind_frame",
                        "discarded before binding",
                    );
                    release_gate(self);
                },
                Ok(ReadEvent::Discarded { bound }) => {
                    // An oversize frame is refused without ending the
                    // handshake wait: record the typed framing failure
                    // and keep waiting for the hello result until the
                    // deadline or EOF.
                    self.record(
                        "oversize_frame",
                        format!(
                            "frame exceeded the inbound bound of {bound} bytes; \
                             discarded without reply"
                        )
                        .as_str(),
                    );
                    release_gate(self);
                },
                Ok(ReadEvent::Eof) | Ok(ReadEvent::Unusable) => {
                    return Err(());
                },
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if self.clock.now_ms() >= deadline {
                        return Err(());
                    }
                },
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(());
                },
            }
        };
        // The hello reply is a bootstrap envelope: it must carry the
        // version marker, must not carry a JSON-RPC error, and must
        // not carry both result and error.
        if envelope.get("jsonrpc") != Some(&Value::String("2.0".to_string())) {
            return Err(());
        }
        if envelope.get("error").is_some() {
            self.record(
                "hello_error_reply",
                "hello result must not carry an error",
            );
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
        self.frame_bound.store(
            result.negotiated.max_frame_bytes as usize,
            Ordering::Relaxed,
        );
        self.install_negotiated(&result.negotiated);
        let (binding, _) = establish_binding(
            &pinned,
            result.runtime_instance,
            result.adapter_generation,
        );
        self.table.retire(Direction::Outbound, hello_wire);
        self.binding = Some(binding);
        self.liveness = Some(Liveness::new(
            self.clock.now_ms(),
            self.config.liveness_timeout_ms,
        ));
        // The transition is ordered before post-hello acquisition:
        // installing first, opening the reader gate second, so no
        // coalesced first operation is ever read under the bootstrap
        // bound after negotiation.
        self.installed.store(true, Ordering::Release);
        release_gate(self);
        for event in self.config.script.events.clone() {
            if self.emit_lifecycle(&event.kind, &event.code).is_some() {
                return Err(());
            }
        }
        Ok(())
    }

    /// Installs the negotiated limits VERBATIM: the hello result
    /// carries already-resolved values, so no second application of
    /// the matched-pair formula. Direction arrows name the adapter's
    /// own perspective — supervisor→adapter is the adapter's RX
    /// (bounding inbound acquisition together with the frame cap),
    /// adapter→supervisor is the adapter's TX (bounding the outbox).
    fn install_negotiated(&mut self, negotiated: &NegotiatedLimits) {
        self.choice_gate =
            ChoiceGate::new(negotiated.max_choices_in_flight as usize);
        // Carry the outbox across the transition: the handshakeHello
        // request is still unacknowledged when the hello result lands,
        // and its completion arrives after the install. New caps,
        // preserved usage — no stranded bytes, no double-count.
        self.outbox.set_caps(
            negotiated.queue_tx_bytes as usize,
            negotiated.max_control_pending as usize,
        );
        self.read_bound.store(
            (negotiated.max_frame_bytes as usize)
                .min(negotiated.queue_rx_bytes as usize),
            Ordering::Relaxed,
        );
        self.rx_budget
            .store(negotiated.queue_rx_bytes as usize, Ordering::Relaxed);
        self.look_cap = negotiated.max_looks_in_flight as usize;
        self.ev_max = negotiated.max_events_per_second;
        self.ev_window_ms =
            u64::from(negotiated.window_seconds.saturating_mul(1000));
        self.ev_window_start = None;
        self.ev_count = 0;
        self.drain_wait_ms = negotiated.drain_wait_ms;
    }
}

/// Install accounting test: one test function by the inline-test
/// rule.
#[cfg(test)]
mod install_tests {
    use std::collections::HashMap;
    use std::sync::mpsc;

    use super::super::lifecycle::ManualClock;
    use super::super::script::parse_script;
    use super::super::writer::spawn_writer;
    use super::super::{Adapter, AdapterConfig};
    use super::{LimitsProposal, NegotiatedLimits, VersionOffer};

    fn test_config() -> AdapterConfig {
        let mut latency = HashMap::from([("default".to_string(), 5_000_u64)]);
        latency.insert("text".to_string(), 5_000);
        AdapterConfig {
            executable: "agentmux-adapter".to_string(),
            profile: "generic".to_string(),
            target_session: "seat-1".to_string(),
            offer: VersionOffer {
                major: 2,
                min_minor: 0,
                max_minor: 1,
            },
            proposal: LimitsProposal {
                max_frame_bytes: 65_536,
                queue_tx_bytes: 16_777_216,
                queue_rx_bytes: 16_777_216,
                max_control_pending: 32,
                max_events_per_second: 100,
                window_seconds: 1,
                max_submits_in_flight: 64,
                max_choices_in_flight: 16,
                max_looks_in_flight: 8,
                drain_wait_ms: 250,
                latency_declarations: latency,
            },
            script: parse_script("").expect("empty script parses"),
            handshake_timeout_ms: 30_000,
            liveness_timeout_ms: 60_000,
        }
    }

    /// Installing the negotiated limits carries the outbox across the
    /// transition: 500 unacknowledged hello-request bytes stay counted
    /// under the new 4096-byte cap, so a 3597-byte write is refused
    /// while a 3596-byte write fits, and the late completion releases
    /// exactly the carried bytes. A reset-to-empty install would admit
    /// the 3597-byte write and fail this test.
    #[test]
    fn install_preserves_outbox_usage() {
        let config = test_config();
        let clock = ManualClock::new(1_000);
        let (event_tx, event_rx) = mpsc::channel();
        let queues = spawn_writer(std::io::Cursor::new(Vec::new()), event_tx);
        let mut sink = Vec::new();
        let mut adapter =
            Adapter::new(&config, &clock, queues, event_rx, &mut sink);
        adapter
            .outbox
            .push_data(500)
            .expect("hello request enqueued");
        adapter.install_negotiated(&NegotiatedLimits {
            max_frame_bytes: 65_536,
            queue_tx_bytes: 4_096,
            queue_rx_bytes: 16_777_216,
            max_control_pending: 32,
            max_events_per_second: 100,
            window_seconds: 1,
            max_submits_in_flight: 64,
            max_choices_in_flight: 16,
            max_looks_in_flight: 8,
            drain_wait_ms: 250,
        });
        assert!(
            adapter.outbox.push_data(3_597).is_err(),
            "carried bytes must count against the new cap"
        );
        adapter.outbox.push_data(3_596).expect("remainder fits");
        // The late hello-request completion releases exactly the
        // carried bytes: no strand, no double-count.
        adapter.outbox.release_data(500);
        assert!(!adapter.outbox.is_empty());
        adapter.outbox.release_data(3_596);
        assert!(adapter.outbox.is_empty());
    }
}
