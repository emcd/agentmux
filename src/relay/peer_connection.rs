//! Outbound peer relay connection management.
//!
//! Owns the lazily-established outbound connections this relay uses to forward a
//! cross-relay `Send`/`Raww` to a configured peer. A connection is opened on the
//! first delivery to a given peer — never eagerly at startup — so an unreachable
//! peer neither blocks nor destabilizes relay boot: it surfaces only as a typed
//! delivery outcome on the affected request.
//!
//! To dial a peer the manager reads the outbound PSK from the well-known peer
//! credential path `<state-root>/peers/<alias>.psk`, dials the peer's configured
//! Unix socket `address`, and presents a Hello as the `<connect_as>@RELAY`
//! principal that peer issued this relay (see [`RelayStreamSession::for_peer_relay`])
//! with that PSK. The presented identity is per-peer because the receiving relay
//! determines it via its own `new peer`. The reachability/authentication
//! handshake and its jittered backoff are reused from the shared stream client;
//! this module adds only the per-peer identity, credential lookup, and typed
//! failure classification.

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
    /// This relay's local name for the peer — the bang-path `!<alias>` selector
    /// and the `<peer_alias>` credential filename stem.
    alias: String,
    /// The peer's listening Unix domain socket path.
    address: PathBuf,
    /// The identity the peer issued this relay, presented as `<connect_as>@RELAY`
    /// in Hello when dialing it. Per-peer because the receiver determines it.
    connect_as: String,
}

/// Manages this relay's outbound connections to its configured peer relays.
///
/// Shared (behind an `Arc`) across connection-handler tasks like the bundle
/// catalog. Connections are established lazily and held per peer; a per-peer
/// mutex serializes dials to one peer without blocking deliveries to others.
#[derive(Debug)]
pub struct PeerConnectionManager {
    state_root: PathBuf,
    endpoints: HashMap<String, PeerEndpoint>,
    sessions: HashMap<String, Mutex<Option<RelayStreamSession>>>,
}

impl PeerConnectionManager {
    /// Builds the manager from the resolved relay runtime configuration. Peer
    /// entries are keyed by their local `alias`; the address is validated as an
    /// absolute Unix socket path at configuration load, so it is trusted here.
    #[must_use]
    pub fn from_configuration(state_root: &Path, peers: &[PeerConfiguration]) -> Self {
        let mut endpoints = HashMap::with_capacity(peers.len());
        let mut sessions = HashMap::with_capacity(peers.len());
        for peer in peers {
            endpoints.insert(
                peer.alias.clone(),
                PeerEndpoint {
                    alias: peer.alias.clone(),
                    address: PathBuf::from(peer.address.as_str()),
                    connect_as: peer.connect_as.clone(),
                },
            );
            sessions.insert(peer.alias.clone(), Mutex::new(None));
        }
        Self {
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

    /// The identity this relay presents to the peer named by `alias`, as its
    /// full `<connect_as>@RELAY` principal. The forwarding handler uses it to set
    /// the forwarded request's `requester_session` to the identity that peer
    /// issued this relay (the peer sees this relay as the sender; original-sender
    /// attribution via `on_behalf_of` is deferred). `None` when `alias` names no
    /// configured peer.
    #[must_use]
    pub fn presented_principal_id(&self, alias: &str) -> Option<String> {
        self.endpoints
            .get(alias)
            .map(|endpoint| format!("{}@{RELAY_NAMESPACE}", endpoint.connect_as))
    }

    /// Forwards a fully-formed request to the peer named by the local `alias`
    /// (the bang-path `!<alias>` selector) and returns its response, establishing
    /// the connection lazily. Transport and handshake failures map to the same
    /// typed errors as [`connect`]; a peer that answers (including with an error
    /// response) returns that response for the caller to propagate as the delivery
    /// outcome.
    ///
    /// [`connect`]: PeerConnectionManager::connect
    pub fn forward(
        &self,
        alias: &str,
        request: &RelayRequest,
    ) -> Result<RelayResponse, RelayError> {
        let prepared = self.prepare(alias)?;
        let mut slot = prepared
            .session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::session_mut(&mut slot, &prepared)
            .request(request)
            .map_err(|source| prepared.unavailable_error(&source))
    }

    /// Establishes (or reuses) the outbound connection to the peer named by the
    /// local `alias` (the bang-path `!<alias>` selector), presenting this relay's
    /// per-peer `<connect_as>@RELAY` identity with the peer PSK. Returns a typed
    /// [`RelayError`] the caller maps to a delivery outcome:
    ///
    /// - `validation_unknown_peer` — no `[[peers]]` entry matches `alias`;
    /// - `runtime_peer_credential_missing` — the peer PSK file is absent, empty,
    ///   or unreadable;
    /// - `runtime_peer_unavailable` — the endpoint could not be dialed or the
    ///   Hello handshake failed.
    ///
    /// None of these fail relay startup — the connection is lazy, so every
    /// failure is scoped to the affected delivery.
    pub fn connect(&self, alias: &str) -> Result<(), RelayError> {
        let prepared = self.prepare(alias)?;
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
    fn prepare(&self, alias: &str) -> Result<PreparedDial<'_>, RelayError> {
        let endpoint = self.endpoints.get(alias).ok_or_else(|| {
            relay_error(
                "validation_unknown_peer",
                "no configured peer relay matches the target alias",
                Some(json!({ "relay_id": alias })),
            )
        })?;
        let token = self.read_peer_credential(endpoint.alias.as_str())?;
        let session = self
            .sessions
            .get(alias)
            .expect("session slot exists for every configured peer");
        Ok(PreparedDial {
            alias: alias.to_string(),
            endpoint,
            token,
            session,
        })
    }

    /// Returns the peer's session, initializing it lazily on first use. The dial
    /// presents this relay's per-peer `<connect_as>@RELAY` identity.
    fn session_mut<'slot>(
        slot: &'slot mut Option<RelayStreamSession>,
        prepared: &PreparedDial<'_>,
    ) -> &'slot mut RelayStreamSession {
        slot.get_or_insert_with(|| {
            RelayStreamSession::for_peer_relay(
                prepared.endpoint.address.clone(),
                prepared.endpoint.connect_as.clone(),
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
    alias: String,
    endpoint: &'a PeerEndpoint,
    token: String,
    session: &'a Mutex<Option<RelayStreamSession>>,
}

impl PreparedDial<'_> {
    fn unavailable_error(&self, source: &std::io::Error) -> RelayError {
        relay_error(
            "runtime_peer_unavailable",
            "outbound peer relay connection could not be established",
            Some(json!({
                "relay_id": self.alias,
                "address": self.endpoint.address.display().to_string(),
                "cause": source.to_string(),
            })),
        )
    }
}
