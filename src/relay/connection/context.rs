//! The connection-independent handles every accepted connection is served
//! against.
//!
//! Assembled once by the relay host and cloned per connection. Fields are
//! readable across the `connection` subtree because the frame loop and the
//! per-frame handlers each need a different subset; nothing outside the subtree
//! reaches past the accessors.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::configuration::ConfigurationRoots;

use super::super::PeerConnectionManager;
use super::super::catalog::BundleCatalog;

/// Shared, connection-independent context threaded to every connection worker:
/// the config/state roots, the live bundle catalog, the outbound peer connection
/// manager, and the resolved relay-wide serving controls. Grouping these into one
/// value keeps the connection-serving signatures within argument limits (rather
/// than suppressing the lint) and gives each accepted connection a cheap clone of
/// the shared handles.
#[derive(Clone, Debug)]
pub struct ConnectionServeContext {
    pub(super) configuration_roots: ConfigurationRoots,
    pub(super) state_root: PathBuf,
    pub(super) bundle_catalog: BundleCatalog,
    pub(super) peer_connection_manager: Arc<PeerConnectionManager>,
    /// This relay's configured outbound peer aliases, sorted, read from the
    /// normalized `[[peers]]` configuration at startup. `list.relays` enumerates
    /// them directly rather than dialing or querying the connection manager.
    pub(super) relay_aliases: Arc<Vec<String>>,
    pub(super) require_session_credentials: bool,
    pub(super) pre_hello_idle_timeout: Duration,
    /// Relay-scope mutex serializing identity-admin (`new peer` / `change psk`)
    /// store transactions. Each admin op runs a load/stage/persist/rename
    /// sequence on the blocking pool against the on-disk principal store; without
    /// serialization two concurrent ops could interleave store persists and
    /// credential renames, publishing a credential whose PSK no longer matches
    /// the stored hash or losing an unrelated registration. Shared across every
    /// cloned per-connection context via the `Arc`.
    pub(super) identity_admin_lock: Arc<Mutex<()>>,
}

impl ConnectionServeContext {
    /// Assembles the shared serving context from resolved relay runtime state; it
    /// is cloned per accepted connection.
    #[must_use]
    pub fn new(
        configuration_roots: ConfigurationRoots,
        state_root: PathBuf,
        bundle_catalog: BundleCatalog,
        peer_connection_manager: Arc<PeerConnectionManager>,
        relay_aliases: Vec<String>,
        require_session_credentials: bool,
        pre_hello_idle_timeout: Duration,
    ) -> Self {
        let mut relay_aliases = relay_aliases;
        relay_aliases.sort();
        relay_aliases.dedup();
        Self {
            configuration_roots,
            state_root,
            bundle_catalog,
            peer_connection_manager,
            relay_aliases: Arc::new(relay_aliases),
            require_session_credentials,
            pre_hello_idle_timeout,
            identity_admin_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Configuration layers the catalog was built against, used by the serve
    /// phase to spawn watchers without holding a `RuntimeRoots`.
    #[must_use]
    pub fn configuration_roots(&self) -> &ConfigurationRoots {
        &self.configuration_roots
    }

    /// State root the catalog was built against; carried alongside the
    /// configuration root so watcher spawning doesn't depend on outer state.
    #[must_use]
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    /// Live bundle catalog; cloned (cheaply, via the inner `Arc<RwLock<...>>`)
    /// when a connection worker needs to resolve a bundle by name.
    #[must_use]
    pub fn bundle_catalog(&self) -> &BundleCatalog {
        &self.bundle_catalog
    }
}
