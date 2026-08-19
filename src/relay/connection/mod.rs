//! Serving one relay socket connection, split along the frame boundary.
//!
//! This module is an import-only hub. The pieces below run in the order a
//! connection meets them:
//!
//! - [`context`]: the connection-independent handles a connection is served
//!   against, assembled once by the host and cloned per connection.
//! - [`serve`]: one connection's lifetime — the registration drop-guard, the
//!   frame loop, and the state a Hello establishes for the frames after it.
//! - [`framing`]: line reads off the socket half and the blocking-pool hand-off,
//!   both below frame semantics.
//! - [`hello`]: verifying an identity and claiming the registry entry for it.
//! - [`requests`]: gating a request against the registration and routing it to
//!   the dispatcher its operation belongs to.
//! - [`helpers`]: the namespace-routing, principal-resolution, and error-shaping
//!   free functions the two frame handlers share.
//!
//! Nothing is defined here — the root only wires submodules and re-exports the
//! relay-facing API.

mod context;
mod framing;
mod hello;
pub(super) mod helpers;
mod requests;
mod serve;

pub use context::ConnectionServeContext;
pub use serve::serve_connection;
