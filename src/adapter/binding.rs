//! Bootstrap establishment and post-bind call discipline.
//!
//! The hello call is keyed by (spawned-connection id + direction + wire
//! id) with locally-pinned launch inputs, never by an assigned binding;
//! the hello result establishes the captured binding triple atomically.
//! After binding, requests and notifications carry the full triple
//! (validated per message) while results and errors carry none, admitted
//! only by pending-table match on captured binding plus direction plus
//! wire id. Per-type cancel/cleanup terminalization lives in each method
//! module, coordinated through [`PendingTable`] teardown here.

use std::collections::HashMap;

/// Captured binding triple issued by the supervisor hello result.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Binding {
    pub runtime_instance: String,
    pub target_session: String,
    pub adapter_generation: u64,
}

/// Launch inputs pinned locally before the hello exchange. The hello
/// result is validated against these; the result itself carries no
/// target to compare.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedLaunch {
    pub target_session: String,
    pub adapter_profile: String,
    pub adapter_executable: String,
}

/// Call direction for pending-table keying. Direction spaces are
/// independent: an inbound id equal to an outbound id is unrelated,
// never a duplicate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    Outbound,
    Inbound,
}

/// Operation class of a pending call, driving per-type terminalization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallKind {
    Handshake,
    Submit,
    RequestChoice,
    Look,
    Persist,
    Ping,
}

/// One open call tracked by the pending table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingEntry {
    pub wire_id: u64,
    pub direction: Direction,
    pub correlation_id: Option<String>,
    pub kind: CallKind,
}

/// Pending-table failure: non-monotonic reuse or counter exhaustion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindingError {
    ReusedWireId { wire_id: u64 },
    WireIdExhausted,
}

impl std::fmt::Display for BindingError {
    fn fmt(
        &self,
        formatter: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        match self {
            Self::ReusedWireId { wire_id } => {
                write!(formatter, "wire id {wire_id} was already used")
            },
            Self::WireIdExhausted => {
                write!(formatter, "wire id counter exhausted; re-handshake")
            },
        }
    }
}

impl std::error::Error for BindingError {}

/// Pending-call table keyed by (direction, wire id), with
/// originator-monotonic minting from 1 per direction.
#[derive(Debug, Default)]
pub struct PendingTable {
    next_outbound: u64,
    next_inbound: u64,
    entries: HashMap<(Direction, u64), PendingEntry>,
}

impl PendingTable {
    /// Creates an empty table with both directions starting at wire id 1.
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_outbound: 1,
            next_inbound: 1,
            entries: HashMap::new(),
        }
    }

    /// Mints the next wire id for an originated call and opens its
    /// entry. Reaching `u64::MAX` is a binding error: close the binding
    /// and re-handshake.
    ///
    /// # Errors
    ///
    /// Returns [`BindingError::WireIdExhausted`] when the counter for
    /// the direction cannot advance.
    pub fn mint(
        &mut self,
        direction: Direction,
        kind: CallKind,
        correlation_id: Option<String>,
    ) -> Result<u64, BindingError> {
        let next = match direction {
            Direction::Outbound => &mut self.next_outbound,
            Direction::Inbound => &mut self.next_inbound,
        };
        if *next == u64::MAX {
            return Err(BindingError::WireIdExhausted);
        }
        let wire_id = *next;
        *next = next.saturating_add(1);
        self.entries.insert(
            (direction, wire_id),
            PendingEntry {
                wire_id,
                direction,
                correlation_id,
                kind,
            },
        );
        Ok(wire_id)
    }

    /// Admits a reply only by open-entry match. Unknown, retired, or
    /// superseded ids admit nothing; the caller discards them with a
    /// local diagnostic and never answers with a wire error.
    #[must_use]
    pub fn admit(
        &self,
        direction: Direction,
        wire_id: u64,
    ) -> Option<&PendingEntry> {
        self.entries.get(&(direction, wire_id))
    }

    /// Opens an inbound entry for a supervisor-originated call.
    /// A duplicate inbound id is a typed framing failure: the
    /// originator must never reuse a wire id.
    ///
    /// # Errors
    ///
    /// Returns [`BindingError::ReusedWireId`] when the id is already
    /// open in the inbound space.
    pub fn note_inbound(
        &mut self,
        wire_id: u64,
        kind: CallKind,
        correlation_id: Option<String>,
    ) -> Result<(), BindingError> {
        if self.entries.contains_key(&(Direction::Inbound, wire_id)) {
            return Err(BindingError::ReusedWireId { wire_id });
        }
        self.entries.insert(
            (Direction::Inbound, wire_id),
            PendingEntry {
                wire_id,
                direction: Direction::Inbound,
                correlation_id,
                kind,
            },
        );
        Ok(())
    }

    /// Retires an entry after its terminal outcome is recorded.
    pub fn retire(
        &mut self,
        direction: Direction,
        wire_id: u64,
    ) -> Option<PendingEntry> {
        self.entries.remove(&(direction, wire_id))
    }

    /// Clears all entries for disconnect, fence, generation replacement,
    /// or binding refusal, returning them for per-type terminalization.
    /// Wire ids are never reused across the cleanup: counters keep
    /// their values.
    pub fn clear(&mut self) -> Vec<PendingEntry> {
        self.entries.drain().map(|(_, entry)| entry).collect()
    }

    /// Counts open entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Reports an empty table.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Establishes the captured binding from validated hello-result fields
/// against the pinned launch context. The result carries no target, so
/// identity comes from the pinned inputs, not from comparing a field
/// the result never sends.
#[must_use]
pub fn establish_binding(
    pinned: &PinnedLaunch,
    runtime_instance: String,
    adapter_generation: u64,
) -> (Binding, PinnedLaunch) {
    let binding = Binding {
        runtime_instance,
        target_session: pinned.target_session.clone(),
        adapter_generation,
    };
    (binding, pinned.clone())
}
