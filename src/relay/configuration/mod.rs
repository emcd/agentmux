//! The relay's own runtime configuration: `relay.toml`, its override layers,
//! and the normalized object the rest of the relay reads.
//!
//! Distinct from [`crate::configuration`], which resolves the configuration
//! *roots* and the bundle, coder, and policy artifacts within them. This module
//! owns only what governs the relay process itself — whether it watches bundle
//! files, whether it requires session credentials, its delivery quotas and
//! timeouts, and the peer relays it dials.
//!
//! This module is an import-only hub, layered bottom-up:
//!
//! - [`delivery`]: the `[delivery]` table, its ranges, and the cross-key
//!   relations enforced at load.
//! - [`relay_file`]: parsing `relay.toml` into file-derived settings.
//! - [`environment`]: the override layers above the file and the precedence
//!   rule that ranks them.
//! - [`runtime`]: the normalized result the rest of the relay reads.
//!
//! Nothing is defined here — the root only wires submodules and re-exports.

mod delivery;
mod environment;
mod relay_file;
mod runtime;

pub use delivery::DeliveryConfiguration;
pub use environment::{parse_relay_bool_env_value, resolve_relay_bool_setting};
pub use relay_file::PeerConfiguration;
pub(in crate::relay) use relay_file::load_relay_file_configuration;
pub use runtime::{RelayRuntimeConfiguration, load_relay_runtime_configuration};
