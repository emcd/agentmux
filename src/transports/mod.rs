//! Transport abstraction for the relay delivery subsystem.
//!
//! See [`contract`] for the [`Transport`] trait, the [`TransportImpl`] dispatch
//! enum, and the shared delivery types. Concrete transport implementations land
//! in `src/acp/` (Slice 2) and `src/tmux/` (Slice 3).

pub mod contract;

pub use contract::{
    AcpTransport, DeliveryContext, DeliveryEnvelope, DeliveryResult, LookMode, LookSnapshotPayload,
    PermissionResponse, PromptReadinessTemplate, RawWriteResult, SingleDeliveryOutcome,
    StartupContext, TmuxTransport, Transport, TransportError, TransportEvent, TransportImpl,
    TransportReadiness, TransportStatus,
};
