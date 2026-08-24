//! The normalized relay-wide configuration, assembled once from the file and
//! the override layers.
//!
//! This is the object the rest of the relay reads. Everything below it — file
//! parsing, range validation, precedence — has already happened by the time one
//! exists, which is what lets consumers treat it as settled fact.

use std::path::Path;

use serde_json::json;

use crate::configuration::ConfigurationRoots;
use crate::relay::RelayError;
use crate::runtime::paths::principal_store_path;

use super::super::constants::RELAY_NAMESPACE;
use super::super::identity::{PrincipalStore, PrincipalType};
use super::super::relay_error;

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

/// Verifies that every configured peer alias names a relay principal this relay
/// issued.
///
/// Kept separate from [`load_relay_runtime_configuration`] because it spans two
/// roots: peers resolve from the configuration roots, while the principal store
/// lives under the state root. Threading the state root into `relay.toml`
/// parsing would blur a separation the rest of the codebase maintains, so this
/// runs as its own step wherever both roots are already in hand — relay startup
/// and `agentmux check configuration`.
///
/// The check is deliberately unconditional. A peer with no store record cannot
/// be excused as one this relay merely dials and never hears from: absence is
/// equally consistent with a mistyped alias, one left stale by a
/// re-registration, or a row copied between deployments, and nothing local
/// distinguishes those from a legitimate dial-only peer. Excusing absence would
/// accept every one of them, which is the misconfiguration this exists to
/// reject.
///
/// Only the record's existence and principal type are checked. The outbound
/// credential at `<state-root>/peers/<alias>.psk` is deliberately not compared
/// against the record's credential hash: the peer issued this relay that
/// credential, while this relay issued the peer the recorded one, so requiring
/// them to agree would assert a relationship the data model does not record and
/// would fail every correct deployment.
///
/// # Errors
///
/// Returns [`RelayError`] naming the offending alias when it is absent from the
/// principal store or registered with a principal type other than relay.
pub fn validate_peer_aliases(
    peers: &[PeerConfiguration],
    state_root: &Path,
) -> Result<(), RelayError> {
    if peers.is_empty() {
        return Ok(());
    }
    let store = PrincipalStore::load(principal_store_path(state_root))?;
    for peer in peers {
        let principal_id = format!("{}@{RELAY_NAMESPACE}", peer.alias);
        let Some(record) = store.find_by_principal_id(principal_id.as_str()) else {
            return Err(relay_error(
                "validation_peer_alias_unregistered",
                "relay peer alias names no identity this relay issued; register the peer with \
                 'agentmux new peer <alias>@RELAY'",
                Some(json!({
                    "field": "peers.alias",
                    "value": peer.alias,
                    "principal_id": principal_id,
                })),
            ));
        };
        if record.principal_type != PrincipalType::Relay {
            return Err(relay_error(
                "validation_peer_alias_not_relay_principal",
                "relay peer alias names a registered principal which is not a peer relay",
                Some(json!({
                    "field": "peers.alias",
                    "value": peer.alias,
                    "principal_id": principal_id,
                    "principal_type": record.principal_type.as_str(),
                })),
            ));
        }
    }
    Ok(())
}
