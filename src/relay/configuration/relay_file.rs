//! Parsing `relay.toml` into file-derived settings, before any override layer
//! is applied.
//!
//! Validation lives here rather than at the point of use so relay startup and
//! `agentmux check configuration` — which both load through this path — report
//! the same structured errors for the same file.

use std::{collections::HashSet, fs, path::Path};

use serde::Deserialize;
use serde_json::json;

use crate::configuration::{ConfigurationRoots, relay_configuration_path};
use crate::relay::{RelayError, errors::map_config, relay_error};

use super::delivery::{
    DeliveryConfiguration, RawRelayDeliverySection, resolve_delivery_configuration,
};

const DEFAULT_CHOICES_PENDING_MAX: usize = 256;
const MIN_CHOICES_PENDING_MAX: usize = 1;
const MAX_CHOICES_PENDING_MAX: usize = 4096;

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawRelayFile {
    #[serde(default)]
    watch_bundles: Option<bool>,
    #[serde(default)]
    require_session_credentials: Option<bool>,
    #[serde(default)]
    choices: Option<RawRelayChoicesSection>,
    #[serde(default)]
    delivery: Option<RawRelayDeliverySection>,
    #[serde(default)]
    peers: Vec<RawPeerEntry>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawRelayChoicesSection {
    #[serde(default)]
    pending_max: Option<usize>,
}

/// Raw `[[peers]]` entry: an active outbound peer relay endpoint.
///
/// `alias` is this relay's *local* name for the peer — the bang-path `!<alias>`
/// routing selector and the peer credential filename stem, internal to us and
/// never seen by the peer. It is not free-form: it MUST be the identity *we*
/// issued that peer via our own `new peer <alias>@RELAY`, so that a peer
/// authenticating inbound is named by the principal we just verified rather than
/// by a second name nothing relates to it. Checked at load against the principal
/// store. `connect-as` is the identity the peer issued us via its
/// `new peer <connect-as>@RELAY` — presented in Hello when we dial it, since each
/// peer determines the identity it expects from us (two peers can issue different
/// or even colliding identities to this relay). `address` is the peer's listening
/// endpoint — in this slice an absolute Unix domain socket path (TCP is future).
/// The table is outbound-only and carries no `scope`: inbound cross-relay
/// authorization is the scope this relay sets via `new peer <id>@RELAY --scope`
/// and reads through the ingress filter, so `deny_unknown_fields` rejects a stray
/// `scope` key here.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawPeerEntry {
    alias: String,
    address: String,
    connect_as: String,
}

/// A validated `[[peers]]` entry naming an outbound peer relay.
///
/// `alias` is this relay's local name for the peer, and is the identity this
/// relay issued that peer: the bang-path `!<alias>` routing selector and the
/// `<peer_alias>` credential filename stem. `connect_as`
/// is the identity the peer issued us, presented as `<connect_as>@RELAY` in Hello
/// when we dial it. `address` is an absolute Unix domain socket path. The
/// presented identity is per-peer because the *receiver* determines it (via its
/// `new peer`), so no single relay-wide identity exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerConfiguration {
    pub alias: String,
    pub address: String,
    pub connect_as: String,
}

/// File-derived relay settings before override resolution. `watch_bundles` and
/// `require_session_credentials` stay `Option` so the precedence ladder can tell
/// "absent in file" apart from an explicit value; `choices_pending_max` and
/// `peers` have no override layer and are resolved (and validated) here.
pub(in crate::relay) struct RelayFileConfiguration {
    pub(in crate::relay) watch_bundles: Option<bool>,
    pub(in crate::relay) require_session_credentials: Option<bool>,
    pub(in crate::relay) choices_pending_max: usize,
    pub(in crate::relay) delivery: DeliveryConfiguration,
    pub(in crate::relay) peers: Vec<PeerConfiguration>,
}

/// Parses and validates `relay.toml` into file-derived settings without applying
/// overrides. A missing file yields documented defaults with no configured
/// peers. Choices range and peer-entry validation happen here so both relay
/// startup and `agentmux check configuration` (which loads through this path)
/// report the same structured errors.
pub(in crate::relay) fn load_relay_file_configuration(
    configuration_roots: &ConfigurationRoots,
) -> Result<RelayFileConfiguration, RelayError> {
    let path = relay_configuration_path(configuration_roots).map_err(map_config)?;
    if !path.exists() {
        return Ok(RelayFileConfiguration {
            watch_bundles: None,
            require_session_credentials: None,
            choices_pending_max: DEFAULT_CHOICES_PENDING_MAX,
            delivery: DeliveryConfiguration::default(),
            peers: Vec::new(),
        });
    }
    let raw = fs::read_to_string(&path).map_err(|source| {
        relay_error(
            "validation_invalid_arguments",
            "failed to load relay configuration",
            Some(json!({
                "path": path.display().to_string(),
                "cause": source.to_string(),
            })),
        )
    })?;
    let parsed = toml::from_str::<RawRelayFile>(raw.as_str()).map_err(|source| {
        relay_error(
            "validation_invalid_arguments",
            "failed to parse relay configuration",
            Some(json!({
                "path": path.display().to_string(),
                "cause": source.to_string(),
            })),
        )
    })?;
    let delivery = resolve_delivery_configuration(parsed.delivery, path.as_path())?;
    let choices_pending_max = parsed
        .choices
        .and_then(|choices| choices.pending_max)
        .unwrap_or(DEFAULT_CHOICES_PENDING_MAX);
    if !(MIN_CHOICES_PENDING_MAX..=MAX_CHOICES_PENDING_MAX).contains(&choices_pending_max) {
        return Err(relay_error(
            "validation_invalid_arguments",
            "relay choices pending-max is out of supported range",
            Some(json!({
                "path": path.display().to_string(),
                "field": "choices.pending-max",
                "value": choices_pending_max,
                "minimum": MIN_CHOICES_PENDING_MAX,
                "maximum": MAX_CHOICES_PENDING_MAX,
            })),
        ));
    }
    let mut peers = Vec::with_capacity(parsed.peers.len());
    // The alias is this relay's local name for the peer: it is the bang-path
    // selector (`!<alias>`) and the credential filename stem
    // (`<state-root>/peers/<alias>.psk`), so it MUST be unique. Two entries
    // sharing an alias would silently collapse to one routable endpoint (the
    // connection manager keys on alias), with the surviving one decided by
    // insertion order rather than operator intent — so reject the collision
    // fail-fast at load. Duplicate `connect-as` stays allowed: the presented
    // identity is receiver-issued and two peers may legitimately issue this
    // relay the same one.
    let mut seen_aliases: HashSet<String> = HashSet::with_capacity(peers.capacity());
    for (index, peer) in parsed.peers.into_iter().enumerate() {
        let alias =
            validate_peer_id_token(peer.alias.as_str(), "peers.alias", index, path.as_path())?;
        if !seen_aliases.insert(alias.clone()) {
            return Err(relay_error(
                "validation_invalid_arguments",
                "relay peer alias must be unique across all [[peers]] entries",
                Some(json!({
                    "path": path.display().to_string(),
                    "field": "peers.alias",
                    "peer_index": index,
                    "value": alias,
                })),
            ));
        }
        let connect_as = validate_peer_id_token(
            peer.connect_as.as_str(),
            "peers.connect-as",
            index,
            path.as_path(),
        )?;
        let address = peer.address.trim();
        if address.is_empty() {
            return Err(relay_error(
                "validation_invalid_arguments",
                "relay peer address must be a non-empty string",
                Some(json!({
                    "path": path.display().to_string(),
                    "field": "peers.address",
                    "peer_index": index,
                })),
            ));
        }
        // The relay serves only a Unix domain socket today, so a peer endpoint is
        // an absolute filesystem path. Reject a relative path or a TCP-style
        // `host:port` form (which is not absolute) with a pointed message; remote
        // peering is deferred (see the change's Non-Goals).
        if !Path::new(address).is_absolute() {
            return Err(relay_error(
                "validation_invalid_arguments",
                "relay peer address must be an absolute Unix socket path (TCP host:port is not yet supported)",
                Some(json!({
                    "path": path.display().to_string(),
                    "field": "peers.address",
                    "peer_index": index,
                    "value": address,
                })),
            ));
        }
        peers.push(PeerConfiguration {
            alias,
            address: address.to_string(),
            connect_as,
        });
    }
    Ok(RelayFileConfiguration {
        watch_bundles: parsed.watch_bundles,
        require_session_credentials: parsed.require_session_credentials,
        choices_pending_max,
        delivery,
        peers,
    })
}

/// Validates a peer id token — a bang-path `!<alias>` selector or a `connect-as`
/// identity local part. The grammar is deliberately strict: the `alias` becomes a
/// credential filename stem (`<alias>.psk`) and the bang-path selector, and the
/// `connect-as` is qualified to `<connect-as>@RELAY` when presented, so both must
/// be non-empty after trimming and free of the `@` namespace qualifier, the `!`
/// bang-path separator, and any path separator. `field` names the offending key
/// for the error. Returns the trimmed token.
fn validate_peer_id_token(
    raw: &str,
    field: &str,
    peer_index: usize,
    path: &Path,
) -> Result<String, RelayError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(relay_error(
            "validation_invalid_arguments",
            "relay peer id token must be non-empty",
            Some(json!({
                "path": path.display().to_string(),
                "field": field,
                "peer_index": peer_index,
            })),
        ));
    }
    if value.contains('@') || value.contains('!') || value.contains('/') {
        return Err(relay_error(
            "validation_invalid_arguments",
            "relay peer id token must not contain '@', '!', or a path separator",
            Some(json!({
                "path": path.display().to_string(),
                "field": field,
                "peer_index": peer_index,
                "value": value,
            })),
        ));
    }
    Ok(value.to_string())
}
