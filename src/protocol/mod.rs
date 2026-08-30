//! The delivery protocol boundary: the vocabulary both delivery call directions
//! depend on, and neither one owns.
//!
//! Delivery runs in two directions. The relay calls into a transport to read a
//! target (`look`). A transport calls into the relay to consume a target's
//! mailbox (peek, declare, acknowledge). Both directions need to name the same
//! things — what an entry is, where it sits, who may consume it, what a write
//! proved — and neither may reach into the other to do it.
//!
//! That is the whole reason this module sits at the crate root rather than inside
//! `transports`. It is below both sides in the dependency order, so it can be
//! depended on from either without a cycle, and the direction of every dependency
//! it participates in is inward.
//!
//! **This module imports nothing from `crate::relay`, `crate::acp`,
//! `crate::tmux`, `crate::pty`, or `crate::transports`.** The rule is mechanical
//! rather than aspirational: `scripts/lint-delivery-protocol-boundary.py` fails
//! the commit that reintroduces such an import. A back-edge here would not merely
//! be untidy — it would mean one call direction had come to need the other's
//! concrete types to express its own contract, which is the coupling the split
//! exists to prevent. If either side ever cannot say what it means without the
//! other, the inversion has not happened.
//!
//! What stays outside, deliberately: transport construction and startup, the
//! `Transport` trait and its dispatch enum, generation fencing, and every relay
//! error type. Those describe *how* a side does its work, and each belongs to one
//! side alone.
//!
//! This is not the relay's client-facing wire contract. That is a different
//! protocol, spoken to CLI, MCP and TUI clients over a socket, and it lives in
//! `crate::relay::contract`.

pub mod delivery;
pub mod doorbell;
pub mod identity;
pub mod look;
pub mod mailbox;
pub mod message;
pub mod operations;
pub mod worker;

pub use delivery::{
    DeliveryPayloadMode, PackingUnitId, PartitionError, SendOutcome, SubmissionEvidence,
};
pub use doorbell::DeliveryDoorbell;
pub use identity::{ConsumerBinding, ConsumerGenerationId, DeliveryTargetId};
pub use look::{
    LookFreshness, LookSnapshotPayload, LookSnapshotSource, StructuredEntry, ToolCallStatus,
};
pub use mailbox::{
    CursorPosition, EntryRange, EntrySequence, MailboxEntry, MailboxEntryKind, MailboxPayload,
};
pub use message::{DeliveryEnvelope, DeliveryMessage};
pub use operations::{
    AckAccepted, AckRejection, AckRequest, AckResult, DeclareAccepted, DeclareRejection,
    DeclareRequest, DeclareResult, MemberAcknowledgment, PeekRejection, PeekRequest, PeekResponse,
    PeekResult,
};
pub use worker::{WorkerFailureReason, WorkerReadinessState};
