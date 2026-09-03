//! Tmux's name for the receipt barrier it shares with ACP.
//!
//! The rule itself lives in the transport contract, because two transports
//! reached it independently and state it identically. This re-export keeps the
//! tmux-facing name the module's own tests and callers already use.

pub use crate::transports::receipt_runs as coalescing_runs;
