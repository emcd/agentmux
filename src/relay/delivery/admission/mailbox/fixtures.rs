//! Fixture builders shared by this module's test blocks.
//!
//! The ledger is a process-global, so every fixture is namespaced by the test
//! that owns it: two tests sharing a target would share a cursor and an
//! outstanding declaration, and the one that ran second would be asserting
//! against the other's leftovers.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::configuration::SessionType;
use crate::envelope::AddressIdentity;
use crate::protocol::identity::{ConsumerBinding, ConsumerGenerationId, DeliveryTargetId};
use crate::protocol::mailbox::{EntryRange, EntrySequence, MailboxPayload};
use crate::protocol::message::{DeliveryEnvelope, DeliveryMessage};
use crate::protocol::operations::PeekRequest;

use super::super::admit::admit;
use super::super::ledger::AdmissionTargetKey;
use super::enqueue::enqueue;
use super::generation::claim_consumer_generation;

pub(super) const TARGET_SESSION: &str = "target";

pub(super) fn runtime_directory(namespace: &str) -> PathBuf {
    Path::new("/nonexistent").join(namespace)
}

pub(super) fn target(namespace: &str) -> DeliveryTargetId {
    DeliveryTargetId::new(
        namespace,
        runtime_directory(namespace).as_path(),
        TARGET_SESSION,
    )
}

/// A binding naming a generation of the caller's choosing, for the cases that
/// need one the relay did not issue.
pub(super) fn binding(namespace: &str, generation: u64) -> ConsumerBinding {
    ConsumerBinding::new(target(namespace), ConsumerGenerationId::new(generation))
}

/// Takes the target's generation from the relay and binds to it.
///
/// The only way a test gets a usable binding, because a mailbox nobody has
/// claimed has no generation and refuses every operation. Which identifier comes
/// back is the relay's business: each fixture namespace is its own target, so the
/// sequence starts fresh, and a test that named the number instead of taking it
/// would be re-asserting the sequence rather than using it.
pub(super) fn claim(namespace: &str) -> ConsumerBinding {
    let issued = claim_consumer_generation(&target(namespace)).expect("the fixture target is free");
    ConsumerBinding::new(target(namespace), issued)
}

pub(super) fn admission_key(namespace: &str) -> AdmissionTargetKey {
    AdmissionTargetKey::new(
        namespace,
        runtime_directory(namespace).as_path(),
        TARGET_SESSION,
    )
}

pub(super) fn mail(body: &str) -> MailboxPayload {
    let identity = |name: &str| AddressIdentity {
        session_name: name.to_string(),
        display_name: None,
    };
    MailboxPayload::Mail(Arc::new(DeliveryEnvelope {
        message_id: body.to_string(),
        message: DeliveryMessage {
            body: body.to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            namespace: "fixture".to_string(),
            sender: identity("sender@fixture"),
            target: identity("target@fixture"),
            cc: Vec::new(),
            authenticated_identity: None,
            on_behalf_of: None,
        },
        append_enter: true,
        choice_decider_sessions: Vec::new(),
        is_receipt: false,
    }))
}

pub(super) fn raw(content: &str) -> MailboxPayload {
    MailboxPayload::Raw {
        content: content.to_string(),
        append_enter: true,
    }
}

/// Admits an entry and makes it peekable, returning its message id.
pub(super) fn place(namespace: &str, message_id: &str, bytes: u64, payload: MailboxPayload) {
    admit(
        message_id,
        admission_key(namespace),
        SessionType::Tmux,
        bytes,
    )
    .expect("admit");
    enqueue(message_id, payload).expect("enqueue");
}

/// Admits an entry without making it peekable, leaving a hole at the
/// position it was given.
pub(super) fn admit_only(namespace: &str, message_id: &str, bytes: u64) {
    admit(
        message_id,
        admission_key(namespace),
        SessionType::Tmux,
        bytes,
    )
    .expect("admit");
}

pub(super) fn seq(value: u64) -> EntrySequence {
    EntrySequence::new(value).expect("a position is never zero")
}

pub(super) fn range(from: u64, through: u64) -> EntryRange {
    EntryRange::new(seq(from), seq(through)).expect("a fixture range is well-formed")
}

pub(super) fn request(binding: &ConsumerBinding, entry_max: usize, bytes_max: u64) -> PeekRequest {
    PeekRequest {
        binding: binding.clone(),
        entry_max,
        canonical_bytes_max: bytes_max,
    }
}

pub(super) fn peeked(binding: &ConsumerBinding, entry_max: usize, bytes_max: u64) -> Vec<u64> {
    super::peek::peek(&request(binding, entry_max, bytes_max))
        .expect("the fixture generation is current")
        .entries
        .iter()
        .map(|entry| entry.sequence.value())
        .collect()
}
