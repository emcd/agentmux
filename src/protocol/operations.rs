//! The request and response shapes for the three mailbox operations a delivery
//! executor calls: peek, declare, acknowledge.
//!
//! Only the shapes live here. What the relay does when it receives one — the
//! validation, the ledger mutation, the terminal transition — is relay-owned and
//! deliberately absent, because a transport must be able to state a call without
//! depending on the machinery that answers it.
//!
//! Every operation carries a [`ConsumerBinding`] rather than a bare target,
//! because every one of them is checked against the target's active generation
//! before it takes effect.

use super::delivery::{PackingUnitId, SubmissionEvidence};
use super::identity::ConsumerBinding;
use super::mailbox::{CursorPosition, EntryRange, EntrySequence, MailboxEntry};

/// Reads the head of a target's mailbox without advancing anything.
///
/// Both bounds are stated in units the relay can evaluate without rendering.
/// A token budget is deliberately absent: tokens are a property of an entry as a
/// *specific* transport would render it, so a bound expressed in them would ask
/// the relay to know something only the caller knows.
#[derive(Clone, Debug)]
pub struct PeekRequest {
    pub binding: ConsumerBinding,
    /// The most entries to return.
    pub entry_max: usize,
    /// The most canonical payload bytes to return, summed across entries.
    pub canonical_bytes_max: u64,
}

/// The head run of a target's mailbox, as it stood when the peek was answered.
#[derive(Clone, Debug)]
pub struct PeekResponse {
    /// The contiguous run at the head, in mailbox order. Empty when the mailbox
    /// holds nothing beyond the cursor, or when the head entry alone exceeds the
    /// requested bounds.
    pub entries: Vec<MailboxEntry>,
    /// The cursor at the time of the read, so a caller can name the position its
    /// declaration must begin at without a second call.
    pub cursor: CursorPosition,
}

/// Why a peek returned nothing at all rather than an empty run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeekRejection {
    /// The binding does not name the target's active generation.
    ///
    /// Covers a generation that has been superseded and a target no consumer
    /// holds at all, and does not distinguish them: in neither case does the
    /// caller own the target, and there is nothing it could do differently
    /// knowing which it was.
    GenerationSuperseded,
    /// The relay holds no mailbox for the named target.
    UnknownTarget,
}

pub type PeekResult = Result<PeekResponse, PeekRejection>;

/// Records, before any write is attempted, the exact run of entries a delivery
/// executor is about to submit as one packing unit.
///
/// The range names both ends rather than only its last position. A well-formed
/// call always begins at the cursor plus one, but the relay validates that rather
/// than assuming it: a request that cannot express a wrong start cannot be
/// rejected for having one, and the rule that declarations begin at the cursor
/// would then be a convention no code enforces.
#[derive(Clone, Debug)]
pub struct DeclareRequest {
    pub binding: ConsumerBinding,
    /// The run being declared, inclusive of both ends.
    pub range: EntryRange,
}

/// A declaration the relay accepted and bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeclareAccepted {
    /// The identity the declared entries are now bound to, and the identity a
    /// later acknowledgment names.
    pub unit: PackingUnitId,
    /// The run the unit covers, echoed back as the relay recorded it.
    pub range: EntryRange,
}

/// Why a declaration bound nothing.
///
/// Every variant leaves the named entries queued and undeclared, and leaves any
/// declaration already outstanding untouched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclareRejection {
    /// The binding does not name the target's active generation.
    ///
    /// Covers a generation that has been superseded and a target no consumer
    /// holds at all, and does not distinguish them: in neither case does the
    /// caller own the target, and there is nothing it could do differently
    /// knowing which it was.
    GenerationSuperseded,
    /// The relay holds no mailbox for the named target.
    UnknownTarget,
    /// The range does not begin at the cursor plus one.
    NotAtCursor {
        expected: EntrySequence,
        requested: EntrySequence,
    },
    /// The mailbox does not hold every position the range names.
    NotContiguous { absent: EntrySequence },
    /// The range extends past the highest position the mailbox holds.
    PastMailboxEnd {
        /// The highest position held, or `None` for an empty mailbox.
        highest: Option<EntrySequence>,
        requested: EntrySequence,
    },
    /// The target already has a declared unit that has not been acknowledged.
    ///
    /// Rejected regardless of which range the new call names: one declared unit
    /// must be fully resolved before another is declared, so that two units can
    /// never bind the same entry and give one member two guards.
    UnitAlreadyOutstanding { outstanding: PackingUnitId },
}

pub type DeclareResult = Result<DeclareAccepted, DeclareRejection>;

/// What a delivery executor observed about one member of a declared unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberAcknowledgment {
    pub sequence: EntrySequence,
    pub evidence: SubmissionEvidence,
}

/// Terminalizes the entries a prior declaration bound, from what the executor
/// observed writing them.
///
/// The unit is named; the range is not. The relay looks the range up from its own
/// record of the declaration rather than trusting an endpoint the caller supplies,
/// which is what keeps the acknowledged run identical to the run declared before
/// any write began.
#[derive(Clone, Debug)]
pub struct AckRequest {
    pub binding: ConsumerBinding,
    /// The unit a prior declaration returned.
    pub unit: PackingUnitId,
    /// Per-member evidence, one entry per position the unit covers.
    pub members: Vec<MemberAcknowledgment>,
}

/// What an accepted acknowledgment did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AckAccepted {
    /// The unit's entries terminalized on this call, and the cursor advanced.
    Terminalized {
        range: EntryRange,
        cursor: CursorPosition,
    },
    /// The unit had already been acknowledged. A repeat is a no-op rather than an
    /// error: it names a real binding that has already resolved, so there is
    /// nothing to reject and nothing left to do.
    AlreadyTerminalized { range: EntryRange },
}

/// Why an acknowledgment terminalized nothing and advanced no cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AckRejection {
    /// The binding does not name the target's active generation.
    ///
    /// Covers a generation that has been superseded and a target no consumer
    /// holds at all, and does not distinguish them: in neither case does the
    /// caller own the target, and there is nothing it could do differently
    /// knowing which it was.
    GenerationSuperseded,
    /// The relay holds no mailbox for the named target.
    UnknownTarget,
    /// No declaration under this caller's generation ever bound the named unit.
    ///
    /// Covers a unit that was never declared and one declared under a superseded
    /// generation alike: neither leaves a binding the current generation may
    /// resolve, and the relay does not distinguish them, because the caller may
    /// do nothing differently in either case.
    UnitNotDeclared,
    /// The supplied evidence is not one entry per position the unit covers.
    ///
    /// Rejected rather than repaired. An acknowledgment reports what a write
    /// observed for each member, so a missing member has no evidence to be
    /// resolved from and a surplus or repeated one names a member this unit does
    /// not answer for. Filling a gap from a sibling's report would terminalize a
    /// member with an outcome nothing observed for it — the invented evidence
    /// this contract exists to make impossible.
    EvidenceDoesNotCoverUnit {
        /// The range the unit was declared over, which the evidence must match
        /// exactly, once per position.
        expected: EntryRange,
    },
}

pub type AckResult = Result<AckAccepted, AckRejection>;
