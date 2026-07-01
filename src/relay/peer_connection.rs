//! Outbound peer relay connection management.
//!
//! Owns the lazily-established outbound connections this relay uses to forward a
//! cross-relay `Send`/`Raww` to a configured peer. A connection is opened on the
//! first delivery to a given peer — never eagerly at startup — so an unreachable
//! peer neither blocks nor destabilizes relay boot: it surfaces only as a typed
//! delivery outcome on the affected request.
//!
//! To dial a peer the manager reads the outbound PSK from the well-known peer
//! credential path `<state-root>/peers/<peer_alias>.psk`, dials the peer's
//! configured Unix socket `address`, and presents a Hello as this relay's own
//! `<relay-id>@RELAY` principal (see [`RelayStreamSession::for_peer_relay`]) with
//! that PSK. The reachability/authentication handshake and its jittered backoff
//! are reused from the shared stream client; this module adds only the
//! per-peer identity, credential lookup, and typed failure classification.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde_json::json;

use crate::runtime::paths::peer_relay_psk_path;

use super::authorization::PeerConfiguration;
use super::client::RelayStreamSession;
use super::{RELAY_NAMESPACE, RelayError, RelayRequest, RelayResponse, relay_error};

/// One configured outbound peer endpoint.
#[derive(Clone, Debug)]
struct PeerEndpoint {
    /// The peer's bare id — the bang-path `!<relay_id>` selector and the
    /// `<peer_alias>` credential filename stem.
    alias: String,
    /// The peer's listening Unix domain socket path.
    address: PathBuf,
}

/// Manages this relay's outbound connections to its configured peer relays.
///
/// Shared (behind an `Arc`) across connection-handler tasks like the bundle
/// catalog. Connections are established lazily and held per peer; a per-peer
/// mutex serializes dials to one peer without blocking deliveries to others.
pub struct PeerConnectionManager {
    /// This relay's own bare id, presented as `<relay-id>@RELAY` when dialing a
    /// peer. Present iff any peer is configured (guaranteed by relay.toml
    /// validation); `None` on a relay with no peers.
    relay_id: Option<String>,
    state_root: PathBuf,
    endpoints: HashMap<String, PeerEndpoint>,
    sessions: HashMap<String, Mutex<Option<RelayStreamSession>>>,
}

impl PeerConnectionManager {
    /// Builds the manager from the resolved relay runtime configuration. Peer
    /// entries are keyed by their bare id (alias); the address is validated as an
    /// absolute Unix socket path at configuration load, so it is trusted here.
    #[must_use]
    pub fn from_configuration(
        relay_id: Option<String>,
        state_root: &Path,
        peers: &[PeerConfiguration],
    ) -> Self {
        let mut endpoints = HashMap::with_capacity(peers.len());
        let mut sessions = HashMap::with_capacity(peers.len());
        for peer in peers {
            let alias = peer.alias().to_string();
            endpoints.insert(
                alias.clone(),
                PeerEndpoint {
                    alias: alias.clone(),
                    address: PathBuf::from(peer.address.as_str()),
                },
            );
            sessions.insert(alias, Mutex::new(None));
        }
        Self {
            relay_id,
            state_root: state_root.to_path_buf(),
            endpoints,
            sessions,
        }
    }

    /// Whether any outbound peer is configured. A relay with no peers holds an
    /// empty manager and never dials.
    #[must_use]
    pub fn is_configured(&self) -> bool {
        !self.endpoints.is_empty()
    }

    /// This relay's own outbound identity local part, presented as
    /// `<relay-id>@RELAY` to a peer. The forwarding handler uses it to set the
    /// forwarded request's `requester_session` to this relay's authenticated
    /// principal (the peer sees this relay as the sender; original-sender
    /// attribution via `on_behalf_of` is deferred). `None` when no peers are
    /// configured.
    #[must_use]
    pub fn own_relay_principal_id(&self) -> Option<String> {
        self.relay_id
            .as_deref()
            .map(|relay_id| format!("{relay_id}@{RELAY_NAMESPACE}"))
    }

    /// Forwards a fully-formed request to the peer named by `relay_id` and
    /// returns its response, establishing the connection lazily. Transport and
    /// handshake failures map to the same typed errors as [`connect`]; a peer
    /// that answers (including with an error response) returns that response for
    /// the caller to propagate as the delivery outcome.
    ///
    /// [`connect`]: PeerConnectionManager::connect
    pub fn forward(
        &self,
        relay_id: &str,
        request: &RelayRequest,
    ) -> Result<RelayResponse, RelayError> {
        let prepared = self.prepare(relay_id)?;
        let mut slot = prepared
            .session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::session_mut(&mut slot, &prepared)
            .request(request)
            .map_err(|source| prepared.unavailable_error(&source))
    }

    /// Establishes (or reuses) the outbound connection to the peer named by the
    /// bang-path `relay_id`, presenting this relay's `<relay-id>@RELAY` identity
    /// with the peer PSK. Returns a typed [`RelayError`] the caller maps to a
    /// delivery outcome:
    ///
    /// - `validation_unknown_peer` — no `[[peers]]` entry matches `relay_id`;
    /// - `runtime_peer_credential_missing` — the peer PSK file is absent, empty,
    ///   or unreadable;
    /// - `runtime_peer_unavailable` — the endpoint could not be dialed or the
    ///   Hello handshake failed.
    ///
    /// None of these fail relay startup — the connection is lazy, so every
    /// failure is scoped to the affected delivery.
    pub fn connect(&self, relay_id: &str) -> Result<(), RelayError> {
        let prepared = self.prepare(relay_id)?;
        let mut slot = prepared
            .session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::session_mut(&mut slot, &prepared)
            .connect()
            .map_err(|source| prepared.unavailable_error(&source))
    }

    /// Validates the peer endpoint, this relay's identity, and the peer
    /// credential, returning the per-peer session slot plus the parameters a
    /// lazy dial needs. Shared by [`connect`] and [`forward`] so the two agree on
    /// classification and credential handling.
    ///
    /// [`connect`]: PeerConnectionManager::connect
    /// [`forward`]: PeerConnectionManager::forward
    fn prepare(&self, relay_id: &str) -> Result<PreparedDial<'_>, RelayError> {
        let endpoint = self.endpoints.get(relay_id).ok_or_else(|| {
            relay_error(
                "validation_unknown_peer",
                "no configured peer relay matches the target relay id",
                Some(json!({ "relay_id": relay_id })),
            )
        })?;
        let own_relay_id = self.relay_id.as_deref().ok_or_else(|| {
            relay_error(
                "internal_peer_identity_missing",
                "relay-id is not configured; cannot present an outbound peer identity",
                Some(json!({ "relay_id": relay_id })),
            )
        })?;
        let token = self.read_peer_credential(endpoint.alias.as_str())?;
        let session = self
            .sessions
            .get(relay_id)
            .expect("session slot exists for every configured peer");
        Ok(PreparedDial {
            relay_id: relay_id.to_string(),
            endpoint,
            own_relay_id: own_relay_id.to_string(),
            token,
            session,
        })
    }

    /// Returns the peer's session, initializing it lazily on first use.
    fn session_mut<'slot>(
        slot: &'slot mut Option<RelayStreamSession>,
        prepared: &PreparedDial<'_>,
    ) -> &'slot mut RelayStreamSession {
        slot.get_or_insert_with(|| {
            RelayStreamSession::for_peer_relay(
                prepared.endpoint.address.clone(),
                prepared.own_relay_id.clone(),
                prepared.token.clone(),
            )
        })
    }

    /// Reads the outbound PSK for a peer from `<state-root>/peers/<alias>.psk`.
    fn read_peer_credential(&self, alias: &str) -> Result<String, RelayError> {
        let path = peer_relay_psk_path(&self.state_root, alias);
        let credential_error = |cause: &str| {
            relay_error(
                "runtime_peer_credential_missing",
                "peer relay credential is absent or unreadable",
                Some(json!({
                    "peer_alias": alias,
                    "path": path.display().to_string(),
                    "cause": cause,
                })),
            )
        };
        match fs::read_to_string(&path) {
            Ok(contents) => {
                let trimmed = contents.trim();
                if trimmed.is_empty() {
                    Err(credential_error("credential file is empty"))
                } else {
                    Ok(trimmed.to_string())
                }
            }
            Err(source) => Err(credential_error(source.to_string().as_str())),
        }
    }
}

/// The validated inputs a lazy dial needs, borrowed from the manager for the
/// duration of one [`PeerConnectionManager::connect`] or
/// [`PeerConnectionManager::forward`] call.
struct PreparedDial<'a> {
    relay_id: String,
    endpoint: &'a PeerEndpoint,
    own_relay_id: String,
    token: String,
    session: &'a Mutex<Option<RelayStreamSession>>,
}

impl PreparedDial<'_> {
    fn unavailable_error(&self, source: &std::io::Error) -> RelayError {
        relay_error(
            "runtime_peer_unavailable",
            "outbound peer relay connection could not be established",
            Some(json!({
                "relay_id": self.relay_id,
                "address": self.endpoint.address.display().to_string(),
                "cause": source.to_string(),
            })),
        )
    }
}
