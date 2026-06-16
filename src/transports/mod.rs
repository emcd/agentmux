//! Transport abstraction for the relay delivery subsystem.
//!
//! See [`contract`] for the [`Transport`] trait, the [`TransportImpl`] dispatch
//! enum, and the shared delivery types. Concrete transport implementations land
//! in `src/acp/` (Slice 2) and `src/tmux/` (Slice 3).

pub mod contract;

pub use contract::{
    AcpTransport, ChoiceMade, ChoiceToMake, Chooser, DeliveryContext, DeliveryEnvelope,
    DeliveryResult, LookMode, LookSnapshotPayload, OutputView, PromptReadinessTemplate,
    RawWriteResult, SingleDeliveryOutcome, StartupContext, ThingToChoose, TmuxTransport, Transport,
    TransportError, TransportImpl, TransportReadiness, TransportStatus,
};
