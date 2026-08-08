//! Pty transport: libghostty-vt-backed delivery with portable-pty child
//! process management. Compiled only when the `pty` Cargo feature is
//! enabled; the default `cargo build` does NOT pull libghostty-vt or
//! portable-pty and does NOT invoke Zig.
//!
//! Build-time prerequisites (Zig 0.15.x on PATH, optional outbound
//! network access for the vendored ghostty clone) and the upstream
//! escape hatches (`GHOSTTY_SOURCE_DIR`, `GHOSTTY_ZIG_SYSTEM_DIR`,
//! `libghostty-vt-sys/pkg-config`) are documented in
//! `documentation/development/README.md` Zig-free Pty Builds.
//!
//! Module layout:
//! - [`state`] holds the cross-thread shared state ([`PtyShared`],
//!   [`PtyConfigSnapshot`], [`SnapshotRequest`] / [`SnapshotResponse`])
//!   plus the per-thread look / prompt consumer ([`PtyOutputView`],
//!   [`PtyPromptProbe`]).
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
    LOOK_LINES_DEFAULT, PtyConfigSnapshot, PtyOutputView, PtyPromptProbe, PtyShared, PtyState,
    SnapshotRequest, SnapshotResponse,
};
pub use transport::{PtyMirrorStateFn, PtyTargetConfiguration, PtyTransport};
