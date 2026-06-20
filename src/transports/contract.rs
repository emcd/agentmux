//! Transport interface contract for the relay delivery subsystem.
//!
//! The relay delivery worker dispatches every agent delivery operation through
//! the synchronous [`Transport`] trait. Concrete transports (ACP, Tmux) each
//! implement the trait in their own module; the relay selects between them via
//! the [`TransportImpl`] enum, which delegates by `match` with no dynamic
//! allocation.
//!
//! ## Write boundary: non-blocking, future-resolved
//!
//! The write methods ([`Transport::mailw`] for relay-framed envelopes,
//! [`Transport::raww`] for raw input) do not block. Each enqueues the write onto
//! the transport's own internal ordered channel and returns an [`OutcomeFuture`]
//! that resolves when the transport's internal delivery task drives that write
//! to a terminal [`SingleDeliveryOutcome`]. The transport owns that task, its
//! `spawn_blocking`, and the quiescence/coalesce waits; the relay worker
//! concurrently submits new writes and collects resolved futures without
//! blocking on any single one.
//!
//! This retires the earlier "the sync core never crosses `.await`; the worker
//! owns `spawn_blocking`" invariant: ownership of the blocking delivery moves
//! into each transport. The legacy synchronous [`Transport::deliver`] and
//! [`Transport::prepare_delivery`] remain during the transition and are removed
//! once their last relay callsites move onto the write methods.
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
//! ## Status
//!
//! Complete (`decouple-transport-layer`): the trait, the [`TransportImpl`]
//! dispatch enum, and the shared types live here; the ACP transport lives in
//! `crate::acp` (driven by the `AcpWorkerDriver` lifecycle behind
//! [`TransportImpl::Acp`]) and the tmux transport in `crate::tmux`. The relay
//! delivery worker holds a [`TransportImpl`] per target and dispatches every
//! agent delivery through it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::oneshot;

use crate::acp::{AcpDriverServices, AcpWorkerDriver};
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
use crate::transports::vocabulary::{DeliveryPayloadMode, LookSnapshotPayload, SendOutcome};

/// A pending delivery outcome handed back by the non-blocking write methods
/// ([`Transport::mailw`], [`Transport::raww`]). It resolves to the terminal
/// [`SingleDeliveryOutcome`] once the transport's internal delivery task settles
/// the write; the sender half lives inside the transport's ordered channel item.
///
/// Carries the transport-side [`SingleDeliveryOutcome`], not the relay
/// `SendResult`: the transport contract never depends on `crate::relay`, so the
/// relay worker maps the resolved outcome onto its own `SendResult` at the
/// collect site.
pub type OutcomeFuture = oneshot::Receiver<SingleDeliveryOutcome>;

/// Delivery contract implemented by each concrete transport.
///
/// The non-blocking write methods ([`mailw`](Transport::mailw),
/// [`raww`](Transport::raww)) return an [`OutcomeFuture`]; each transport owns
/// its own internal delivery task and `spawn_blocking`. The legacy synchronous
/// [`deliver`](Transport::deliver)/[`prepare_delivery`](Transport::prepare_delivery)
/// seam remains during the write-interface transition.
pub trait Transport {
    /// Establishes (or re-establishes, on respawn) the transport runtime for a
    /// target. On respawn the transport may publish a fresh [`OutputView`]; the
    /// worker re-calls [`give_output`] afterward to pick up the new handle.
    ///
    /// [`give_output`]: Transport::give_output
    fn startup(&mut self, context: StartupContext) -> Result<TransportStatus, TransportError>;

    /// Gates delivery on the target being ready to receive a batch, returning a
    /// resolved target handle. A transport whose readiness is observable (tmux
    /// pane quiescence; pty fd idle, later) performs the wait here; a transport
    /// with no pre-delivery wait (ACP today) returns ready immediately, echoing
    /// any [`DeliveryContext::pre_resolved_target`] back unchanged.
    ///
    /// The relay worker calls this as a distinct step BEFORE [`deliver`], so it
    /// can drain task arrivals into the batch during the wait
    /// (coalesce-during-wait); the barrier is deliberately NOT folded into
    /// [`deliver`]. On timeout, shutdown, or target unavailability it returns a
    /// [`DeliveryWaitError`] the worker fans across the coalesced batch.
    ///
    /// [`deliver`]: Transport::deliver
    fn prepare_delivery(
        &self,
        context: &DeliveryContext,
    ) -> Result<DeliveryPreparation, DeliveryWaitError>;

    /// Delivers a coalesced batch of envelopes in a single call, BLOCKING until
    /// each envelope reaches a terminal state, and returning one terminal
    /// outcome per envelope aligned with the input order. Completion is owned
    /// here (no separate relay completion callback): the worker fans out from
    /// the returned [`DeliveryResult`]. Choices raised mid-delivery are serviced
    /// inline via the [`StartupContext::choose`] resolver, on the transport's own
    /// thread, without worker involvement.
    ///
    /// The relay decides how envelopes are grouped before this call; a transport
    /// must honor whatever batch it receives. A transport that accepts at most one
    /// prompt batch per dispatch declares so via
    /// [`TransportImpl::can_take_batches`] returning `false`, and the relay peels
    /// the rendered envelopes to a single batch before calling `deliver`. The ACP
    /// worker pre-combines a coalesced turn into a single envelope and replicates
    /// the lone outcome across the contributing tasks, so ACP's `deliver` observes
    /// one envelope in practice.
    ///
    /// INVARIANT: an implementation MUST observe relay shutdown and return a
    /// terminal/dropped outcome promptly rather than parking the blocking thread
    /// indefinitely on a wedged turn.
    fn deliver(
        &mut self,
        envelopes: Vec<DeliveryEnvelope>,
        context: &DeliveryContext,
    ) -> DeliveryResult;

    /// Submits one relay-framed envelope for delivery WITHOUT blocking, returning
    /// an [`OutcomeFuture`] that resolves when the transport's internal delivery
    /// task drives this envelope to a terminal [`SingleDeliveryOutcome`]. The
    /// transport buffers the envelope on its own ordered channel, coalesces it
    /// with contiguous envelopes during its quiescence wait, and resolves the
    /// future once the combined turn settles.
    ///
    /// Replaces the blocking [`deliver`](Transport::deliver) seam; see the
    /// module-level "Write boundary" note. Default body is an additions-only stub;
    /// each transport overrides it when its internal delivery task lands
    /// (write-interface refactor sections 2-3).
    fn mailw(&mut self, envelope: DeliveryEnvelope) -> OutcomeFuture {
        let _ = envelope;
        unimplemented!("mailw lands with the per-transport internal delivery task")
    }

    /// Submits raw input (no envelope framing) for `raww` WITHOUT blocking,
    /// returning an [`OutcomeFuture`] that resolves when the write settles. FIFO
    /// with [`mailw`](Transport::mailw) on the transport's internal channel: a raw
    /// item flushes any buffered envelope group first, then delivers as its own
    /// write, acting as a batch barrier.
    ///
    /// Replaces the blocking [`raw_write`](Transport::raw_write) seam. Default body
    /// is an additions-only stub overridden when the internal delivery task lands.
    fn raww(&mut self, content: String, append_enter: bool) -> OutcomeFuture {
        let _ = (content, append_enter);
        unimplemented!("raww lands with the per-transport internal delivery task")
    }

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
#[allow(clippy::large_enum_variant)]
pub enum TransportImpl {
    /// ACP delivery transport with its worker lifecycle driver (Slice 2 / 4A-2).
    /// The driver owns the `AcpTransport` plus its bootstrap/respawn lifecycle;
    /// delivery methods delegate to the inner transport. Boxed: the driver is far
    /// larger than the other variants, and the worker moves the `TransportImpl`
    /// in and out of `spawn_blocking` each delivery, so the indirection keeps that
    /// move cheap.
    Acp(Box<AcpWorkerDriver>),
    /// Tmux pane delivery transport (implemented in Slice 3).
    Tmux(TmuxTransport),
    /// Forward-declared PTY transport. The capability row answers now
    /// (look/write/stream all true); delivery methods are unimplemented until
    /// the PTY transport lands as the long-term replacement for Tmux. When it
    /// does, this becomes `Pty(PtyTransport)`.
    Pty,
}

impl TransportImpl {
    /// Builds an ACP transport with its worker lifecycle driver for one target.
    /// The relay constructs `services` closing over its own registries; the
    /// driver imports nothing from `crate::relay`.
    #[must_use]
    pub fn acp(
        target_member: BundleMember,
        runtime_directory: PathBuf,
        bundle_name: String,
        services: AcpDriverServices,
        max_prompt_tokens: usize,
    ) -> Self {
        Self::Acp(Box::new(AcpWorkerDriver::new(
            target_member,
            runtime_directory,
            bundle_name,
            services,
            max_prompt_tokens,
        )))
    }

    /// Builds a tmux delivery transport carrying the per-prompt token budget the
    /// internal delivery task consumes when combining a coalesced envelope group.
    #[must_use]
    pub fn tmux(max_prompt_tokens: usize) -> Self {
        Self::Tmux(TmuxTransport::new(max_prompt_tokens))
    }

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

    /// The target's transport accepts the full coalesced batch in a single
    /// [`deliver`](Transport::deliver) call (Tmux/Pty). ACP accepts at most one
    /// prompt batch per turn and returns `false`, so the relay peels the rendered
    /// envelopes to a single batch and re-queues the remainder to the worker
    /// carry buffer.
    #[must_use]
    pub fn can_take_batches(&self) -> bool {
        match self {
            Self::Tmux(_) | Self::Pty => true,
            Self::Acp(_) => false,
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

    /// Runs the pre-delivery readiness barrier; see [`Transport::prepare_delivery`].
    pub fn prepare_delivery(
        &self,
        context: &DeliveryContext,
    ) -> Result<DeliveryPreparation, DeliveryWaitError> {
        match self {
            Self::Acp(transport) => transport.prepare_delivery(context),
            Self::Tmux(transport) => transport.prepare_delivery(context),
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

    /// Submits one envelope via the non-blocking write seam; see
    /// [`Transport::mailw`].
    pub fn mailw(&mut self, envelope: DeliveryEnvelope) -> OutcomeFuture {
        match self {
            Self::Acp(transport) => transport.mailw(envelope),
            Self::Tmux(transport) => transport.mailw(envelope),
            Self::Pty => unimplemented!("PTY transport not yet implemented"),
        }
    }

    /// Submits raw input via the non-blocking write seam; see
    /// [`Transport::raww`].
    pub fn raww(&mut self, content: String, append_enter: bool) -> OutcomeFuture {
        match self {
            Self::Acp(transport) => transport.raww(content, append_enter),
            Self::Tmux(transport) => transport.raww(content, append_enter),
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
/// fields (`message_id`, `target_session`, `decider_sessions`) are sourced from
/// the [`DeliveryContext`] and head envelope when the transport raises a choice
/// mid-`deliver`, since the startup-time chooser cannot close over them. The
/// queue bound (`choices_pending_max`) is a per-bundle constant the chooser
/// captures at construction, so it is not carried here.
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
    /// Sessions authorized to decide choices raised during this envelope's
    /// delivery, threaded to [`ChoiceToMake::decider_sessions`].
    pub choice_decider_sessions: Vec<String>,
    /// Quiescence poll window for the transport's internal delivery task.
    /// The transport uses this as the quiet period before declaring the target
    /// ready to receive a flush group. Ignored by transports with no
    /// quiescence wait (ACP).
    pub quiet_window: Duration,
    /// Deadline for the quiescence wait; `None` means unbounded (bounded only
    /// by relay shutdown). Ignored by transports with no quiescence wait.
    pub quiescence_timeout: Option<Duration>,
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
    /// Tmux pane quiescence poll window. The relay unpacks its `QuiescenceOptions`
    /// scheduling config into these primitives so [`Transport::prepare_delivery`]
    /// can run the pane wait without the transport depending on relay. Ignored by
    /// transports with no pre-delivery wait (ACP today).
    pub quiet_window: Duration,
    /// Deadline for the quiescence wait; `None` means unbounded (async delivery,
    /// bounded only by relay shutdown).
    pub quiescence_timeout: Option<Duration>,
    /// Sessions authorized to decide choices raised during this delivery, used
    /// to populate [`ChoiceToMake::decider_sessions`].
    pub choice_decider_sessions: Vec<String>,
}

/// The resolved outcome of a [`Transport::prepare_delivery`] barrier. Carries a
/// pre-resolved target handle (today a tmux pane string) the worker threads back
/// into [`DeliveryContext::pre_resolved_target`] for the subsequent
/// [`Transport::deliver`] call. A transport whose handle is not a string MAY
/// leave this `None` and re-resolve in its own `deliver` (an opaque per-transport
/// `DeliveryTarget` handle is a deferred generalization; see the design).
#[derive(Clone, Debug, Default)]
pub struct DeliveryPreparation {
    pub pre_resolved_target: Option<String>,
}

/// A pre-delivery readiness-barrier failure surfaced by
/// [`Transport::prepare_delivery`]. The relay maps it to a per-task `SendResult`
/// template fanned across the coalesced batch. Its canonical home is the
/// transport contract (the barrier's return type), so neither the tmux loop that
/// raises it nor the relay that maps it forms a transport<->relay back-edge.
#[derive(Debug)]
pub enum DeliveryWaitError {
    Timeout {
        timeout: Duration,
        readiness_mismatch: bool,
        mismatch_reason: Option<String>,
    },
    Failed {
        reason: String,
    },
    Shutdown,
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
