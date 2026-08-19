//! The normalized relay-wide configuration, assembled once from the file and
//! the override layers.
//!
//! This is the object the rest of the relay reads. Everything below it — file
//! parsing, range validation, precedence — has already happened by the time one
//! exists, which is what lets consumers treat it as settled fact.

use crate::configuration::ConfigurationRoots;
use crate::relay::RelayError;

use super::delivery::DeliveryConfiguration;
use super::environment::{
    DEFAULT_REQUIRE_SESSION_CREDENTIALS, DEFAULT_WATCH_BUNDLES, ENV_REQUIRE_SESSION_CREDENTIALS,
    ENV_WATCH_BUNDLES, relay_bool_env_override, resolve_relay_bool_setting,
};
use super::relay_file::{PeerConfiguration, load_relay_file_configuration};

/// Fully-resolved relay-wide runtime configuration, after applying the
/// precedence ladder (CLI override > environment override > `relay.toml` >
/// documented defaults). This is the single normalized object relay startup and
/// `agentmux check configuration` read; consumers MUST NOT re-parse `relay.toml`
/// or re-apply defaulting/precedence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayRuntimeConfiguration {
    pub watch_bundles: bool,
    pub require_session_credentials: bool,
    pub choices_pending_max: usize,
    pub delivery: DeliveryConfiguration,
    pub peers: Vec<PeerConfiguration>,
}

/// Loads and resolves the relay-wide runtime configuration from
/// `<config-root>/relay.toml`, applying the precedence ladder
/// (CLI override > environment override > `relay.toml` > documented defaults)
/// for the boolean controls. A missing file yields the documented defaults; a
/// malformed file, unknown field, wrong field type, out-of-range choices bound,
/// invalid environment override, or invalid peer entry fails fast with a
/// structured [`RelayError`].
pub fn load_relay_runtime_configuration(
    configuration_roots: &ConfigurationRoots,
    cli_watch_bundles: Option<bool>,
    cli_require_session_credentials: Option<bool>,
) -> Result<RelayRuntimeConfiguration, RelayError> {
    let file = load_relay_file_configuration(configuration_roots)?;
    let environment_watch_bundles = relay_bool_env_override(ENV_WATCH_BUNDLES)?;
    let environment_require_session_credentials =
        relay_bool_env_override(ENV_REQUIRE_SESSION_CREDENTIALS)?;
    Ok(RelayRuntimeConfiguration {
        watch_bundles: resolve_relay_bool_setting(
            cli_watch_bundles,
            environment_watch_bundles,
            file.watch_bundles,
            DEFAULT_WATCH_BUNDLES,
        ),
        require_session_credentials: resolve_relay_bool_setting(
            cli_require_session_credentials,
            environment_require_session_credentials,
            file.require_session_credentials,
            DEFAULT_REQUIRE_SESSION_CREDENTIALS,
        ),
        choices_pending_max: file.choices_pending_max,
        delivery: file.delivery,
        peers: file.peers,
    })
}
