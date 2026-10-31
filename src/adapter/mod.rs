//! Deterministic reference adapter for the frozen agent-adapter wire.
//!
//! Implements the ADAPTER SIDE of the contract only: newline-delimited
//! JSON-RPC 2.0 over stdio, with handshake, submit, choice, look, persist,
//! and lifecycle traffic. The module uses only std plus serde/serde_json
//! for wire and product-boundary types; fixture parsing may use the
//! existing `toml` dependency. Product-boundary imports (relay, MCP,
//! transport, acp, tmux, pty) never cross here — opaque handles only, per
//! the contract's no-back-edge requirement, enforced by
//! `scripts/lint-delivery-protocol-boundary.py`.
//!
//! Submodules follow the design's nine-module mapping: `frame` owns
//! outbox bounds, queue caps, and the control lane; `choice` owns
//! originated-call in-flight caps and the pending-call table (`persist`
//! tracks its own writes); `lifecycle` owns ping/pong liveness and the
//! local diagnostic/inscription sink; per-type cancel/cleanup lives in
//! each method module, coordinated through `binding` teardown.

pub mod binding;
pub mod choice;
pub mod frame;
pub mod handshake;
pub mod lifecycle;
pub mod look;
pub mod persist;
pub mod script;
pub mod submit;
