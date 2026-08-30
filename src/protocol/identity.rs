//! Who a mailbox belongs to, and who is currently entitled to consume it.

use std::path::{Path, PathBuf};

/// Identifies the target a mailbox belongs to.
///
/// The three components are what distinguish one target from another across
/// namespaces and across relays sharing a machine: a session name alone repeats,
/// and a namespace-qualified name still repeats between two relays rooted at
/// different runtime directories.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct DeliveryTargetId {
    pub namespace: String,
    pub runtime_directory: PathBuf,
    pub target_session: String,
}

impl DeliveryTargetId {
    #[must_use]
    pub fn new(namespace: &str, runtime_directory: &Path, target_session: &str) -> Self {
        Self {
            namespace: namespace.to_string(),
            runtime_directory: runtime_directory.to_path_buf(),
            target_session: target_session.to_string(),
        }
    }
}

/// Identifies one consumer generation for a target.
///
/// Values are drawn per target identity from a monotonically increasing sequence
/// that is never reused — not even after a target is fully torn down and later
/// recreated under the same session name. That is what makes a stale identifier
/// held by some errant caller guaranteed non-matching rather than possibly
/// colliding with a freshly issued one.
///
/// The sequence itself is relay-owned state, so this type carries no minting
/// operation: an identifier is issued with a target's generation, never
/// manufactured beside one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConsumerGenerationId(u64);

impl ConsumerGenerationId {
    #[must_use]
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }
}

/// A delivery executor's claim to consume one target's mailbox.
///
/// Carried on every peek, declaration, and acknowledgment, and checked against
/// the target's active generation before any of them takes effect. Pairing the
/// two identifiers in one value is deliberate: they are never meaningful apart,
/// and passing them separately invites a call site that checks the generation
/// against the wrong target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsumerBinding {
    pub target: DeliveryTargetId,
    pub generation: ConsumerGenerationId,
}

impl ConsumerBinding {
    #[must_use]
    pub fn new(target: DeliveryTargetId, generation: ConsumerGenerationId) -> Self {
        Self { target, generation }
    }
}
