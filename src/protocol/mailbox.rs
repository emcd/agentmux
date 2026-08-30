//! What a target's mailbox holds, and how positions in it are named.
//!
//! A mailbox is an ordered, per-target sequence of entries under relay custody.
//! Entries are numbered from one; the cursor names how far acknowledgment has
//! advanced. Those are separate types on purpose: every rule in the pull model
//! that can go wrong by one — a declaration must begin at *the cursor plus one* —
//! is stated in terms of both, and a single integer type would let the two be
//! confused at exactly the position where confusing them is a defect.

use std::sync::Arc;

use super::message::DeliveryEnvelope;

/// An entry's position in its target's mailbox.
///
/// Per-target and monotonic, beginning at one so that a cursor of zero can mean
/// "nothing acknowledged yet" without a sentinel value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntrySequence(u64);

impl EntrySequence {
    /// The first position any mailbox issues.
    #[must_use]
    pub fn first() -> Self {
        Self(1)
    }

    /// Builds a position from a raw value, rejecting zero.
    ///
    /// Zero is not a position: it is the cursor's "nothing acknowledged" value,
    /// and admitting it here would make that distinction representable in the
    /// wrong type.
    #[must_use]
    pub fn new(value: u64) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    /// The position immediately after this one.
    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }

    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }
}

/// How far a target's mailbox has been acknowledged.
///
/// Names the last entry an acknowledgment terminalized — not the next entry to
/// serve. [`next_sequence`](Self::next_sequence) is the only way to get from one
/// to the other, so the "cursor plus one" rule is expressed once rather than
/// re-derived at each call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct CursorPosition(u64);

impl CursorPosition {
    /// A mailbox that has acknowledged nothing.
    #[must_use]
    pub fn start() -> Self {
        Self(0)
    }

    /// The position a declaration must begin at.
    #[must_use]
    pub fn next_sequence(self) -> EntrySequence {
        EntrySequence(self.0 + 1)
    }

    /// The cursor left behind after acknowledging through `sequence`.
    #[must_use]
    pub fn advanced_through(sequence: EntrySequence) -> Self {
        Self(sequence.value())
    }

    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }
}

/// An inclusive run of mailbox positions.
///
/// Inclusive at both ends because every rule stated about a range names real
/// entries — "entries 1 through 5" is five entries, and a half-open spelling
/// would put an off-by-one between the requirement's language and the code that
/// enforces it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryRange {
    from: EntrySequence,
    through: EntrySequence,
}

impl EntryRange {
    /// Builds a range, rejecting one whose end precedes its start.
    #[must_use]
    pub fn new(from: EntrySequence, through: EntrySequence) -> Option<Self> {
        if through < from {
            None
        } else {
            Some(Self { from, through })
        }
    }

    #[must_use]
    pub fn from(self) -> EntrySequence {
        self.from
    }

    #[must_use]
    pub fn through(self) -> EntrySequence {
        self.through
    }

    /// How many positions the range spans.
    #[must_use]
    pub fn entries_count(self) -> u64 {
        self.through.value() - self.from.value() + 1
    }

    #[must_use]
    pub fn contains(self, sequence: EntrySequence) -> bool {
        self.from <= sequence && sequence <= self.through
    }

    /// The positions the range names, in order.
    pub fn sequences(self) -> impl Iterator<Item = EntrySequence> {
        (self.from.value()..=self.through.value()).map(EntrySequence)
    }
}

/// Which kind of entry occupies a mailbox position.
///
/// A projection of [`MailboxPayload`]'s variant, not a second source of truth:
/// the payload is what an entry *is*, and this is the discriminator a peek needs
/// in order to apply the raw-singleton rule without inspecting message content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailboxEntryKind {
    /// An envelope the receiving transport renders.
    Mail,
    /// Raw input written without envelope framing.
    Raw,
}

/// What a mailbox entry carries.
///
/// The two variants are the two things a transport can be asked to write, and
/// they differ in more than framing: mail is a structured envelope the transport
/// renders, while raw input is written through verbatim.
///
/// The envelope is shared rather than owned outright because a peek is specified
/// to be repeatable and is driven by a bounded poll: the same entry is read back
/// many times before it is ever acknowledged, and it is immutable from admission
/// onward, so each read shares the payload instead of copying it.
#[derive(Clone, Debug)]
pub enum MailboxPayload {
    /// An envelope to render and write.
    Mail(Arc<DeliveryEnvelope>),
    /// Raw input to write without envelope framing.
    Raw {
        content: String,
        /// Whether to submit (append Enter) after writing the content.
        append_enter: bool,
    },
}

impl MailboxPayload {
    #[must_use]
    pub fn kind(&self) -> MailboxEntryKind {
        match self {
            Self::Mail(_) => MailboxEntryKind::Mail,
            Self::Raw { .. } => MailboxEntryKind::Raw,
        }
    }

    /// Whether this payload ends a peeked run rather than joining it.
    ///
    /// Raw input is a barrier: it is written on its own so that it cannot be
    /// reordered against, or coalesced into, the mail around it.
    #[must_use]
    pub fn is_barrier(&self) -> bool {
        matches!(self, Self::Raw { .. })
    }
}

/// One entry under relay custody, at a known position in its target's mailbox.
///
/// Peeking yields copies: an entry stays in the mailbox until an acknowledgment
/// advances the cursor past it, so reading one must not consume it.
#[derive(Clone, Debug)]
pub struct MailboxEntry {
    /// This entry's position in its target's mailbox.
    pub sequence: EntrySequence,
    /// Correlation id, carried on the terminal-outcome receipt the sender sees.
    pub message_id: String,
    /// The admitted canonical payload size, in bytes.
    ///
    /// Held on the entry because a peek bound is evaluated against it, and the
    /// relay must be able to apply that bound without rendering anything.
    pub canonical_bytes: u64,
    /// What the transport is being asked to write.
    pub payload: MailboxPayload,
}

impl MailboxEntry {
    #[must_use]
    pub fn kind(&self) -> MailboxEntryKind {
        self.payload.kind()
    }

    #[must_use]
    pub fn is_barrier(&self) -> bool {
        self.payload.is_barrier()
    }
}
