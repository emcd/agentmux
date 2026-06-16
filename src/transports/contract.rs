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
//! ## Slice status
//!
//! This module is the Slice 1 scaffold: the trait, the dispatch enum, the
//! placeholder transport structs, and the shared types. The placeholder
//! [`Transport`] implementations have `todo!()` bodies; the real bodies arrive
//! when ACP delivery moves here (Slice 2) and Tmux delivery moves here
//! (Slice 3). Nothing constructs or calls these types yet, so there is no
//! behavior change.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use serde_json::Value;

use crate::acp::{AcpSnapshotEntry, PermissionRequest, ReplayEntry};
use crate::configuration::BundleMember;
// Re-export the configuration prompt-readiness template into the transport
// contract namespace; tmux quiescence consumes it and Slice 3 wires it through
// the delivery context. It is defined once in `configuration`; re-exporting
// (rather than redefining) keeps the two in lockstep.
pub use crate::configuration::PromptReadinessTemplate;
// Reused delivery wire vocabulary. These enums currently live in the relay
// contract; the transport layer reuses them rather than duplicating the serde
// shapes. Relocating their canonical home into `transports` is a relay-contract
// move deferred beyond this additive slice.
use crate::relay::{DeliveryPayloadMode, SendOutcome};

/// Synchronous delivery contract implemented by each concrete transport.
///
/// The relay worker wraps these calls in `spawn_blocking` where needed; an
/// implementation MUST NOT impose async boundaries on the worker.
pub trait Transport {
    /// Establishes (or re-establishes, on respawn) the transport runtime for a
    /// target. Each call invalidates any previously issued [`inbound`] channel;
    /// the worker re-calls [`inbound`] afterward to pick up the fresh receiver.
    ///
    /// [`inbound`]: Transport::inbound
    fn startup(&mut self, context: StartupContext) -> Result<TransportStatus, TransportError>;

    /// Delivers a coalesced batch of envelopes in a single transport call,
    /// returning one outcome per envelope aligned with the input order.
    fn deliver(
        &mut self,
        envelopes: Vec<DeliveryEnvelope>,
        context: &DeliveryContext,
    ) -> DeliveryResult;

    /// Captures a snapshot of the target's current output for `look`.
    fn look(&self, mode: LookMode) -> Result<LookSnapshotPayload, TransportError>;

    /// Reports whether the transport is ready to accept delivery.
    fn is_ready(&self) -> bool;

    /// Injects raw input (no envelope framing) for `raww`.
    fn raw_write(
        &mut self,
        text: &str,
        append_enter: bool,
        context: &DeliveryContext,
    ) -> RawWriteResult;

    /// Resolves a pending permission request raised by the transport.
    fn resolve_permission(
        &mut self,
        request: PermissionRequest,
        response: PermissionResponse,
    ) -> Result<(), TransportError>;

    /// Tears down the transport runtime, releasing its resources.
    fn shutdown(&mut self);

    /// Reports how many envelopes the transport accepts per [`deliver`] call.
    /// Tmux returns `1` (one paste per call); ACP accepts the full batch.
    ///
    /// [`deliver`]: Transport::deliver
    fn accept_capacity(&self) -> usize;

    /// Hands the worker the receiver for this transport's inbound events, or
    /// `None` for transports without a push-based inbound stream (Tmux).
    ///
    /// The transport owns the `Sender` internally and creates a fresh channel
    /// in [`startup`]. A `None` yielded by polling the receiver signals an
    /// expected respawn (the old `Sender` dropped), not a delivery error: the
    /// worker re-subscribes by re-calling this method.
    ///
    /// [`startup`]: Transport::startup
    fn inbound(&mut self) -> Option<Receiver<TransportEvent>>;
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

    /// Captures a snapshot; see [`Transport::look`].
    pub fn look(&self, mode: LookMode) -> Result<LookSnapshotPayload, TransportError> {
        match self {
            Self::Acp(transport) => transport.look(mode),
            Self::Tmux(transport) => transport.look(mode),
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

    /// Resolves a permission request; see [`Transport::resolve_permission`].
    pub fn resolve_permission(
        &mut self,
        request: PermissionRequest,
        response: PermissionResponse,
    ) -> Result<(), TransportError> {
        match self {
            Self::Acp(transport) => transport.resolve_permission(request, response),
            Self::Tmux(transport) => transport.resolve_permission(request, response),
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

    /// Hands over the inbound event receiver; see [`Transport::inbound`].
    pub fn inbound(&mut self) -> Option<Receiver<TransportEvent>> {
        match self {
            Self::Acp(transport) => transport.inbound(),
            Self::Tmux(transport) => transport.inbound(),
            Self::Pty => unimplemented!("PTY transport not yet implemented"),
        }
    }
}

/// ACP delivery transport. Placeholder for Slice 1; the implementation moves
/// here from `relay/delivery/acp_delivery.rs` in Slice 2.
#[derive(Debug, Default)]
pub struct AcpTransport;

/// Tmux pane delivery transport. Placeholder for Slice 1; the implementation
/// moves here from `relay/tmux.rs` plus the quiescence loop in Slice 3.
#[derive(Debug, Default)]
pub struct TmuxTransport;

impl Transport for AcpTransport {
    fn startup(&mut self, _context: StartupContext) -> Result<TransportStatus, TransportError> {
        todo!("AcpTransport::startup — Slice 2")
    }

    fn deliver(
        &mut self,
        _envelopes: Vec<DeliveryEnvelope>,
        _context: &DeliveryContext,
    ) -> DeliveryResult {
        todo!("AcpTransport::deliver — Slice 2")
    }

    fn look(&self, _mode: LookMode) -> Result<LookSnapshotPayload, TransportError> {
        todo!("AcpTransport::look — Slice 2")
    }

    fn is_ready(&self) -> bool {
        todo!("AcpTransport::is_ready — Slice 2")
    }

    fn raw_write(
        &mut self,
        _text: &str,
        _append_enter: bool,
        _context: &DeliveryContext,
    ) -> RawWriteResult {
        todo!("AcpTransport::raw_write — Slice 2")
    }

    fn resolve_permission(
        &mut self,
        _request: PermissionRequest,
        _response: PermissionResponse,
    ) -> Result<(), TransportError> {
        todo!("AcpTransport::resolve_permission — Slice 2")
    }

    fn shutdown(&mut self) {
        todo!("AcpTransport::shutdown — Slice 2")
    }

    fn accept_capacity(&self) -> usize {
        todo!("AcpTransport::accept_capacity — Slice 2")
    }

    fn inbound(&mut self) -> Option<Receiver<TransportEvent>> {
        todo!("AcpTransport::inbound — Slice 2")
    }
}

impl Transport for TmuxTransport {
    fn startup(&mut self, _context: StartupContext) -> Result<TransportStatus, TransportError> {
        todo!("TmuxTransport::startup — Slice 3")
    }

    fn deliver(
        &mut self,
        _envelopes: Vec<DeliveryEnvelope>,
        _context: &DeliveryContext,
    ) -> DeliveryResult {
        todo!("TmuxTransport::deliver — Slice 3")
    }

    fn look(&self, _mode: LookMode) -> Result<LookSnapshotPayload, TransportError> {
        todo!("TmuxTransport::look — Slice 3")
    }

    fn is_ready(&self) -> bool {
        todo!("TmuxTransport::is_ready — Slice 3")
    }

    fn raw_write(
        &mut self,
        _text: &str,
        _append_enter: bool,
        _context: &DeliveryContext,
    ) -> RawWriteResult {
        todo!("TmuxTransport::raw_write — Slice 3")
    }

    fn resolve_permission(
        &mut self,
        _request: PermissionRequest,
        _response: PermissionResponse,
    ) -> Result<(), TransportError> {
        todo!("TmuxTransport::resolve_permission — Slice 3")
    }

    fn shutdown(&mut self) {
        todo!("TmuxTransport::shutdown — Slice 3")
    }

    fn accept_capacity(&self) -> usize {
        todo!("TmuxTransport::accept_capacity — Slice 3")
    }

    fn inbound(&mut self) -> Option<Receiver<TransportEvent>> {
        todo!("TmuxTransport::inbound — Slice 3")
    }
}

/// Inputs required to establish a transport runtime for one target.
#[derive(Clone, Debug)]
pub struct StartupContext {
    pub bundle_name: String,
    pub runtime_directory: PathBuf,
    pub target_member: BundleMember,
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

/// A push-based inbound event raised by a transport's runtime.
#[derive(Clone, Debug)]
pub enum TransportEvent {
    /// New replay-buffer entries (ACP stream output).
    ReplayEntries(Vec<ReplayEntry>),
    /// A pending permission request awaiting operator/UI resolution (ACP).
    PermissionRequested(PermissionRequest),
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

/// Windowing parameters for a [`Transport::look`] snapshot.
#[derive(Clone, Copy, Debug, Default)]
pub struct LookMode {
    /// Window size (tmux pane lines or ACP replay entries).
    pub lines: Option<u64>,
    /// Entries to skip from the newest end before the tail window (ACP only).
    pub offset: Option<u64>,
}

/// Transport-level snapshot payload returned by [`Transport::look`].
///
/// The relay maps this onto its wire `LookSnapshotPayload`, adding ACP
/// freshness/source metadata. Kept transport-local so the transport layer does
/// not depend on the relay wire contract.
#[derive(Clone, Debug)]
pub enum LookSnapshotPayload {
    /// Plain text lines (tmux).
    Lines { snapshot_lines: Vec<String> },
    /// Rendered ACP replay entries plus truncation bookkeeping.
    AcpEntries {
        snapshot_entries: Vec<AcpSnapshotEntry>,
        /// Total entries available before tail/offset windowing.
        entries_total: usize,
        /// Count actually returned after the tail-N window and `offset`.
        returned_entries_count: usize,
    },
}

/// An operator/UI decision resolving a [`PermissionRequest`].
#[derive(Clone, Debug)]
pub enum PermissionResponse {
    /// The named option was selected.
    Selected { option_id: String },
    /// The request was cancelled without a selection.
    Cancelled,
}
