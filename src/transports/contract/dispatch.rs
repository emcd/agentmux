//! Static dispatch over the fixed transport set.
//!
//! [`TransportImpl`] delegates every [`Transport`] method to the concrete
//! transport it holds, by `match` and with no dynamic allocation.
//! [`HandoverDimensions`] sits here too: the maxima are declared per session
//! type, which is the same fixed set this enum dispatches over.
//!
//! The traits being dispatched live in [`transport`](super::transport) and the
//! shared delivery types in the parent [`contract`](super) module.

use std::path::PathBuf;
use std::sync::Arc;

use crate::acp::{AcpDriverServices, AcpWorkerDriver};
use crate::configuration::{BundleMember, SessionType};
use crate::envelope::PromptBatchSettings;
use crate::tmux::TmuxTransport;
use crate::transports::ui::{UiTransport, UiTransportServices};

use super::{
    DeliveryEnvelope, GenerationFence, OutcomeFuture, OutputView, PartitionSink, StartupContext,
    Transport, TransportError, TransportHealth, TransportStatus,
};

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
    ///
    /// `readiness_notifier` is the relay's wakeup closure, taken here because the
    /// observer captures it during `startup`. It is optional rather than required:
    /// the delivery contract does not oblige a transport to have a notification
    /// path, and correctness never depends on one — the level the relay reads is
    /// authoritative, and a missing wakeup only defers a delivery to the next
    /// poll.
    ///
    /// `partition_sink` is required for the opposite reason. Tmux pastes a whole
    /// budget group in one injection, so its members share a fate; without a sink
    /// there would be no way to say so, and each member would be resolved from
    /// evidence about a write that was never its own.
    #[must_use]
    pub fn tmux(
        batch_settings: PromptBatchSettings,
        readiness_notifier: Option<crate::tmux::ReadinessNotifier>,
        partition_sink: Arc<dyn PartitionSink>,
    ) -> Self {
        Self::Tmux(TmuxTransport::new(
            batch_settings,
            readiness_notifier,
            partition_sink,
        ))
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
        partition_sink: Arc<dyn PartitionSink>,
    ) -> Self {
        Self::Pty(crate::pty::PtyTransport::new(
            target_member,
            config,
            mirror_state,
            partition_sink,
        ))
    }

    /// The transport declares its own packing units through its
    /// [`PartitionSink`], so the relay must not declare a singleton unit for a
    /// member it hands over.
    ///
    /// A member has exactly one write-once binding. A relay declaration would
    /// consume it and the transport's declaration for the group it actually
    /// writes would be refused, which under the contract means the transport
    /// produces no effect at all.
    ///
    /// Every coder transport now reports its own, so this separates them from UI,
    /// whose single member the relay declares because there is no coalescing to
    /// report. It began as scaffolding for adopting the sink one transport at a
    /// time and outlived that purpose: the distinction it draws is real.
    ///
    /// Two things it does not govern. Raw stays relay-declared whatever this
    /// says, because no transport can name the member at its raw write. And an
    /// un-admitted member — a terminal-outcome receipt — is declared by nobody;
    /// the relay skips it before consulting this, and each transport excludes it
    /// from its own declaration.
    #[must_use]
    pub fn reports_own_partition(&self) -> bool {
        match self {
            Self::Acp(_) | Self::Tmux(_) => true,
            Self::Ui(_) | Self::Pubsub => false,
            #[cfg(feature = "pty")]
            Self::Pty(_) => true,
            #[cfg(not(feature = "pty"))]
            Self::Pty => false,
        }
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

    /// Reports whether the selected transport can reach its target; see
    /// [`Transport::health`].
    #[must_use]
    pub fn health(&self) -> TransportHealth {
        match self {
            Self::Acp(transport) => transport.health(),
            Self::Tmux(transport) => transport.health(),
            Self::Ui(transport) => transport.health(),
            // A `Pubsub` target is rejected synchronously at admission and never
            // reaches a worker, so it has no target to be unreachable from.
            // Reporting `Unreachable` would start a dwell for a member that was
            // already answered.
            Self::Pubsub => TransportHealth::Healthy,
            #[cfg(feature = "pty")]
            Self::Pty(transport) => transport.health(),
            #[cfg(not(feature = "pty"))]
            Self::Pty => TransportHealth::Healthy,
        }
    }

    /// The selected transport's activity marker; see
    /// [`Transport::activity_generation`]. Tmux is the only variant that tracks
    /// one today, so every other arm reports the never-advancing `0`.
    #[must_use]
    pub fn activity_generation(&self) -> u64 {
        match self {
            Self::Tmux(transport) => transport.activity_generation(),
            Self::Acp(_) | Self::Ui(_) | Self::Pubsub => 0,
            #[cfg(feature = "pty")]
            Self::Pty(_) => 0,
            #[cfg(not(feature = "pty"))]
            Self::Pty => 0,
        }
    }

    /// Reports whether the selected transport can accept a handover now; see
    /// [`Transport::is_ready_for_handover`].
    #[must_use]
    pub async fn is_ready_for_handover(&self) -> bool {
        match self {
            Self::Acp(transport) => transport.is_ready_for_handover().await,
            Self::Tmux(transport) => transport.is_ready_for_handover().await,
            Self::Ui(transport) => transport.is_ready_for_handover().await,
            // The delivery worker latches a `Pubsub` stub for a configured Pubsub
            // target (delivery is guarded and answered with a not-implemented
            // outcome), so its query/lifecycle delegates must not panic. It is
            // never ready to deliver.
            Self::Pubsub => false,
            #[cfg(feature = "pty")]
            Self::Pty(transport) => transport.is_ready_for_handover().await,
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
            // configured Pubsub target and calls `shutdown()` on it whenever the
            // worker ends (`stop_drain`). `Pty` is not yet constructible when
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
