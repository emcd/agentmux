//! Packing-unit identity, submission evidence, and the per-target delivery
//! outcome vocabulary.
//!
//! These describe transport-submission fallibility — what a transport was asked
//! to write, and what it can honestly claim about whether the write landed. They
//! are orthogonal to which side initiates delivery, which is why they survive
//! unchanged across the push/pull inversion.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// The unit of *target submission*, as distinct from the run of entries a
/// transport peeked. A peeked prefix is not one atomic target write: a Tmux
/// prefix splits into token-budgeted prompts injected separately, and Pty and ACP
/// do the analogous thing.
///
/// That partition is invisible to the relay, which holds custody of entries
/// without knowing how any transport renders them, so the transport names the
/// partition it chose when it declares a unit. The id lives in this neutral
/// boundary because both call directions name it: the relay mints and binds it,
/// and the transport quotes it back when acknowledging.
///
/// Assigned at declaration and never reassigned. A member's binding to one is
/// what the guard's evidence order reads to tell a provable `not_submitted` from
/// an honest `submission_unknown`, which is why the binding is recorded *before*
/// the first target-side effect rather than alongside it.
///
/// In practice the relay mints and a transport quotes back the id it was given.
/// That is a convention, not something the type enforces: [`mint`](Self::mint)
/// has to be public because any implementer outside this module must be able to
/// produce an id. Minting an id the ledger never issued binds nothing, so the
/// failure mode is a unit no member belongs to rather than a member bound to a
/// unit the guard does not know about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackingUnitId(u64);

static NEXT_PACKING_UNIT_ID: AtomicU64 = AtomicU64::new(1);

impl PackingUnitId {
    /// Mints the next id. Process-local and monotonic; identities never outlive
    /// the relay process, because the ledger holding them is in-memory.
    #[must_use]
    pub fn mint() -> Self {
        Self(NEXT_PACKING_UNIT_ID.fetch_add(1, Ordering::Relaxed))
    }

    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }
}

/// Typed evidence about whether a packing unit produced a target-side effect.
///
/// An undifferentiated error maps to [`SubmissionUnknown`](Self::SubmissionUnknown),
/// never [`NotSubmitted`](Self::NotSubmitted). Only a primitive that can prove
/// nothing was written may claim the latter: a Tmux paste is a body write
/// followed by an Enter, and a Pty unit is several `write_all` calls, so both can
/// fail *after* partial effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmissionEvidence {
    /// The target-side primitive positively reported success.
    Submitted,
    /// Positive evidence that no side effect occurred.
    NotSubmitted,
    /// Side effects cannot be excluded.
    SubmissionUnknown,
}

/// Why the relay refused to bind a proposed packing unit.
///
/// A refusal obliges the transport to produce **no target-side effect for that
/// proposed unit**. The reason is deliberately not enumerated per member: the
/// answer is the same whichever member vetoed, and a transport that could tell
/// them apart would be tempted to write the rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartitionError {
    /// At least one proposed member is no longer eligible to be bound — already
    /// terminal, already bound to another unit, or no longer admitted.
    MemberNotBindable,
    /// The relay could not reach its ledger. Treated exactly like the above by
    /// the transport: no effect for this unit.
    LedgerUnavailable,
}

/// Per-target delivery outcome for `send`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SendOutcome {
    Queued,
    Delivered,
    DroppedOnShutdown,
    Failed,
    /// Positive evidence that no target-side effect occurred: the member was
    /// never handed to a transport, or a primitive that can prove nothing was
    /// written reported so. Soundly asserts non-delivery, unlike
    /// [`SubmissionUnknown`](Self::SubmissionUnknown).
    NotSubmitted,
    /// A target-side effect cannot be excluded. Terminal, and deliberately not a
    /// failure spelling — not knowing is what actually happened, and reporting a
    /// failure would assert a non-delivery the relay cannot support. An
    /// undifferentiated error maps here rather than to
    /// [`NotSubmitted`](Self::NotSubmitted), because a Tmux paste (body write
    /// then Enter) and a Pty unit (several writes) can both fail after partial
    /// effect.
    SubmissionUnknown,
    /// A cross-relay (bang-path) target whose peer relay could not be reached or
    /// whose Hello handshake failed. Distinct from a local delivery `Failed` and
    /// from the `relay_unavailable` error code (which names *this* relay being
    /// unreachable to a client): this marks the *peer* relay as unreachable for
    /// that one forwarded target, so the requester's other targets still report
    /// their own outcomes.
    PeerUnavailable,
}

/// Payload handling mode for one async delivery task.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryPayloadMode {
    EnvelopeMessage,
    RawInput,
}
