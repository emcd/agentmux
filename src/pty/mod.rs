//! Pty transport: libghostty-vt-backed delivery with portable-pty child
//! process management. Compiled only when the `pty` Cargo feature is
//! enabled; the default `cargo build` does NOT pull libghostty-vt or
//! portable-pty and does NOT invoke Zig.
//!
//! Module layout:
//! - [`state`] holds the cross-thread shared state ([`PtyShared`],
//!   [`PtyConfigSnapshot`], [`SnapshotRequest`] / [`SnapshotResponse`])
//!   plus the per-thread look / probe consumers ([`PtyOutputView`],
//!   [`PtyQuiescenceProbe`]).
//! - [`transport`] holds [`PtyTransport`] (the per-target
//!   [`Transport`](crate::transports::Transport) implementation with its
//!   worker thread, delivery task, and reader thread) plus
//!   [`PtyTargetConfiguration`] (the per-coder config bundle).

pub mod command;
pub mod delivery;
pub mod state;
pub mod transport;

pub use command::{CommandParseError, program_and_args, tokenize_command};
pub use state::{
    LOOK_LINES_DEFAULT, PtyConfigSnapshot, PtyOutputView, PtyQuiescenceProbe, PtyShared, PtyState,
    SnapshotRequest, SnapshotResponse,
};
pub use transport::{PtyTargetConfiguration, PtyTransport};
