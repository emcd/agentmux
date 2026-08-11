//! Tmux transport: pane operations, session lifecycle primitives, and the
//! [`Transport`](crate::transports::Transport) implementation.
//!
//! This module owns all tmux-process knowledge that previously lived scattered
//! across `relay/tmux.rs`, `relay/lifecycle.rs`, and relay delivery code. The relay delivery worker dispatches tmux
//! delivery generically through [`TmuxTransport`](transport::TmuxTransport); the
//! relay orchestration layer (bundle reconcile/startup/shutdown) calls the
//! lifecycle primitives in [`lifecycle`] directly.
//!
//! Dependency direction is downward only: this module depends on
//! `crate::transports`, `crate::configuration`, and `crate::runtime`, never on
//! `crate::relay`. The lifecycle primitives surface a transport-local
//! [`TmuxLifecycleError`](lifecycle::TmuxLifecycleError); relay maps it to its
//! own `RelayError` envelope at the orchestration boundary via a `From` impl
//! that lives in relay, so no tmux->relay back-edge is introduced.

pub mod lifecycle;
pub mod pane;
mod prompt_probe;
pub mod transport;

pub use prompt_probe::{PanePromptProbe, PromptReadinessEvaluation};
pub use transport::{
    ReadinessNotifier, TmuxOutputView, TmuxTransport, coalescing_runs, render_paste_text,
};
