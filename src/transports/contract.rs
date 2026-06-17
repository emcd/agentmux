//! Transport interface contract for the relay delivery subsystem.
//!
//! The relay delivery worker dispatches every agent delivery operation through
//! the synchronous [`Transport`] trait. Concrete transports (ACP, Tmux) each
//! implement the trait in their own module; the relay selects between them via
//! the [`TransportImpl`] enum, which delegates by `match` with no dynamic
//! allocation.
//!
//! ## Why synchronous
//!
//! ACP delivery is deliberately synchronous and moved by value into
//! `spawn_blocking`; "the sync core never crosses `.await`" is an explicit
//! relay invariant. Making these methods `async` would invert that contract and
//! force each implementation to manage its own `spawn_blocking`. The relay
//! worker remains responsible for wrapping transport calls in `spawn_blocking`
//! where needed.
//!
//! ## Transport <-> relay interactions
//!
//! There is no generic inbound event channel. Each transport->relay interaction
//! uses its natural primitive:
//!
//! - **Choices** (tool-call permissions, and any future operator decision) are
//!   blocking requests: the relay injects a re-entrant [`Chooser`] via
//!   [`StartupContext`], which the transport invokes inline and blocks on until
//!   the operator decides. No transport->relay back-edge: the transport holds an
//!   opaque `Arc<dyn Fn>` typed here in `transports`.
//! - **Completion** is folded into [`Transport::deliver`], which blocks to a
//!   terminal state and returns the per-envelope outcome; the worker fans out
//!   from the return value.
//! - **Output for `look`** is a concurrent read via [`Transport::give_output`],
//!   which hands the relay an [`OutputView`] handle the look request path can
//!   read without borrowing the worker-owned transport.
//!
//! ## Slice status
//!
//! This module is the Slice 1 scaffold (revised by the `decouple-transport-layer`
//! contract amendment): the trait, the dispatch enum, the placeholder transport
//! structs, and the shared types. The placeholder [`Transport`] implementations
//! have `todo!()` bodies; the real bodies arrive when ACP delivery moves here
//! (Slice 2) and Tmux delivery moves here (Slice 3). Nothing constructs or calls
//! these types yet, so there is no behavior change.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::acp::{AcpSnapshotEntry, AcpTransport};
use crate::configuration::BundleMember;
use crate::tmux::TmuxTransport;
// Re-export the configuration prompt-readiness template into the transport
// contract namespace; tmux quiescence consumes it and Slice 3 wires it through
// the delivery context. It is defined once in `configuration`; re-exporting
// (rather than redefining) keeps the two in lockstep.
pub use crate::configuration::PromptReadinessTemplate;
// Delivery/look wire vocabulary. Canonical home is the sibling `vocabulary`
// module (below `crate::relay` in dependency order), so the transport contract
// never depends on relay. The relay re-exports these from its own contract.
use crate::transports::vocabulary::{
    AcpLookFreshness, AcpLookSnapshotSource, DeliveryPayloadMode, SendOutcome,
};

/// Synchronous delivery contract implemented by each concrete transport.
///
/// The relay worker wraps these calls in `spawn_blocking` where needed; an
/// implementation MUST NOT impose async boundaries on the worker.
pub trait Transport {
    /// Establishes (or re-establishes, on respawn) the transport runtime for a
    /// target. On respawn the transport may publish a fresh [`OutputView`]; the
    /// worker re-calls [`give_output`] afterward to pick up the new handle.
    ///
    /// [`give_output`]: Transport::give_output
    fn startup(&mut self, context: StartupContext) -> Result<TransportStatus, TransportError>;

    /// Delivers a coalesced batch of envelopes in a single call, BLOCKING until
    /// each envelope reaches a terminal state, and returning one terminal
    /// outcome per envelope aligned with the input order. Completion is owned
    /// here (no separate relay completion callback): the worker fans out from
    /// the returned [`DeliveryResult`]. Choices raised mid-delivery are serviced
    /// inline via the [`StartupContext::choose`] resolver, on the transport's own
    /// thread, without worker involvement.
    ///
    /// The relay decides how envelopes are grouped before this call (subject to
    /// [`accept_capacity`]); a transport must honor whatever batch it receives.
    /// Today the ACP worker pre-combines a coalesced turn into a single envelope
    /// and replicates the lone outcome across the contributing tasks, so ACP's
    /// `deliver` observes one envelope in practice even though [`accept_capacity`]
    /// permits more.
    ///
    /// [`accept_capacity`]: Transport::accept_capacity
    ///
    /// INVARIANT: an implementation MUST observe relay shutdown and return a
    /// terminal/dropped outcome promptly rather than parking the blocking thread
    /// indefinitely on a wedged turn.
    fn deliver(
        &mut self,
        envelopes: Vec<DeliveryEnvelope>,
        context: &DeliveryContext,
    ) -> DeliveryResult;

    /// Reports whether the transport is ready to accept delivery.
    fn is_ready(&self) -> bool;

    /// Injects raw input (no envelope framing) for `raww`.
    fn raw_write(
        &mut self,
        text: &str,
        append_enter: bool,
        context: &DeliveryContext,
    ) -> RawWriteResult;

    /// Tears down the transport runtime, releasing its resources.
    fn shutdown(&mut self);

    /// Reports the upper bound on envelopes the transport will accept per
    /// [`deliver`] call — the willingness the relay groups against, not a promise
    /// about the batch any single call receives. Tmux returns `1` (one paste per
    /// call); ACP returns `usize::MAX`, though the worker still pre-combines a
    /// turn into one envelope before dispatching (see [`deliver`]).
    ///
    /// [`deliver`]: Transport::deliver
    fn accept_capacity(&self) -> usize;

    /// Hands the relay a concurrently-readable [`OutputView`] handle for the
    /// `look` request path, or `None` for transports with no observable output.
    ///
    /// The look request runs concurrently with the worker that owns the
    /// transport, so it cannot call [`Transport`] methods directly; the handle
    /// is the shared seam it reads instead. The worker re-fetches the handle
    /// after every [`startup`] (ACP respawn allocates a fresh replay buffer).
    ///
    /// [`startup`]: Transport::startup
    fn give_output(&self) -> Option<Arc<dyn OutputView>>;
}

/// A concurrently-readable view of a transport's output for the `look` request
/// path, published by [`Transport::give_output`].
///
/// The relay stores the handle per-target and reads it from the look request
/// thread, which runs concurrently with the worker that owns the transport. The
/// handle owns the bounded prime-wait: [`look`] reads the transport's shared
/// readiness signal, waits up to [`LookMode::prime_timeout`] for a still-
/// initializing target to populate its first snapshot, then returns the entries
/// plus freshness metadata. The relay supplies only the timeout value (its
/// look-surface policy) and remains transport-generic.
///
/// [`look`]: OutputView::look
pub trait OutputView: Send + Sync {
    /// Captures a snapshot of the target's current output.
    fn look(&self, mode: LookMode) -> Result<LookSnapshotPayload, TransportError>;
}

/// Static dispatch over the fixed transport set.
///
/// Enum dispatch (not `Box<dyn Transport>`) is deliberate: the transport set is
/// fixed and small, sync RPITIT-free methods would still make the trait
/// non-object-safe if it ever went async, and enum dispatch carries zero heap
/// overhead per call.
pub enum TransportImpl {
    /// ACP delivery transport (implemented in Slice 2).
    Acp(AcpTransport),
    /// Tmux pane delivery transport (implemented in Slice 3).
    Tmux(TmuxTransport),
    /// Forward-declared PTY transport. The capability row answers now
    /// (look/write/stream all true); delivery methods are unimplemented until
    /// the PTY transport lands as the long-term replacement for Tmux. When it
    /// does, this becomes `Pty(PtyTransport)`.
    Pty,
}

impl TransportImpl {
    /// The target can be captured by `look`.
    #[must_use]
    pub fn can_be_looked(&self) -> bool {
        match self {
            Self::Acp(_) | Self::Tmux(_) | Self::Pty => true,
        }
    }

    /// The target can be written by `raww`.
    #[must_use]
    pub fn can_be_written(&self) -> bool {
        match self {
            Self::Acp(_) | Self::Tmux(_) | Self::Pty => true,
        }
    }

    /// The target's transport natively produces live output chunks.
    #[must_use]
    pub fn can_stream_output(&self) -> bool {
        match self {
            Self::Acp(_) | Self::Pty => true,
            Self::Tmux(_) => false,
        }
    }

    /// The target's transport can surface choice requests (ACP only).
    #[must_use]
    pub fn can_give_choices(&self) -> bool {
        match self {
            Self::Acp(_) => true,
            Self::Tmux(_) | Self::Pty => false,
        }
    }

    /// Establishes the transport runtime; see [`Transport::startup`].
    pub fn startup(&mut self, context: StartupContext) -> Result<TransportStatus, TransportError> {
        match self {
            Self::Acp(transport) => transport.startup(context),
            Self::Tmux(transport) => transport.startup(context),
            Self::Pty => unimplemented!("PTY transport not yet implemented"),
        }
    }

    /// Delivers a batch; see [`Transport::deliver`].
    pub fn deliver(
        &mut self,
        envelopes: Vec<DeliveryEnvelope>,
        context: &DeliveryContext,
    ) -> DeliveryResult {
        match self {
            Self::Acp(transport) => transport.deliver(envelopes, context),
            Self::Tmux(transport) => transport.deliver(envelopes, context),
            Self::Pty => unimplemented!("PTY transport not yet implemented"),
        }
    }

    /// Reports delivery readiness; see [`Transport::is_ready`].
    #[must_use]
    pub fn is_ready(&self) -> bool {
        match self {
            Self::Acp(transport) => transport.is_ready(),
            Self::Tmux(transport) => transport.is_ready(),
            Self::Pty => unimplemented!("PTY transport not yet implemented"),
        }
    }

    /// Injects raw input; see [`Transport::raw_write`].
    pub fn raw_write(
        &mut self,
        text: &str,
        append_enter: bool,
        context: &DeliveryContext,
    ) -> RawWriteResult {
        match self {
            Self::Acp(transport) => transport.raw_write(text, append_enter, context),
            Self::Tmux(transport) => transport.raw_write(text, append_enter, context),
            Self::Pty => unimplemented!("PTY transport not yet implemented"),
        }
    }

    /// Tears down the transport; see [`Transport::shutdown`].
    pub fn shutdown(&mut self) {
        match self {
            Self::Acp(transport) => transport.shutdown(),
            Self::Tmux(transport) => transport.shutdown(),
            Self::Pty => unimplemented!("PTY transport not yet implemented"),
        }
    }

    /// Reports per-call delivery capacity; see [`Transport::accept_capacity`].
    #[must_use]
    pub fn accept_capacity(&self) -> usize {
        match self {
            Self::Acp(transport) => transport.accept_capacity(),
            Self::Tmux(transport) => transport.accept_capacity(),
            Self::Pty => unimplemented!("PTY transport not yet implemented"),
        }
    }

    /// Publishes the look output handle; see [`Transport::give_output`].
    pub fn give_output(&self) -> Option<Arc<dyn OutputView>> {
        match self {
            Self::Acp(transport) => transport.give_output(),
            Self::Tmux(transport) => transport.give_output(),
            Self::Pty => unimplemented!("PTY transport not yet implemented"),
        }
    }
}

// The concrete `TmuxTransport` lives in `crate::tmux::transport` (Slice 3),
// mirroring `AcpTransport` in `crate::acp`. `TransportImpl::Tmux` delegates to
// it; the relay re-exports it via `transports::mod`.

/// Relay-provided, synchronous resolver for operator choices (tool-call
/// permissions today; any operator decision later).
///
/// Injected once at [`startup`](Transport::startup), so the transport depends
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
/// fields (`message_id`, `target_session`, `pending_max`, `decider_sessions`)
/// are sourced from the [`DeliveryContext`] and head envelope when the transport
/// raises a choice mid-`deliver`, since the startup-time chooser cannot close
/// over them.
#[derive(Clone, Debug)]
pub struct ChoiceToMake {
    /// Transport-native request id used to correlate the operator's response.
    pub request_id: u64,
    /// The originating send's message id (choice event correlation).
    pub message_id: String,
    /// The target session the choice belongs to.
    pub target_session: String,
    /// Maximum number of choices that may be pending at once for this target
    /// before the queue rejects new ones (`runtime_choices_queue_full`).
    pub pending_max: usize,
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
/// the relay's choice-resolution taxonomy so [`Transport::deliver`] can build
/// the same terminal outcome.
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
    pub bundle_name: String,
    pub runtime_directory: PathBuf,
    pub target_member: BundleMember,
    /// Relay-injected, re-entrant resolver for operator choices. See [`Chooser`].
    pub choose: Chooser,
}

impl std::fmt::Debug for StartupContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartupContext")
            .field("bundle_name", &self.bundle_name)
            .field("runtime_directory", &self.runtime_directory)
            .field("target_member", &self.target_member)
            .field("choose", &"<Chooser>")
            .finish()
    }
}

/// One rendered unit to deliver to a target.
#[derive(Clone, Debug)]
pub struct DeliveryEnvelope {
    /// Correlation id echoed back in the [`SingleDeliveryOutcome`].
    pub message_id: String,
    /// Whether `rendered` is an envelope-framed message or raw input.
    pub payload_mode: DeliveryPayloadMode,
    /// The fully rendered text handed to the transport.
    pub rendered: String,
    /// Whether to submit (append Enter) after writing, for raw input.
    pub append_enter: bool,
}

/// Per-batch context shared by every envelope in a [`Transport::deliver`] call.
#[derive(Clone, Debug)]
pub struct DeliveryContext {
    pub target_session: String,
    pub runtime_directory: PathBuf,
    pub target_member: BundleMember,
    /// A pre-resolved target handle (for example, a hoisted tmux pane) when the
    /// worker has already proven readiness; `None` means the transport resolves
    /// it. Slice 3 finalizes the tmux quiescence parameters carried here.
    pub pre_resolved_target: Option<String>,
    /// Maximum number of choices that may be pending at once for this target,
    /// used to populate [`ChoiceToMake::pending_max`] for choices raised
    /// mid-delivery.
    pub choices_pending_max: usize,
    /// Sessions authorized to decide choices raised during this delivery, used
    /// to populate [`ChoiceToMake::decider_sessions`].
    pub choice_decider_sessions: Vec<String>,
}

/// Outcomes for one [`Transport::deliver`] call, aligned with the input order.
#[derive(Clone, Debug)]
pub struct DeliveryResult {
    pub outcomes: Vec<SingleDeliveryOutcome>,
}

/// The transport-level outcome for one delivered envelope. Structurally mirrors
/// the relay `SendResult`; kept distinct so the transport vocabulary can evolve
/// independently of the relay wire contract.
#[derive(Clone, Debug)]
pub struct SingleDeliveryOutcome {
    pub target_session: String,
    pub message_id: String,
    pub outcome: SendOutcome,
    pub reason_code: Option<String>,
    pub reason: Option<String>,
    pub details: Option<Value>,
}

/// The result of a [`Transport::startup`] call.
#[derive(Clone, Debug)]
pub struct TransportStatus {
    pub readiness: TransportReadiness,
}

/// Readiness of a transport runtime after startup.
#[derive(Clone, Debug)]
pub enum TransportReadiness {
    /// Ready to accept delivery immediately.
    Ready,
    /// Established but not yet ready (for example, awaiting first prompt).
    Pending,
    /// Could not be established; carries the failure taxonomy.
    Unavailable { code: String, reason: String },
}

/// The result of a [`Transport::raw_write`] call.
#[derive(Clone, Debug)]
pub enum RawWriteResult {
    /// Input written successfully.
    Written,
    /// Write failed; carries the transport-level reason.
    Failed { reason: String },
}

/// A structured transport failure surfaced to the relay worker.
#[derive(Clone, Debug)]
pub struct TransportError {
    pub code: String,
    pub reason: String,
    pub details: Option<Value>,
}

/// Windowing parameters for an [`OutputView::look`] snapshot.
#[derive(Clone, Copy, Debug, Default)]
pub struct LookMode {
    /// Window size (tmux pane lines or ACP replay entries).
    pub lines: Option<u64>,
    /// Entries to skip from the newest end before the tail window (ACP only).
    pub offset: Option<u64>,
    /// How long the handle may wait for a still-initializing target to populate
    /// its first snapshot before returning a stale-tagged result. The relay
    /// supplies this as its look-surface policy; a zero duration means no wait.
    pub prime_timeout: Duration,
}

/// Transport-level snapshot payload returned by [`OutputView::look`].
///
/// The ACP variant carries the freshness metadata the relay forwards onto its
/// wire `LookSnapshotPayload`. The freshness enums are reused from the relay
/// contract for now (see the module's deferred-relocation note).
#[derive(Clone, Debug)]
pub enum LookSnapshotPayload {
    /// Plain text lines (tmux).
    Lines { snapshot_lines: Vec<String> },
    /// Rendered ACP replay entries plus truncation bookkeeping and freshness.
    AcpEntries {
        snapshot_entries: Vec<AcpSnapshotEntry>,
        /// Total entries available before tail/offset windowing.
        entries_total: usize,
        /// Count actually returned after the tail-N window and `offset`.
        returned_entries_count: usize,
        /// Whether the snapshot is fresh or stale.
        freshness: AcpLookFreshness,
        /// Where the snapshot was sourced from.
        snapshot_source: AcpLookSnapshotSource,
        /// Why the snapshot is stale, when applicable.
        stale_reason_code: Option<String>,
        /// Age of the snapshot in milliseconds, when known.
        snapshot_age_ms: Option<u64>,
    },
}
