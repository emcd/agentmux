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
    Live,
    Cache,
    Synthetic,
    None,
}

impl Freshness {
    fn as_wire(&self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Cache => "cache",
            Self::Synthetic => "synthetic",
            Self::None => "none",
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
        "entries_total": rendered.len(),
        "returned_entries_count": rendered.len(),
        "freshness": snapshot.freshness.as_wire(),
        "snapshot_source": snapshot.source.as_wire(),
        "stale": snapshot.stale || snapshot.freshness == Freshness::Synthetic,
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
        freshness: Freshness::Synthetic,
        source: SnapshotSource::Synthetic,
        stale: true,
        discontinuity: true,
        stale_reason_code: Some("fence_terminalized".to_string()),
    })
}
