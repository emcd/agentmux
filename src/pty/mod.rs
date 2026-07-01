//! Pty transport: libghostty-vt-backed delivery with portable-pty child
//! process management. Compiled only when the `pty` Cargo feature is
//! enabled; the default `cargo build` does NOT pull libghostty-vt or
//! portable-pty and does NOT invoke Zig.
//!
//! Module layout:
//! - [`state`] holds [`PtyState`] (the libghostty-vt terminal, render
//!   state, and config snapshot) and [`PtyOutputView`] (the
//!   [`OutputView`](crate::transports::OutputView) for the relay's look
//!   request path).
//! - [`transport`] holds [`PtyTransport`] (the per-target
//!   [`Transport`](crate::transports::Transport) implementation with its
//!   worker thread, delivery task, and reader thread).
//! - [`PtyQuiescenceProbe`](state::PtyQuiescenceProbe) adapts
//!   [`PtyState`] to the cross-transport
//!   [`WedgeProbe`](crate::transports::WedgeProbe) trait, so the shared
//!   wedge/prime state machine in
//!   [`crate::transports::quiescence`] drives Pty's quiescence wait.

pub mod state;
pub mod transport;
