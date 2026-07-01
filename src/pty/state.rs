//! Pty transport state and shared view.
//!
//! [`PtyState`] holds the libghostty-vt terminal, render state, and the
//! per-coder config snapshot (cols, rows, prompt regex, prime timeout,
//! wedge detection). The terminal is `!Send + !Sync`, so the state lives
//! behind an `Arc<Mutex<...>>` shared between the delivery task (which
//! owns the reader thread and feeds bytes into the terminal) and the
//! look path (which locks the mutex briefly to render a snapshot).
//!
//! [`PtyOutputView`] is the [`OutputView`](crate::transports::OutputView)
//! handle the relay's look request path reads without borrowing the
//! worker-owned transport. It captures a screen snapshot via
//! `Formatter::format_alloc(Format::Plain)` and reports cursor
//! position + visibility.
//!
//! [`PtyQuiescenceProbe`] adapts [`PtyState`] to the cross-transport
//! [`WedgeProbe`](crate::transports::WedgeProbe) trait so the shared
//! state machine in [`crate::transports::quiescence`] can drive Pty's
//! quiescence wait using the same wedge/prime-timeout semantics as Tmux.
//!
//! Implementation lands once the transport core populates the fields
//! these structs describe.
