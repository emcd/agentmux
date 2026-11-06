//! `relay.look` poll responses.
//!
//! Snapshot entries with full freshness metadata from the generation
//! the snapshot was read from, adapter-side prime-wait bounded strictly
//! by the relay-supplied timeout (zero means no wait), and synthetic
//! stale-empty snapshots for fence terminalization. Polls proceed
//! concurrently within `max_looks_in_flight`, never blocked on a turn.

use serde_json::{Value, json};

/// Snapshot freshness as carried on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    Stale,
    Initializing,
}

impl Freshness {
    fn as_wire(&self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Initializing => "initializing",
        }
    }
}

/// Snapshot source as carried on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotSource {
    Live,
    Cache,
    Synthetic,
    None,
}

impl SnapshotSource {
    fn as_wire(&self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Cache => "cache",
            Self::Synthetic => "synthetic",
            Self::None => "none",
        }
    }
}

/// One snapshot entry: base64 text with closed-kind metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotEntry {
    pub kind: String,
    pub seq: u64,
    pub text_base64: String,
    pub truncated: bool,
}

/// Adapter-side prime-wait: bounded strictly by the relay-supplied
/// timeout, evaluated against explicit clock readings (never ambient
/// time). A zero timeout means no wait: the first poll answers stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrimeWait {
    expires_at_ms: Option<u64>,
}

impl PrimeWait {
    /// Starts a prime-wait at `now_ms` lasting `timeout_ms`. Zero
    /// means no wait.
    #[must_use]
    pub fn new(now_ms: u64, timeout_ms: u64) -> Self {
        if timeout_ms == 0 {
            Self {
                expires_at_ms: None,
            }
        } else {
            Self {
                expires_at_ms: Some(now_ms.saturating_add(timeout_ms)),
            }
        }
    }

    /// Reports whether the wait is over at `now_ms`. A zero-timeout
    /// wait is always over.
    #[must_use]
    pub fn is_expired(&self, now_ms: u64) -> bool {
        match self.expires_at_ms {
            None => true,
            Some(expires_at_ms) => now_ms >= expires_at_ms,
        }
    }
}

/// A look snapshot with full freshness metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub correlation_id: String,
    pub adapter_generation: u64,
    pub entries: Vec<SnapshotEntry>,
    pub freshness: Freshness,
    pub source: SnapshotSource,
    pub stale: bool,
    pub discontinuity: bool,
    pub stale_reason_code: Option<String>,
    /// Total entries available before windowing, when the returned
    /// entries are a lines/offset slice. None means the returned
    /// entries are the whole snapshot.
    pub entries_total_override: Option<u64>,
}

/// Renders a look result value with full freshness metadata. Synthetic
/// snapshots always carry stale=true, never fresh.
#[must_use]
pub fn snapshot_result(snapshot: &Snapshot) -> Value {
    let rendered: Vec<Value> = snapshot
        .entries
        .iter()
        .map(|entry| {
            json!({
                "kind": entry.kind,
                "seq": entry.seq,
                "text_b64": entry.text_base64,
                "truncated": entry.truncated,
            })
        })
        .collect();
    let mut result = json!({
        "correlation_id": snapshot.correlation_id,
        "adapter_generation": snapshot.adapter_generation,
        "entries": rendered,
        "entries_total": snapshot.entries_total_override.unwrap_or(rendered.len() as u64),
        "returned_entries_count": rendered.len(),
        "freshness": snapshot.freshness.as_wire(),
        "snapshot_source": snapshot.source.as_wire(),
        "stale": snapshot.stale,
        "discontinuity": snapshot.discontinuity,
    });
    if let Some(code) = &snapshot.stale_reason_code {
        result["stale_reason_code"] = Value::String(code.clone());
    }
    result
}

/// Renders the fence-terminalized look outcome: a synthetic stale
/// EMPTY snapshot with zero counts from the fencing generation.
#[must_use]
pub fn fence_snapshot(correlation_id: &str, adapter_generation: u64) -> Value {
    snapshot_result(&Snapshot {
        correlation_id: correlation_id.to_string(),
        adapter_generation,
        entries: Vec::new(),
        freshness: Freshness::Stale,
        source: SnapshotSource::Synthetic,
        stale: true,
        discontinuity: true,
        stale_reason_code: Some("fence_terminalized".to_string()),
        entries_total_override: None,
    })
}

use super::Adapter;
use super::PendingLook;
use super::binding::{CallKind, Direction};
use super::frame::contract_error;

pub(crate) fn base64_encode(text: &str) -> String {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    STANDARD.encode(text.as_bytes())
}

impl Adapter<'_> {
    pub(crate) fn handle_look(&mut self, envelope: &Value) -> Option<i32> {
        let Some(params) = envelope.get("params") else {
            self.answer_invalid(envelope.get("id"));
            return None;
        };
        if !self.check_binding(params) {
            self.answer_binding_mismatch(
                envelope.get("id"),
                envelope.get("params"),
            );
            return None;
        }
        let (Some(wire), Some(correlation)) =
            (self.wire_id_of(envelope), self.correlation_of(params))
        else {
            self.answer_invalid(envelope.get("id"));
            return None;
        };
        if self
            .table
            .note_inbound(wire, CallKind::Look, Some(correlation.clone()))
            .is_err()
        {
            self.answer_binding_mismatch(
                envelope.get("id"),
                envelope.get("params"),
            );
            return None;
        }
        // Look parameter validation: the handle is a present string
        // and a stable logical handle; lines and offset are validated
        // against their own ranges (u32 and u64); the prime timeout is
        // a required u64. Each present-with-wrong-type or
        // out-of-range field is a structural refusal.
        let handle = params
            .get("look_handle_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let lines = match params.get("lines") {
            None => None,
            Some(value) => match value.as_u64() {
                Some(lines) if u32::try_from(lines).is_ok() => Some(lines),
                _ => {
                    self.answer_structural(
                        envelope.get("id"),
                        &correlation,
                        "look lines must lie within u32",
                    );
                    self.table.retire(Direction::Inbound, wire);
                    return None;
                },
            },
        };
        let offset = match params.get("offset") {
            None => None,
            Some(value) => match value.as_u64() {
                Some(offset) => Some(offset),
                None => {
                    self.answer_structural(
                        envelope.get("id"),
                        &correlation,
                        "look offset must lie within u64",
                    );
                    self.table.retire(Direction::Inbound, wire);
                    return None;
                },
            },
        };
        let prime_timeout_ms =
            match params.get("prime_timeout_ms").and_then(Value::as_u64) {
                Some(timeout) => timeout,
                None => {
                    self.answer_structural(
                        envelope.get("id"),
                        &correlation,
                        "look prime_timeout_ms is required",
                    );
                    self.table.retire(Direction::Inbound, wire);
                    return None;
                },
            };
        let Some(handle) = handle else {
            self.answer_structural(
                envelope.get("id"),
                &correlation,
                "look_handle_id is required",
            );
            self.table.retire(Direction::Inbound, wire);
            return None;
        };
        // Admitted: binding plus every required look param validated.
        // Only now does the poll prove a live peer.
        self.observe_liveness();
        let offset = offset.unwrap_or(0);
        // The negotiated in-flight cap bounds concurrently HELD
        // prime-waits: immediate answers retire at once and never
        // occupy it, but a wait held past the cap is refused with a
        // typed concurrency error rather than queued unboundedly.
        if self.pending_looks.len() >= self.look_cap {
            let _ = self.write_value(&contract_error(
                envelope.get("id"),
                "look_concurrency_exceeded",
                "too many concurrent look prime-waits",
                None,
                Some(&correlation),
            ));
            self.table.retire(Direction::Inbound, wire);
            return None;
        }
        let target = params
            .get("target_session")
            .and_then(Value::as_str)
            .unwrap_or("");
        let generation = self
            .binding
            .as_ref()
            .map(|binding| binding.adapter_generation)
            .unwrap_or(0);
        let now = self.clock.now_ms();
        // The anchor is keyed by the stable logical handle and set at
        // the first poll: the handle survives generation replacement
        // by rebinding, so the anchor lives with the handle.
        let anchor = *self.prime_anchors.entry(handle.clone()).or_insert(now);
        if self.config.script.snapshot_for(target).is_none() {
            // Unscripted targets never become available: answer
            // synthetic-stale immediately rather than holding a wait
            // that could only expire.
            let _ = self.write_value(&json!({
                "jsonrpc": "2.0",
                "id": wire.to_string(),
                "result": snapshot_result(&Snapshot {
                    correlation_id: correlation.clone(),
                    adapter_generation: generation,
                    entries: Vec::new(),
                    freshness: Freshness::Stale,
                    source: SnapshotSource::Synthetic,
                    stale: true,
                    discontinuity: false,
                    stale_reason_code: Some(
                        "no_snapshot_scripted".to_string(),
                    ),
                    entries_total_override: None,
                }),
            }));
            self.table.retire(Direction::Inbound, wire);
            return None;
        }
        if self.snapshot_available(target, anchor, now) {
            self.answer_fresh_look(&PendingLook {
                wire,
                correlation: correlation.clone(),
                target: target.to_string(),
                handle,
                deadline_ms: now,
                lines,
                offset,
            });
            self.table.retire(Direction::Inbound, wire);
            return None;
        }
        if prime_timeout_ms == 0 {
            self.answer_priming_look(&PendingLook {
                wire,
                correlation: correlation.clone(),
                target: target.to_string(),
                handle,
                deadline_ms: now,
                lines,
                offset,
            });
            self.table.retire(Direction::Inbound, wire);
            return None;
        }
        // A positive timeout holds the look open: the run loop keeps
        // dispatching fence, cancel, ack, and choice traffic while the
        // wait is pending, then answers fresh or stale-tagged.
        self.pending_looks.push(PendingLook {
            wire,
            correlation,
            target: target.to_string(),
            handle,
            deadline_ms: now.saturating_add(prime_timeout_ms),
            lines,
            offset,
        });
        None
    }

    /// Answers one look with its scripted snapshot rendered fresh,
    /// applying the requested lines/offset window as a tail-N slice
    /// (mirroring the relay's slicing of the same frozen params):
    /// entries_total counts the whole snapshot,
    /// returned_entries_count counts the window, an offset past the
    /// start yields an empty window, and scripted per-entry flags are
    /// preserved verbatim — a cut is expressed through the counts,
    /// never by rewriting entry content.
    pub(crate) fn answer_fresh_look(&mut self, look: &PendingLook) {
        let generation = self
            .binding
            .as_ref()
            .map(|binding| binding.adapter_generation)
            .unwrap_or(0);
        let scripted: Vec<SnapshotEntry> = self
            .config
            .script
            .snapshot_for(&look.target)
            .map(|scripted| {
                scripted
                    .entries
                    .iter()
                    .map(|entry| SnapshotEntry {
                        kind: entry.kind.clone(),
                        seq: entry.seq,
                        text_base64: base64_encode(&entry.text),
                        truncated: entry.truncated,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let total = scripted.len() as u64;
        // Tail-N window, mirroring the relay's own slicing of the
        // same frozen params (src/acp/state.rs): skip `offset`
        // entries from the newest end, then take the last `lines` of
        // what remains. An offset past the start yields an empty
        // window without underflow; absent lines returns everything
        // up to the window end.
        let window_end = scripted.len().saturating_sub(look.offset as usize);
        let window_start = match look.lines {
            Some(lines) => window_end.saturating_sub(lines as usize),
            None => 0,
        };
        let windowed: Vec<SnapshotEntry> =
            scripted[window_start..window_end].to_vec();
        // Scripted per-entry flags are preserved verbatim: a window
        // cut is expressed through entries_total and
        // returned_entries_count, never by rewriting entry content.
        let _ = self.write_value(&json!({
            "jsonrpc": "2.0",
            "id": look.wire.to_string(),
            "result": snapshot_result(&Snapshot {
                correlation_id: look.correlation.clone(),
                adapter_generation: generation,
                entries: windowed,
                freshness: Freshness::Fresh,
                source: SnapshotSource::Live,
                stale: false,
                discontinuity: false,
                stale_reason_code: None,
                entries_total_override: Some(total),
            }),
        }));
    }

    /// Answers one look stale-tagged while its snapshot is priming.
    pub(crate) fn answer_priming_look(&mut self, look: &PendingLook) {
        let generation = self
            .binding
            .as_ref()
            .map(|binding| binding.adapter_generation)
            .unwrap_or(0);
        let _ = self.write_value(&json!({
            "jsonrpc": "2.0",
            "id": look.wire.to_string(),
            "result": snapshot_result(&Snapshot {
                correlation_id: look.correlation.clone(),
                adapter_generation: generation,
                entries: Vec::new(),
                freshness: Freshness::Stale,
                source: SnapshotSource::Synthetic,
                stale: true,
                discontinuity: false,
                stale_reason_code: Some("priming".to_string()),
                entries_total_override: None,
            }),
        }));
    }
}
