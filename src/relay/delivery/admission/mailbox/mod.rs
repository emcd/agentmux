//! The per-target mailbox and the three operations a delivery-loop executor
//! calls against it: peek, declare, acknowledge.
//!
//! The relay holds custody of every entry until it is acknowledged. Nothing here
//! hands an entry to anyone: a peek yields copies, a declaration records what is
//! about to be written, and only an acknowledgment resolves what an executor
//! asked for. That is the whole of the pull model's relay side — an executor asks
//! what is there, says what it is about to do, does it, and reports what
//! happened.
//!
//! **Of the three, only acknowledgment advances the cursor or terminalizes
//! anything.** Peek and declare are deliberately without effect on either, so an
//! executor that peeks and then dies, or declares and then dies, leaves the
//! mailbox exactly where a watchdog or a replacement can still reason about it.
//!
//! The cursor does move outside these three, and the distinction is worth keeping
//! straight: a position retired because its reservation was rolled back, or
//! because a lifecycle trigger resolved its entry, is one the target will never
//! serve, and the cursor advances over it. Those paths resolve entries the
//! executor never asked about. What no executor-driven operation short of an
//! acknowledgment does is move the cursor over an entry that is still waiting to
//! be written.
//!
//! Every operation here takes the ledger lock once at its head and holds it for
//! the whole operation, as every path in this subsystem does.
//!
//! Split one module per operation, because each carries the reasoning for its
//! own refusals and those are what most of the text is:
//!
//! - [`addressing`] — naming the target an operation acts on.
//! - [`generation`] — issuing the generation entitled to consume a target, and
//!   replacing it behind a fence.
//! - [`doorbell`] — the notify-only hint that a peek is worth making.
//! - [`enqueue`] — placing an admitted entry into its mailbox.
//! - [`peek`] — reading the head run without advancing anything.
//! - [`declare`] — recording the run about to be submitted as one packing unit.
//! - [`ack`] — terminalizing exactly that run from the executor's evidence.
//! - [`reap`] — giving the target up and reclaiming its mailbox when its
//!   registration goes away.

mod ack;
mod addressing;
mod declare;
mod doorbell;
mod enqueue;
#[cfg(test)]
mod fixtures;
mod generation;
mod peek;
mod reap;

pub(in crate::relay) use self::ack::ack;
pub(in crate::relay) use self::declare::declare;
pub(in crate::relay) use self::doorbell::{Doorbell, register_doorbell};
pub(in crate::relay) use self::enqueue::{EnqueueRejection, enqueue};
pub(in crate::relay) use self::generation::{
    GenerationRejection, GenerationReplacement, ResolvedMember, claim_consumer_generation,
    replace_consumer_generation,
};
pub(in crate::relay) use self::peek::peek;
pub(in crate::relay) use self::reap::{TargetReap, reap_target};
