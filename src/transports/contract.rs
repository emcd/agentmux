//! Transport interface contract for the relay delivery subsystem.
//!
//! The relay delivery worker dispatches every agent delivery operation through
//! the [`Transport`] trait. Concrete transports (ACP, Tmux, UI) each implement
//! the trait in their own module; the relay selects between them via the
//! [`TransportImpl`] enum, which delegates by `match` with no dynamic
//! allocation. Promoting UI (and the forward-declared Pubsub) to first-class
//! transports retires the relay's former `Acp/Tmux/Ui/Pubsub` routing fork.
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
//! into each transport. The legacy synchronous `deliver`/`prepare_delivery`/
//! `raw_write` seam has been removed now that every relay callsite delivers
//! through the write methods.
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
//! - **Completion** resolves through the [`OutcomeFuture`] returned by
//!   [`Transport::mailw`]/[`Transport::raww`]: the transport's internal delivery
//!   task drives each write to a terminal [`SingleDeliveryOutcome`], and the
//!   worker fans out from the resolved future.
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
use crate::configuration::{BundleMember, SessionType};
use crate::tmux::TmuxTransport;
use crate::transports::ui::{UiTransport, UiTransportServices};
// Re-export the configuration prompt-readiness template into the transport
// contract namespace; tmux quiescence consumes it and Slice 3 wires it through
// the delivery context. It is defined once in `configuration`; re-exporting
// (rather than redefining) keeps the two in lockstep.
pub use crate::configuration::PromptReadinessTemplate;
// Pane-envelope rendering helpers. Canonical home is the transport-safe
// `crate::envelope` module (it imports no relay internals), so coder transports
// can render their own pane text from the structured delivery message.
use crate::envelope::{AddressIdentity, EnvelopeRenderInput, PromptBatchSettings, render_envelope};
// Delivery/look wire vocabulary. Canonical home is the sibling `vocabulary`
// module (below `crate::relay` in dependency order), so the transport contract
// never depends on relay. The relay re-exports these from its own contract.
use crate::transports::vocabulary::{LookSnapshotPayload, SendOutcome};

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
/// its own internal delivery task and `spawn_blocking`. They are the relay's
/// only delivery seam — the legacy synchronous `deliver`/`prepare_delivery`/
/// `raw_write` methods have been removed.
/// The three actions a generation supervisor needs to fence a transport
/// generation, split out of [`Transport`] so the fence protocol can be driven
/// against anything that can be stopped and observed.
///
/// A generation SHALL be torn down and fenced before its replacement begins, so
/// an old generation cannot submit after its `Authorized` entries were resolved
/// against it. Without that, "resolved unknown" and "still able to act" coexist,
/// which is a target-side ordering hazard.
///
/// **Marking a generation fenced is not a fence.** A submission already past its
/// check will still produce its effect. Only *observed cessation* — through
/// [`generation_ceased`](Self::generation_ceased) — establishes that execution
/// has stopped.
///
/// None of the three has a default body. A defaulted
/// [`generation_ceased`](Self::generation_ceased) is the dangerous one: `true`
/// would acknowledge a fence no one observed, releasing a replacement while the
/// old generation can still write, and `false` would make every fence negative
/// and every target permanently unreplaceable. Neither is a safe thing to get by
/// forgetting to implement it.
pub trait GenerationFence {
    /// Step 1 — cooperative stop request. Marks the generation fenced so an
    /// executor that checks the flag stops at its next check.
    ///
    /// A signal, not a wait. It costs nothing when it works, which is why it is
    /// tried before the destructive step 3: escalating straight to termination
    /// would destroy a child that was about to stop on its own.
    fn fence_generation(&mut self);

    /// Step 3 — forced generation termination. Initiates cessation of every
    /// effect path this generation owns and returns **without blocking**.
    ///
    /// Not "kill the child": that is one implementation and does not generalise
    /// to a transport owning no child, or one reaching its target through a
    /// process it does not own. Tmux in particular SHALL NOT terminate the tmux
    /// server, which belongs to the operator rather than to the generation.
    ///
    /// Invoking it successfully does **not** acknowledge the fence. It initiates;
    /// step 4 observes. Its value is that it unblocks an executor blocked writing
    /// into the terminated path, so step 4's observation can succeed where step
    /// 2's could not.
    fn terminate_generation(&mut self);

    /// Steps 2 and 4 — the cessation observation. Whether every
    /// generation-owned executor has been observed to cease.
    ///
    /// Non-blocking, and deliberately not a join: no runtime primitive can force
    /// a thread blocked in a syscall to return, so a blocking join would
    /// reintroduce the unbounded wait the fence bound exists to close. The
    /// supervisor polls this and gives up on its own clock.
    fn generation_ceased(&self) -> bool;
}

pub trait Transport: GenerationFence {
    /// Establishes (or re-establishes, on respawn) the transport runtime for a
    /// target. On respawn the transport may publish a fresh [`OutputView`]; the
    /// worker re-calls [`give_output`] afterward to pick up the new handle.
    ///
    /// [`give_output`]: Transport::give_output
    fn startup(&mut self, context: StartupContext) -> Result<TransportStatus, TransportError>;

    /// Submits one relay-framed envelope for delivery WITHOUT blocking, returning
    /// an [`OutcomeFuture`] that resolves when the transport's internal delivery
    /// task drives this envelope to a terminal [`SingleDeliveryOutcome`]. The
    /// transport buffers the envelope on its own ordered channel, coalesces it
    /// with contiguous envelopes during its quiescence wait, and resolves the
    /// future once the combined turn settles.
    ///
    /// The relay's sole envelope-delivery seam; see the module-level "Write
    /// boundary" note. Default body is an additions-only stub; each transport
    /// overrides it with its internal delivery task.
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
    /// The relay's sole raw-input delivery seam. Default body is an
    /// additions-only stub overridden when the internal delivery task lands.
    fn raww(&mut self, content: String, append_enter: bool) -> OutcomeFuture {
        let _ = (content, append_enter);
        unimplemented!("raww lands with the per-transport internal delivery task")
    }

    /// Reports whether the transport is ready to accept delivery.
    fn is_ready(&self) -> bool;

    /// Reports whether the transport can accept a handover now.
    ///
    /// This is a level-triggered, advisory observation. A caller must still
    /// handle a fallible delivery attempt after reading it.
    fn can_accept_handover(&self) -> bool {
        self.is_ready()
    }

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

/// Lets the fence protocol drive a [`TransportImpl`] through the same seam it
/// drives any other generation, so the supervisor holds no knowledge of which
/// transport it is fencing.
impl GenerationFence for TransportImpl {
    fn fence_generation(&mut self) {
        TransportImpl::fence_generation(self);
    }

    fn terminate_generation(&mut self) {
        TransportImpl::terminate_generation(self);
    }

    fn generation_ceased(&self) -> bool {
        TransportImpl::generation_ceased(self)
    }
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

/// The largest handover a transport will accept, declared statically per
/// transport and expressed in the two units the relay can evaluate without
/// packing: envelope count and canonical payload bytes.
///
/// "Canonical payload bytes" means the serialized envelope payload the relay
/// already holds, not the text a transport would render for its target.
/// Declaring the maxima in tokens would be circular, since only the transport can
/// render and count those.
///
/// These are distinct from admission quota (relay-owned, how much may be queued)
/// and from acceptance capacity (dynamic, whether the transport can accept right
/// now). The relay uses them for two things: rejecting at admission an envelope
/// no partition could ever carry, and stopping batch formation at whichever
/// component binds first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HandoverDimensions {
    /// Most envelopes one handover may carry.
    pub envelopes_max: usize,
    /// Most canonical payload bytes one handover may carry.
    pub canonical_bytes_max: u64,
}

/// The canonical-payload-byte ceiling every delivering transport declares.
///
/// One value rather than five because the differences between these transports
/// are not byte ceilings: a tmux pane, a child's stdin, a pty master, and a
/// broadcast channel all accept far more than any envelope a relay peer should be
/// sending, and each transport's real constraint binds elsewhere and later —
/// tmux and pty against the rendered prompt's token budget, ACP against its
/// framing. What this value is for is the line past which an envelope is not a
/// large message but a mistake, so that admission rejects it at the request
/// boundary instead of queueing something no partition could carry.
///
/// It equals the default `[delivery].scheduling-quantum-bytes`, which the spec
/// requires to be at least the largest declared byte component. Equality is the
/// intended relationship: a rotation visit's credit exactly covers one maximal
/// handover.
const HANDOVER_CANONICAL_BYTES_MAX: u64 = 262_144;

/// Most envelopes one handover may carry on a transport that coalesces a group
/// into a single turn (tmux, ACP, pty). Matches the ACP worker's existing pending
/// bound so batch formation admits nothing looser than what that queue already
/// allowed.
const HANDOVER_ENVELOPES_MAX_COALESCING: usize = 64;

impl HandoverDimensions {
    /// The maxima declared by the transport implementing `session_type`, or
    /// `None` for a session type with no delivery path.
    ///
    /// `Pubsub` returns `None`: it is a forward-declared stub rejected
    /// synchronously at admission, so it never reaches batch formation and has no
    /// dimensions to declare. `None` therefore means "cannot accept a handover at
    /// all", not "unbounded".
    #[must_use]
    pub fn for_session_type(session_type: SessionType) -> Option<Self> {
        match session_type {
            SessionType::Tmux | SessionType::Acp | SessionType::Pty => Some(Self {
                envelopes_max: HANDOVER_ENVELOPES_MAX_COALESCING,
                canonical_bytes_max: HANDOVER_CANONICAL_BYTES_MAX,
            }),
            // UI broadcasts one stream event per envelope and coalesces nothing,
            // so a handover is exactly one envelope.
            SessionType::Ui => Some(Self {
                envelopes_max: 1,
                canonical_bytes_max: HANDOVER_CANONICAL_BYTES_MAX,
            }),
            SessionType::Pubsub => None,
        }
    }

    /// The largest canonical-payload-byte component any transport declares, with
    /// the session type that declared it.
    ///
    /// This is what `[delivery].scheduling-quantum-bytes` must be at least: a
    /// rotation visit whose credit is smaller than some transport's maximal
    /// handover could never grant enough to submit one, and that target would be
    /// visited forever without progressing.
    ///
    /// Session types with no delivery path contribute nothing — they accept no
    /// handover, so no quantum is too small for them.
    #[must_use]
    pub fn largest_declared_canonical_bytes() -> (SessionType, u64) {
        const ALL: [SessionType; 5] = [
            SessionType::Tmux,
            SessionType::Acp,
            SessionType::Pty,
            SessionType::Ui,
            SessionType::Pubsub,
        ];
        ALL.into_iter()
            .filter_map(|session_type| {
                Self::for_session_type(session_type)
                    .map(|dimensions| (session_type, dimensions.canonical_bytes_max))
            })
            // Folded rather than `max_by_key` so a tie keeps the first declaring
            // session type: every transport declares the same byte component
            // today, and a rejection that names them in declaration order reads
            // less arbitrarily than one naming whichever landed last.
            .fold(None, |largest, candidate| match largest {
                Some((_, bytes)) if candidate.1 <= bytes => largest,
                _ => Some(candidate),
            })
            .expect("at least one session type declares handover dimensions")
    }
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
    /// UI stream-broadcast transport. Delivers via `mailw` (a single broadcast
    /// with a bounded reconnect wait); not lookable, not raw-writable, not
    /// batchable. Promoting UI to a first-class transport retires the relay's
    /// `Acp/Tmux/Ui/Pubsub` routing fork.
    Ui(UiTransport),
    /// Forward-declared pub/sub fan-out transport. The capability row answers now
    /// (mirrors UI: not lookable/writable/streamable/batchable); delivery methods
    /// are unimplemented until the Pubsub transport lands. When it does, this
    /// becomes `Pubsub(PubsubTransport)`.
    Pubsub,
    /// Pty transport (libghostty-vt-backed delivery with portable-pty child
    /// process management). When the `pty` Cargo feature is OFF, the
    /// variant stays a unit stub (matches the original forward-declared
    /// shape) and every dispatch arm panics. When the feature is ON,
    /// the variant carries a real [`PtyTransport`](crate::pty::PtyTransport)
    /// and the dispatch arms delegate to it.
    #[cfg(feature = "pty")]
    Pty(crate::pty::PtyTransport),
    #[cfg(not(feature = "pty"))]
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
        namespace: String,
        services: AcpDriverServices,
        batch_settings: PromptBatchSettings,
    ) -> Self {
        Self::Acp(Box::new(AcpWorkerDriver::new(
            target_member,
            runtime_directory,
            namespace,
            services,
            batch_settings,
        )))
    }

    /// Builds a tmux delivery transport carrying the prompt-batch settings (token
    /// budget and tokenizer profile) the internal delivery task consumes when
    /// combining a coalesced envelope group.
    #[must_use]
    pub fn tmux(batch_settings: PromptBatchSettings) -> Self {
        Self::Tmux(TmuxTransport::new(batch_settings))
    }

    /// Builds a UI stream-broadcast transport for one target. The relay
    /// constructs `services` closing over its own stream registry; the transport
    /// imports nothing from `crate::relay`.
    #[must_use]
    pub fn ui(services: UiTransportServices) -> Self {
        Self::Ui(UiTransport::new(services))
    }

    /// Builds a Pty transport for one target. Only available when the
    /// `pty` Cargo feature is enabled; callers select transport at the
    /// bundle-config layer per `[coders.<id>]` (the per-coder
    /// configuration lands in §6 alongside `PtyTargetConfiguration`).
    ///
    /// `mirror_state` is the relay-constructed closure that mirrors
    /// per-turn readiness transitions into the relay's global
    /// worker-state registry. The relay dispatcher closes over
    /// `set_worker_readiness(namespace, runtime_directory,
    /// target_session, state)` (see
    /// `src/relay/delivery/async_worker.rs`); the transport holds an
    /// opaque `Arc<dyn Fn>` so `src/pty` does not import
    /// `crate::relay`. Mirrors `AcpWorkerDriver`'s `MirrorStateFn`
    /// (see `src/acp/worker_driver.rs`).
    #[cfg(feature = "pty")]
    #[must_use]
    pub fn pty(
        target_member: crate::configuration::BundleMember,
        config: crate::pty::PtyTargetConfiguration,
        mirror_state: Option<crate::pty::PtyMirrorStateFn>,
    ) -> Self {
        Self::Pty(crate::pty::PtyTransport::new(
            target_member,
            config,
            mirror_state,
        ))
    }

    /// The target can be captured by `look`.
    #[must_use]
    pub fn can_be_looked(&self) -> bool {
        match self {
            Self::Acp(_) | Self::Tmux(_) => true,
            #[cfg(feature = "pty")]
            Self::Pty(_) => true,
            Self::Ui(_) | Self::Pubsub => false,
            #[cfg(not(feature = "pty"))]
            Self::Pty => false,
        }
    }

    /// The target can be written by `raww`.
    #[must_use]
    pub fn can_be_written(&self) -> bool {
        match self {
            Self::Acp(_) | Self::Tmux(_) => true,
            #[cfg(feature = "pty")]
            Self::Pty(_) => true,
            Self::Ui(_) | Self::Pubsub => false,
            #[cfg(not(feature = "pty"))]
            Self::Pty => false,
        }
    }

    /// The target's transport natively produces live output chunks.
    #[must_use]
    pub fn can_stream_output(&self) -> bool {
        match self {
            Self::Acp(_) => true,
            #[cfg(feature = "pty")]
            Self::Pty(_) => true,
            Self::Tmux(_) | Self::Ui(_) | Self::Pubsub => false,
            #[cfg(not(feature = "pty"))]
            Self::Pty => false,
        }
    }

    /// The target's transport can surface choice requests (ACP only).
    #[must_use]
    pub fn can_give_choices(&self) -> bool {
        match self {
            Self::Acp(_) => true,
            Self::Tmux(_) | Self::Ui(_) | Self::Pubsub => false,
            #[cfg(feature = "pty")]
            Self::Pty(_) => false,
            #[cfg(not(feature = "pty"))]
            Self::Pty => false,
        }
    }

    /// The largest handover this transport accepts; see [`HandoverDimensions`].
    ///
    /// Delegates to the session-type function so the live-instance answer and the
    /// one the relay reads at admission — where no transport exists yet — cannot
    /// diverge.
    #[must_use]
    pub fn maximum_handover_dimensions(&self) -> Option<HandoverDimensions> {
        HandoverDimensions::for_session_type(self.session_type())
    }

    /// The session type this transport implements.
    #[must_use]
    pub fn session_type(&self) -> SessionType {
        match self {
            Self::Acp(_) => SessionType::Acp,
            Self::Tmux(_) => SessionType::Tmux,
            Self::Ui(_) => SessionType::Ui,
            Self::Pubsub => SessionType::Pubsub,
            #[cfg(feature = "pty")]
            Self::Pty(_) => SessionType::Pty,
            #[cfg(not(feature = "pty"))]
            Self::Pty => SessionType::Pty,
        }
    }

    /// Establishes the transport runtime; see [`Transport::startup`].
    pub fn startup(&mut self, context: StartupContext) -> Result<TransportStatus, TransportError> {
        match self {
            Self::Acp(transport) => transport.startup(context),
            Self::Tmux(transport) => transport.startup(context),
            Self::Ui(transport) => transport.startup(context),
            Self::Pubsub => unimplemented!("Pubsub transport not yet implemented"),
            #[cfg(feature = "pty")]
            Self::Pty(transport) => transport.startup(context),
            #[cfg(not(feature = "pty"))]
            Self::Pty => {
                unimplemented!("PTY transport is feature-gated; rebuild with --features pty")
            }
        }
    }

    /// Submits one envelope via the non-blocking write seam; see
    /// [`Transport::mailw`].
    pub fn mailw(&mut self, envelope: DeliveryEnvelope) -> OutcomeFuture {
        match self {
            Self::Acp(transport) => transport.mailw(envelope),
            Self::Tmux(transport) => transport.mailw(envelope),
            Self::Ui(transport) => transport.mailw(envelope),
            Self::Pubsub => unimplemented!("Pubsub transport not yet implemented"),
            #[cfg(feature = "pty")]
            Self::Pty(transport) => transport.mailw(envelope),
            #[cfg(not(feature = "pty"))]
            Self::Pty => {
                unimplemented!("PTY transport is feature-gated; rebuild with --features pty")
            }
        }
    }

    /// Submits raw input via the non-blocking write seam; see
    /// [`Transport::raww`].
    pub fn raww(&mut self, content: String, append_enter: bool) -> OutcomeFuture {
        match self {
            Self::Acp(transport) => transport.raww(content, append_enter),
            Self::Tmux(transport) => transport.raww(content, append_enter),
            Self::Ui(transport) => transport.raww(content, append_enter),
            Self::Pubsub => unimplemented!("Pubsub transport not yet implemented"),
            #[cfg(feature = "pty")]
            Self::Pty(transport) => transport.raww(content, append_enter),
            #[cfg(not(feature = "pty"))]
            Self::Pty => {
                unimplemented!("PTY transport is feature-gated; rebuild with --features pty")
            }
        }
    }

    /// Reports delivery readiness; see [`Transport::is_ready`].
    #[must_use]
    pub fn is_ready(&self) -> bool {
        match self {
            Self::Acp(transport) => transport.is_ready(),
            Self::Tmux(transport) => transport.is_ready(),
            Self::Ui(transport) => transport.is_ready(),
            // The delivery worker latches a `Pubsub` stub for a configured Pubsub
            // target (delivery is guarded and answered with a not-implemented
            // outcome), so its query/lifecycle delegates must not panic. It is
            // never ready to deliver.
            Self::Pubsub => false,
            #[cfg(feature = "pty")]
            Self::Pty(transport) => transport.is_ready(),
            #[cfg(not(feature = "pty"))]
            Self::Pty => false,
        }
    }

    /// Reports whether the selected transport can accept a handover now.
    #[must_use]
    pub fn can_accept_handover(&self) -> bool {
        match self {
            Self::Acp(transport) => transport.can_accept_handover(),
            Self::Tmux(transport) => transport.can_accept_handover(),
            Self::Ui(transport) => transport.can_accept_handover(),
            Self::Pubsub => false,
            #[cfg(feature = "pty")]
            Self::Pty(transport) => transport.can_accept_handover(),
            #[cfg(not(feature = "pty"))]
            Self::Pty => false,
        }
    }

    /// Requests a cooperative stop; see [`GenerationFence::fence_generation`].
    pub fn fence_generation(&mut self) {
        match self {
            Self::Acp(transport) => transport.fence_generation(),
            Self::Tmux(transport) => transport.fence_generation(),
            Self::Ui(transport) => transport.fence_generation(),
            // The `Pubsub` stub is refused at admission, so it never owns a
            // generation to fence. The same holds for a `Pty` that the feature
            // gate leaves unconstructible.
            Self::Pubsub => {}
            #[cfg(feature = "pty")]
            Self::Pty(transport) => transport.fence_generation(),
            #[cfg(not(feature = "pty"))]
            Self::Pty => {}
        }
    }

    /// Initiates forced termination; see [`GenerationFence::terminate_generation`].
    pub fn terminate_generation(&mut self) {
        match self {
            Self::Acp(transport) => transport.terminate_generation(),
            Self::Tmux(transport) => transport.terminate_generation(),
            Self::Ui(transport) => transport.terminate_generation(),
            Self::Pubsub => {}
            #[cfg(feature = "pty")]
            Self::Pty(transport) => transport.terminate_generation(),
            #[cfg(not(feature = "pty"))]
            Self::Pty => {}
        }
    }

    /// Observes cessation; see [`GenerationFence::generation_ceased`].
    pub fn generation_ceased(&self) -> bool {
        match self {
            Self::Acp(transport) => transport.generation_ceased(),
            Self::Tmux(transport) => transport.generation_ceased(),
            Self::Ui(transport) => transport.generation_ceased(),
            // A stub that owns no executor has trivially ceased. This is the one
            // place `true` is the honest answer rather than the dangerous
            // default the trait refuses to provide: there is nothing here that
            // could still write to a target.
            Self::Pubsub => true,
            #[cfg(feature = "pty")]
            Self::Pty(transport) => transport.generation_ceased(),
            #[cfg(not(feature = "pty"))]
            Self::Pty => true,
        }
    }

    /// Tears down the transport; see [`Transport::shutdown`].
    pub fn shutdown(&mut self) {
        match self {
            Self::Acp(transport) => transport.shutdown(),
            Self::Tmux(transport) => transport.shutdown(),
            Self::Ui(transport) => transport.shutdown(),
            // The `Pubsub` stub owns no runtime, so teardown is a no-op — and it
            // MUST NOT panic: the delivery worker latches this stub for a
            // configured Pubsub target and calls `shutdown()` on it during relay
            // shutdown (`shutdown_drain`). `Pty` is not yet constructible when
            // the `pty` feature is off, so it can never be a latched shutdown
            // target; when the feature is on, the inner `PtyTransport`
            // receives the shutdown call.
            Self::Pubsub => {}
            #[cfg(feature = "pty")]
            Self::Pty(transport) => transport.shutdown(),
            #[cfg(not(feature = "pty"))]
            Self::Pty => {}
        }
    }

    /// Publishes the look output handle; see [`Transport::give_output`].
    pub fn give_output(&self) -> Option<Arc<dyn OutputView>> {
        match self {
            Self::Acp(transport) => transport.give_output(),
            Self::Tmux(transport) => transport.give_output(),
            Self::Ui(transport) => transport.give_output(),
            // The latched `Pubsub` stub is not lookable; it publishes no handle.
            Self::Pubsub => None,
            #[cfg(feature = "pty")]
            Self::Pty(transport) => transport.give_output(),
            #[cfg(not(feature = "pty"))]
            Self::Pty => None,
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
/// the [`DeliveryEnvelope`] the transport's internal delivery task is submitting
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

/// One structured message to deliver to a target, plus the per-write control
/// hints the transport's internal delivery task needs.
///
/// The relay populates [`message`](Self::message) with relay-authored attribution
/// after routing and authorization; the transport renders its own representation
/// from those fields (coder transports render pane-envelope text, UI builds a
/// stream event) and never infers or mutates attribution. The remaining fields
/// are per-write transport control, not message content.
#[derive(Clone, Debug)]
pub struct DeliveryEnvelope {
    /// Correlation id echoed back in the [`SingleDeliveryOutcome`].
    pub message_id: String,
    /// Structured, transport-neutral message data. The receiving transport
    /// renders the representation it owns from these fields.
    pub message: DeliveryMessage,
    /// Whether to submit (append Enter) after writing the rendered text.
    pub append_enter: bool,
    /// Sessions authorized to decide choices raised during this envelope's
    /// delivery, threaded to [`ChoiceToMake::decider_sessions`].
    pub choice_decider_sessions: Vec<String>,
    /// Quiescence poll window for the transport's internal delivery task.
    /// The transport uses this as the quiet period before declaring the target
    /// ready to receive a flush group. Ignored by transports with no
    /// quiescence wait (ACP).
    pub quiet_window: Duration,
    /// Generic bounded prime window for any prime-wait transport's internal
    /// delivery task. The relay populates it from per-coder config
    /// (`[coders.<id>.tmux].prime-timeout-ms` today; ACP follow-up will set
    /// it for ACP targets) without knowing which transport will consume it,
    /// so the envelope stays transport-neutral.
    ///
    /// `None` issues no prime-window verdict. It does NOT mean the wait is
    /// unbounded: see `readiness_timeout_ms`, which applies regardless where
    /// the transport defines one. When the prime window elapses during a
    /// transport's prime wait with no observable output, the transport MUST
    /// resolve its wait as `SendOutcome::Timeout` (existing outcome variant).
    /// This field bounds only the prime window.
    pub prime_timeout_ms: Option<u64>,
    /// Bound on the ENTIRE wait for a flush group, for transports whose
    /// readiness contract defines one. Covers the prime window and any period
    /// of continuous target activity, not merely the post-quiescence stretch,
    /// and no signal defers, extends, or suspends it.
    ///
    /// Populated only for transports that both define a readiness bound and
    /// can soundly report its expiry as non-delivery. That is Tmux alone
    /// today: Tmux injects into the pane only after its readiness wait, so an
    /// expired bound provably precedes delivery. Pty writes every envelope to
    /// the PTY master before its wait and ACP submits the prompt before its
    /// wait, so on those an expired bound may follow actual delivery and
    /// reporting non-delivery would assert what the transport cannot
    /// establish. They receive `None`, as do UI and pubsub.
    ///
    /// A `None` value MUST NOT be read as that transport being bounded by
    /// some other means. It is not. See `agentmux:issues/relay/61`.
    pub readiness_timeout_ms: Option<u64>,
    /// True when this envelope carries a terminal-outcome receipt (a
    /// relay/system-originated notice back to the original sender for a
    /// non-delivered outcome). Carried on the envelope so per-transport
    /// rendering polish (e.g. ACP's flush-barrier behavior) can branch on it
    /// without re-deriving from the message body. The relay's delivery
    /// mechanics are receipt-agnostic; this flag is a per-transport hint,
    /// not a dispatch concern. `false` for ordinary peer messages.
    pub is_receipt: bool,
}

/// Structured, transport-neutral message data sufficient for any transport to
/// render its own representation without importing `crate::relay` or parsing
/// already-rendered text. The relay authors every field; transports treat them
/// as read-only input.
///
/// Each party is carried as an [`AddressIdentity`] directly: coder transports
/// render the decorating pane-header form via `render_address`, while
/// machine-consumed event fields use the bare
/// [`AddressIdentity::canonical_session_id`] form.
#[derive(Clone, Debug)]
pub struct DeliveryMessage {
    /// The message body text.
    pub body: String,
    /// RFC 3339 creation timestamp, rendered into the `Date` header.
    pub created_at: String,
    /// The routing namespace qualifying canonical `session@namespace` ids
    /// (a session bundle, or a relay-wide namespace such as `GLOBAL`).
    pub namespace: String,
    /// Canonical sender identity.
    pub sender: AddressIdentity,
    /// Canonical target identity.
    pub target: AddressIdentity,
    /// Canonical co-recipient identities (the full target set minus this
    /// envelope's own recipient), including co-recipients in other namespaces.
    pub cc: Vec<AddressIdentity>,
    /// The sender's verified `principal_id`, when present; `None` for
    /// socket-trust senders.
    pub authenticated_identity: Option<String>,
    /// Origin principal a peer relay forwarded this message on behalf of, carried
    /// uninterpreted alongside `authenticated_identity` (the peer relay). `None`
    /// for local delivery and non-relay senders.
    pub on_behalf_of: Option<String>,
}

impl DeliveryMessage {
    /// Renders this message as RFC 822/MIME pane-envelope text. Coder transports
    /// (Tmux/ACP) call this before writing to the harness; UI does not render
    /// pane text. `message_id` is the owning envelope's correlation id, which
    /// seeds the MIME boundary and `Message-Id` header.
    #[must_use]
    pub fn render_pane_envelope(&self, message_id: &str) -> String {
        render_envelope(&EnvelopeRenderInput {
            message_id: message_id.to_string(),
            created_at: self.created_at.clone(),
            from: self.sender.clone(),
            to: vec![self.target.clone()],
            cc: self.cc.clone(),
            subject: None,
            body: self.body.clone(),
        })
    }
}

/// A quiescence-barrier failure surfaced by the tmux transport's internal
/// delivery task when it waits for its target pane to fall quiet before a flush
/// group. The task maps it to a [`SingleDeliveryOutcome`] for the buffered
/// group. Its canonical home is the transport contract, so the tmux loop that
/// raises it forms no transport<->relay back-edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryWaitError {
    /// The prime window elapsed during the wait with no observable output.
    /// Maps to `SendOutcome::Timeout` (existing variant); the
    /// `readiness_mismatch`/`mismatch_reason` fields capture whether the
    /// pane was already in a not-prompt-ready state at fire time.
    Timeout {
        timeout: Duration,
        readiness_mismatch: bool,
        mismatch_reason: Option<String>,
    },
    /// The target became quiescent + not-prompt-ready, and wedge detection is
    /// enabled. Only Pty enables it: Tmux passes `wedge_detection: false`
    /// because a settled non-prompt frame is produced by a hung coder, a
    /// permission dialog, a compose box, and a coder working silently alike,
    /// and `capture-pane` cannot tell them apart. Maps to
    /// `SendOutcome::Failed` with `reason_code = "pane_wedged"`. The
    /// `reason` carries the last-observed prompt-readiness mismatch reason
    /// (or a default placeholder when the probe did not record one) so
    /// operators can diagnose the wedge from the inscribed diagnostic.
    Wedged {
        reason: String,
    },
    /// The flush group's readiness bound elapsed without the target becoming
    /// ready. Maps to `SendOutcome::Timeout` carrying `reason_code`'s string
    /// form. Distinct from [`DeliveryWaitError::Timeout`] because the two
    /// bounds diagnose different things and the prime timeout outranks this
    /// one when both elapse in the same iteration.
    ReadinessTimeout {
        reason_code: ReadinessTimeoutReason,
        elapsed: Duration,
        mismatch_reason: Option<String>,
    },
    Failed {
        reason: String,
    },
    Shutdown,
}

/// Why a readiness bound expired, derived from the most recent observation.
///
/// Diagnostic only: every variant resolves the same `SendOutcome::Timeout`.
/// The distinction exists so an operator can tell a short bound from a stuck
/// target, not so the transport can decide differently. In particular none of
/// these is a claim that the target has failed — a settled non-prompt frame is
/// produced by a hung coder, a permission dialog, a compose box, and a coder
/// working without terminal output alike, and the inspected tail cannot
/// separate them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadinessTimeoutReason {
    /// Target activity advanced across the observation pair: the target was
    /// producing output the whole time and never settled.
    TargetNeverSettled,
    /// The inspected tail carried no observable content.
    TargetUnresponsive,
    /// The prompt frame was present but the cursor sat away from its
    /// configured idle column, so an operator has input pending.
    PendingOperatorInput,
    /// The prompt frame was absent. Covers the four indistinguishable cases
    /// together, precisely because they are indistinguishable.
    TargetNotReady,
}

impl ReadinessTimeoutReason {
    /// Returns the stable `reason_code` string reported to the sender.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::TargetNeverSettled => "target_never_settled",
            Self::TargetUnresponsive => "target_unresponsive",
            Self::PendingOperatorInput => "pending_operator_input",
            Self::TargetNotReady => "target_not_ready",
        }
    }
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
