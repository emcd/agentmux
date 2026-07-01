//! Pty transport core.
//!
//! [`PtyTransport`] is the per-target
//! [`Transport`](crate::transports::Transport) implementation. It owns:
//!
//! - One `libghostty_vt::Terminal<'static, 'static>` (the terminal is
//!   `!Send + !Sync`, so it lives on the delivery task's worker thread).
//! - One `portable_pty::PtyPair` master (split into reader + writer
//!   halves; the writer is moved to the delivery task, the reader feeds
//!   bytes into the terminal via a channel).
//! - One reader thread that loops on
//!   `pty_master.read() -> channel -> terminal.vt_write()`.
//! - One delivery task that drains the `mailw` / `raww` channel,
//!   renders pane-envelope text, writes to the master, and waits for
//!   quiescence via the shared wedge/prime state machine in
//!   [`crate::transports::quiescence`].
//! - One [`Arc<PtyOutputView>`](super::state::PtyOutputView) shared
//!   with the relay's look path.
//!
//! The shared state shape `Arc<Mutex<PtyState>>` is borrowed from
//! `headless-terminal`'s `internal/session/session.go`, which solves
//! the same "single-threaded libghostty-vt terminal, multi-threaded
//! look path" coordination problem with a single mutex.
