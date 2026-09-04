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
//!   plus the look consumer ([`PtyOutputView`]) and the prompt-readiness
//!   predicate ([`prompt_satisfied`]).
//! - [`transport`] — `PtyTransport` facade (struct, `Transport`/`GenerationFence` impls, config types); `transport::lifecycle` owns bring-up (`startup_inner`, `StartupGuard`, bounded `observe_thread_finished`); `transport::runtime` owns the `!Send` terminal worker/reader threads (`run_worker`, `run_reader`, handlers).
//! - [`delivery`] holds this transport's half of the shared delivery-loop
//!   executor ([`delivery::PtyDeliveryWriter`]).

pub mod command;
pub mod delivery;
pub mod state;
pub mod transport;

pub use command::{CommandParseError, program_and_args, tokenize_command};
pub use state::{
    LOOK_LINES_DEFAULT, PtyConfigSnapshot, PtyOutputView, PtyShared, PtyState, SnapshotRequest,
    SnapshotResponse, prompt_satisfied,
};
pub use transport::{PtyMirrorStateFn, PtyTargetConfiguration, PtyTransport};
