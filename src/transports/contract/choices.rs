//! Operator choices and the startup context that injects the resolver.
//!
//! [`Chooser`] is the relay-provided, re-entrant, blocking resolver a transport
//! invokes when its agent raises a tool-call permission request. It is injected
//! once at [`Transport::startup`](super::Transport::startup) through
//! [`StartupContext`], so a transport depends only downward on `transports` and
//! holds an opaque `Arc<dyn Fn>` rather than reaching into `crate::relay`.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::configuration::BundleMember;

/// Relay-provided, synchronous resolver for operator choices (tool-call
/// permissions today; any operator decision later).
///
/// Injected once at [`startup`](super::Transport::startup), so the transport depends
/// only downward on `transports`, never on `crate::relay`: the transport holds
/// an opaque `Arc<dyn Fn>`; the relay constructs it closing over its choice
/// queue. The transport invokes it on its own thread and BLOCKS until the
/// operator decides, preserving "the agent turn does not progress past a pending
/// choice."
///
/// RE-ENTRANT: the relay implementation keys per-request state by a generated
/// choice id and guards the shared queue with a mutex plus a per-request
/// condvar, so concurrent invocations (multiple permission requests in one turn)
/// each manage a distinct choice safely. INVARIANT: it MUST unblock and return
/// [`ChoiceMade::Cancelled`] on relay shutdown or respawn invalidation.
pub type Chooser = Arc<dyn Fn(ChoiceToMake) -> ChoiceMade + Send + Sync>;

/// A pending choice handed to the [`Chooser`]. The per-delivery correlation
/// fields (`message_id`, `target_session`, `decider_sessions`) are sourced from
/// the [`DeliveryEnvelope`](super::DeliveryEnvelope) the transport's internal
/// delivery task is submitting
/// when it raises a choice, since the startup-time chooser cannot close over
/// them. The queue bound (`choices_pending_max`) is a per-bundle constant the
/// chooser captures at construction, so it is not carried here.
#[derive(Clone, Debug)]
pub struct ChoiceToMake {
    /// Transport-native request id used to correlate the operator's response.
    pub request_id: u64,
    /// The originating send's message id (choice event correlation).
    pub message_id: String,
    /// The target session the choice belongs to.
    pub target_session: String,
    /// Sessions authorized to decide this choice.
    pub decider_sessions: Vec<String>,
    /// Human-facing title for the choice (for example, a tool-call title).
    pub title: String,
    /// The category of choice (for example, the requested permission kind).
    pub species: String,
    /// Transport-native detail payload for the choice.
    pub details: Value,
    /// The options the operator may choose among.
    pub options: Vec<ThingToChoose>,
}

/// One selectable option within a [`ChoiceToMake`].
#[derive(Clone, Debug)]
pub struct ThingToChoose {
    pub option_id: String,
    pub name: String,
    pub species: String,
}

/// The resolution of a [`ChoiceToMake`], returned by the [`Chooser`]. Mirrors
/// the relay's choice-resolution taxonomy so the transport's internal delivery
/// task can build the same terminal outcome.
#[derive(Clone, Debug)]
pub enum ChoiceMade {
    /// An option was chosen; carries the option id and who decided.
    Chosen {
        option_id: String,
        decided_by: String,
    },
    /// The choice was cancelled; carries the cancellation taxonomy (queue full,
    /// queue unavailable, user cancelled, shutdown, respawn invalidation).
    Cancelled {
        decided_by: String,
        reason_code: String,
        reason: Option<String>,
    },
}

/// Inputs required to establish a transport runtime for one target.
#[derive(Clone)]
pub struct StartupContext {
    pub namespace: String,
    pub runtime_directory: PathBuf,
    pub target_member: BundleMember,
    /// Relay-injected, re-entrant resolver for operator choices. See [`Chooser`].
    pub choose: Chooser,
}

impl std::fmt::Debug for StartupContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartupContext")
            .field("namespace", &self.namespace)
            .field("runtime_directory", &self.runtime_directory)
            .field("target_member", &self.target_member)
            .field("choose", &"<Chooser>")
            .finish()
    }
}
