//! Relay IPC contract and message-routing implementation.

use std::{path::Path, time::Duration};

use crate::configuration::{ConfigurationRoots, load_bundle_configuration};
use crate::runtime::paths::{BundleRuntimePaths, tmux_socket_path_for_runtime_directory};
use crate::transports::WorkerReadinessState;

mod authorization;
mod client;
mod connection;
mod constants;
mod context;
mod contract;
mod delivery;
mod drain;
mod errors;
mod handlers;
mod identity;
mod lifecycle;
mod peer_connection;
mod routing;
mod startup_state;
mod stream;
mod watcher;

use self::authorization::load_authorization_context;
pub use self::authorization::{
    DeliveryConfiguration, PeerConfiguration, RelayRuntimeConfiguration,
    load_relay_runtime_configuration, parse_relay_bool_env_value, resolve_relay_bool_setting,
};

pub use self::client::{RelayStreamSession, request_relay};
pub use self::connection::{
    BundleCatalog, ConnectionServeContext, HostingIntent, serve_connection,
};
use self::constants::*;
use self::context::*;
pub use self::contract::*;
pub use self::delivery::admission::{
    UndeliveredReporting, configure_delivery, configured_undelivered_reporting,
    report_undelivered_queue,
};
pub use self::delivery::fence::{FenceOutcome, FenceResolution, FenceVerdict, acknowledge_fence};
pub use self::delivery::install_pending_choice_request_for_testing;
pub use self::delivery::observability::{
    ChoicesQueueEvent, subscribe_choices_queue_events, subscribe_worker_readiness,
};
pub use self::drain::{ConnectionDrainCoordinator, ConnectionDrainReport, ConnectionWorkerSlot};
use self::errors::*;
use self::identity::*;
pub use self::peer_connection::PeerConnectionManager;
pub use self::stream::second_claim_is_live_conflict_for_testing;
pub use self::watcher::{BundleWatcher, spawn_bundle_watcher};
#[doc(hidden)]
pub use self::watcher::{WatchWake, run_bundle_watch_loop};

/// Executes one relay request for a configured bundle.
pub fn handle_request(
    request: RelayRequest,
    configuration_roots: &ConfigurationRoots,
    bundle_name: &str,
    runtime_directory: &Path,
) -> Result<RelayResponse, RelayError> {
    // The non-stream entry point routes within a single bundle; cross-bundle
    // fan-out is reachable only over the stream path, which threads the live
    // catalog through `dispatch_request`. The catalog carries only the home
    // bundle — delivery resolves it like any other catalog entry, and targets
    // beyond it are confined to relay-wide (`@GLOBAL`) sessions.
    //
    // The state root is recovered by inverting the layout
    // `BundleRuntimePaths::resolve` builds — `<state_root>/bundles/<bundle>` —
    // because this entry point is handed only a runtime directory, and `up`
    // reads the catalog's state root to point spawned members at their relay.
    let state_root = runtime_directory
        .parent()
        .and_then(Path::parent)
        .unwrap_or(runtime_directory)
        .to_path_buf();
    let bundle_catalog = BundleCatalog::from_paths([BundleRuntimePaths {
        state_root,
        bundle_name: bundle_name.to_string(),
        runtime_directory: runtime_directory.to_path_buf(),
        tmux_socket: tmux_socket_path_for_runtime_directory(runtime_directory),
    }]);
    handle_request_with_principal(
        request,
        configuration_roots,
        bundle_name,
        runtime_directory,
        None,
        &bundle_catalog,
    )
}

fn handle_request_with_principal(
    request: RelayRequest,
    configuration_roots: &ConfigurationRoots,
    bundle_name: &str,
    runtime_directory: &Path,
    principal: Option<RequestPrincipal>,
    bundle_catalog: &BundleCatalog,
) -> Result<RelayResponse, RelayError> {
    // The target operations (`Send`/`Look`/`Raww`) flow through the
    // namespace-centric paths, which resolve the requester in its home namespace
    // rather than a borrowed dispatch bundle. The single-bundle entry point is
    // always bundle-bound, so the home namespace is this bundle. (The connection
    // layer reaches these dispatchers directly and never routes the operations
    // here.)
    match request {
        RelayRequest::Send { .. } => {
            // The non-stream single-bundle entry point holds no peer connection
            // manager, so a cross-relay target reports as unavailable here; the
            // stream path supplies the manager for real forwarding.
            return handlers::handle_send_routed(
                bundle_name,
                request,
                configuration_roots,
                bundle_catalog,
                principal.as_ref(),
                None,
            );
        }
        RelayRequest::Look { .. } => {
            return handlers::handle_look_routed(
                bundle_name,
                request,
                configuration_roots,
                bundle_catalog,
                principal.as_ref(),
            );
        }
        RelayRequest::Raww { .. } => {
            // The non-stream single-bundle entry point holds no peer connection
            // manager, so a cross-relay target reports as unavailable here; the
            // stream path supplies the manager for real forwarding.
            return handlers::handle_raww_routed(
                bundle_name,
                request,
                configuration_roots,
                bundle_catalog,
                principal.as_ref(),
                None,
            );
        }
        _ => {}
    }
    let bundle = load_bundle_configuration(configuration_roots, bundle_name).map_err(map_config)?;
    let authorization = load_authorization_context(configuration_roots, Some(&bundle))?;
    handlers::handle_request(
        request,
        &bundle,
        &authorization,
        runtime_directory,
        principal,
        bundle_catalog,
    )
}

/// Reconciles configured bundle sessions against tmux state.
///
/// Takes the resolved bundle paths for the same reason [`startup_bundle`] does:
/// reconcile creates members, and a created member has to carry the spawning
/// relay's state root.
///
/// # Errors
///
/// Returns structured validation/configuration errors when bundle loading
/// fails, and internal failures when tmux session operations fail.
pub fn reconcile_bundle(
    configuration_roots: &ConfigurationRoots,
    paths: &BundleRuntimePaths,
) -> Result<ReconciliationReport, RelayError> {
    lifecycle::reconcile_bundle(configuration_roots, paths)
}

/// Validates a bundle's configuration the way startup would (bundle + coders
/// schema and authorization-policy resolution) without starting tmux or relay
/// runtime. Backs the `agentmux check configuration` pre-flight CLI command.
///
/// # Errors
///
/// Returns the same structured validation/configuration errors as startup when
/// any configuration artifact fails to parse or resolve.
pub fn preflight_bundle_configuration(
    configuration_roots: &ConfigurationRoots,
    bundle_name: &str,
) -> Result<(), RelayError> {
    lifecycle::preflight_bundle_configuration(configuration_roots, bundle_name)
}

/// Attempts startup for all configured bundle sessions and reports outcomes.
///
/// Takes the resolved bundle paths rather than a bare runtime directory: the
/// spawning relay's state root is injected into every member's environment
/// here, and re-deriving it by walking up from the runtime directory would be
/// the guesswork this propagation exists to remove.
pub fn startup_bundle(
    configuration_roots: &ConfigurationRoots,
    paths: &BundleRuntimePaths,
) -> Result<BundleStartupReport, RelayError> {
    lifecycle::startup_bundle(configuration_roots, paths)
}

/// Registers the relay-wide principals declared in `users.toml` as static
/// (offline) unified-registry entries at relay startup, so look/raww resolve
/// their capabilities from the registry and a declared-but-disconnected principal
/// is a known target rather than an unknown one.
pub fn register_configured_relay_wide_principals(
    configuration_roots: &ConfigurationRoots,
) -> Result<(), RelayError> {
    lifecycle::register_configured_relay_wide_principals(configuration_roots)
}

/// Registers a bundle's configured members as static (offline) unified-registry
/// shells without starting transports, for the process-only / `--no-autostart`
/// host path where no startup or reconcile runs. Keeps the registry holding every
/// configured principal before the relay serves requests.
pub fn register_configured_bundle(
    configuration_roots: &ConfigurationRoots,
    bundle_name: &str,
) -> Result<(), RelayError> {
    lifecycle::register_configured_bundle(configuration_roots, bundle_name)
}

/// Returns the canonical `principal_id`s currently registered in `namespace`,
/// connected or not. Exposes the unified registry's membership so host startup
/// paths (and embedders) can confirm that every configured principal is known —
/// offline is a state, not absence — independent of transport readiness.
pub fn registered_principal_ids(namespace: &str) -> Vec<String> {
    stream::list_namespace_sessions(namespace)
        .into_iter()
        .map(|(principal_id, _session_type, _ready)| principal_id)
        .collect()
}

/// Tears one bundle's runtime down: stops its delivery workers, prunes its
/// managed tmux sessions, and reaps the tmux server when safe.
///
/// Takes the bundle name and runtime directory alongside the socket because a
/// bundle's runtime is more than its tmux sessions — those two identify the
/// delivery workers it owns, which are what a tmux-only teardown left running.
///
/// # Errors
///
/// Returns internal failures when tmux session operations fail.
pub fn shutdown_bundle_runtime(
    bundle_name: &str,
    runtime_directory: &Path,
    tmux_socket: &Path,
) -> Result<ShutdownReport, RelayError> {
    lifecycle::shutdown_bundle_runtime(bundle_name, runtime_directory, tmux_socket)
}

/// Test-only cleanup for `acp::*` integration tests.
///
/// Removes all `AsyncDeliveryRegistry` workers keyed by `runtime_directory == root`.
///
/// Each `acp::*` test uses `flat_bundle_paths` where `runtime_directory ==
/// `TempDir.path()`. This removes the registry entry so the sender cannot be
/// reused, but it does not reap the `acp_stub` children — the `AcpTransport`
/// worker holds the `AcpStdioClient` and is never cancelled, so the child
/// teardown in `AcpStdioClient::shutdown` never runs. The harness kills them
/// explicitly in `GuardedTempDir::drop`.
#[doc(hidden)]
pub fn test_cleanup_acp_workers(root: &Path) {
    crate::relay::delivery::async_worker::registry::remove_workers_for_runtime_directory(root);
}

/// Loads persisted startup-failure history for one bundle runtime directory.
pub fn load_startup_failures(
    runtime_directory: &Path,
) -> Result<Vec<StartupFailureRecord>, String> {
    startup_state::load_startup_failures(runtime_directory)
}

/// Appends one startup-failure record to persisted bundle history.
pub fn append_startup_failure(
    runtime_directory: &Path,
    record: StartupFailureRecord,
) -> Result<StartupFailureRecord, String> {
    startup_state::append_startup_failure(runtime_directory, record)
}

/// Records that a session has been observed serving successfully, clearing any
/// startup-failure records it left behind.
///
/// The recovery half of [`append_startup_failure`]: one records that a session
/// failed to start, the other that it is now serving, and [`load_startup_failures`]
/// reads what the two have left standing. Repeated calls for a session whose
/// history is already clear are cheap.
///
/// # Errors
///
/// Returns the history read/write failure cause, or a lock-poisoned cause.
pub fn note_session_served_successfully(
    runtime_directory: &Path,
    session_id: &str,
) -> Result<(), String> {
    startup_state::note_session_served_successfully(runtime_directory, session_id)
}

/// Persists and announces a bring-up pass's per-session startup failures,
/// returning the persisted records.
///
/// # Errors
///
/// Returns the history-write failure cause when a record cannot be persisted.
pub fn persist_startup_failures(
    bundle_name: &str,
    runtime_directory: &Path,
    failures: &[StartupFailureRecord],
) -> Result<Vec<StartupFailureRecord>, String> {
    startup_state::persist_and_announce_startup_failures(bundle_name, runtime_directory, failures)
}

/// Waits for async delivery workers to stop after shutdown is requested.
///
/// Returns the number of workers still running when timeout is reached.
#[must_use]
pub fn wait_for_async_delivery_shutdown(timeout: Duration) -> usize {
    delivery::wait_for_async_delivery_shutdown(timeout)
}

/// Reads the in-memory worker readiness state for an observability check.
///
/// Transport-agnostic: any worker-driven transport (ACP today, Pty next)
/// populates the same state. Returns one of "initializing", "available", "busy",
/// "recovering", "unavailable" when a worker is registered for the (namespace,
/// runtime_directory, target_session) triple, or `None` when no worker is
/// registered or no readiness state has been recorded yet. The "recovering"
/// value indicates the worker observed a transport failure and is rebuilding its
/// underlying worker process (the ACP child today); clients that do not recognize
/// the value should treat it as non-ready.
#[must_use]
pub fn read_worker_readiness(
    namespace: &str,
    runtime_directory: &Path,
    target_session: &str,
) -> Option<&'static str> {
    delivery::get_worker_readiness(namespace, runtime_directory, target_session).map(|state| {
        match state {
            WorkerReadinessState::Initializing => "initializing",
            WorkerReadinessState::Available => "available",
            WorkerReadinessState::Busy => "busy",
            WorkerReadinessState::Recovering => "recovering",
            WorkerReadinessState::Unavailable => "unavailable",
        }
    })
}

pub(in crate::relay) fn dispatch_request(
    request: RelayRequest,
    configuration_roots: &ConfigurationRoots,
    bundle_name: &str,
    runtime_directory: &Path,
    principal: Option<RequestPrincipal>,
    bundle_catalog: &BundleCatalog,
) -> RelayResponse {
    match handle_request_with_principal(
        request,
        configuration_roots,
        bundle_name,
        runtime_directory,
        principal,
        bundle_catalog,
    ) {
        Ok(value) => value,
        Err(error) => RelayResponse::Error { error },
    }
}

/// Dispatches a stream `List` request through the routing/authorization spine.
///
/// `dispatch_paths` locate the requester's home (authorization) bundle and
/// `enumerate_paths` the bundle whose sessions are listed; they coincide for a
/// same-bundle list and differ for a cross-bundle list. The requester's `list`
/// control is always resolved in the dispatch bundle — a peer bundle never
/// supplies the requester's policy — while the enumerated bundle supplies the
/// session listing and runtime context. This is the seam that lets a session
/// list a peer bundle without being rejected as unknown in the wrong bundle's
/// members.
pub(in crate::relay) fn dispatch_list(
    request: RelayRequest,
    configuration_roots: &ConfigurationRoots,
    dispatch_paths: &crate::runtime::paths::BundleRuntimePaths,
    enumerate_paths: &crate::runtime::paths::BundleRuntimePaths,
) -> RelayResponse {
    let result = (|| {
        let RelayRequest::List { requester_session } = request else {
            return Err(relay_error(
                "internal_unexpected_request",
                "non-list request routed to the list dispatcher",
                None,
            ));
        };
        let dispatch_bundle =
            load_bundle_configuration(configuration_roots, &dispatch_paths.bundle_name)
                .map_err(map_config)?;
        let dispatch_authorization =
            load_authorization_context(configuration_roots, Some(&dispatch_bundle))?;
        let enumerate_bundle =
            load_bundle_configuration(configuration_roots, &enumerate_paths.bundle_name)
                .map_err(map_config)?;
        handlers::handle_list_routed(
            &dispatch_bundle,
            &dispatch_authorization,
            &enumerate_bundle,
            &enumerate_paths.runtime_directory,
            requester_session,
        )
    })();
    match result {
        Ok(value) => value,
        Err(error) => RelayResponse::Error { error },
    }
}

/// Dispatches a `Send` through the namespace-centric send path.
///
/// The requester is identified by its **home namespace**: its bound bundle for a
/// session principal, or `GLOBAL` for a relay-wide principal — whose controls
/// come from the operator policy, not a borrowed peer bundle. `bound_bundle` is
/// `None` for relay-wide senders. The send path loads the home bundle and its
/// authorization context from the namespace; per-target delivery loads every
/// target bundle — the home bundle included — from the catalog.
pub(in crate::relay) fn dispatch_send(
    request: RelayRequest,
    configuration_roots: &ConfigurationRoots,
    bound_bundle: Option<&crate::runtime::paths::BundleRuntimePaths>,
    principal: Option<RequestPrincipal>,
    bundle_catalog: &BundleCatalog,
    peer_connection_manager: &PeerConnectionManager,
) -> RelayResponse {
    let home_namespace = match bound_bundle {
        Some(paths) => paths.bundle_name.clone(),
        None => GLOBAL_NAMESPACE.to_string(),
    };
    match handlers::handle_send_routed(
        home_namespace.as_str(),
        request,
        configuration_roots,
        bundle_catalog,
        principal.as_ref(),
        Some(peer_connection_manager),
    ) {
        Ok(value) => value,
        Err(error) => RelayResponse::Error { error },
    }
}

/// Dispatches a `Look` through the namespace-centric path.
///
/// The requester is identified by its **home namespace**: its bound bundle for a
/// session principal, or `GLOBAL` for a relay-wide principal. `bound_bundle` is
/// `None` for relay-wide requesters. The look path resolves and authorizes the
/// requester in its home namespace and loads the target's bundle separately for
/// the snapshot — never a borrowed dispatch bundle.
pub(in crate::relay) fn dispatch_look(
    request: RelayRequest,
    configuration_roots: &ConfigurationRoots,
    bound_bundle: Option<&crate::runtime::paths::BundleRuntimePaths>,
    principal: Option<RequestPrincipal>,
    bundle_catalog: &BundleCatalog,
) -> RelayResponse {
    let home_namespace = match bound_bundle {
        Some(paths) => paths.bundle_name.clone(),
        None => GLOBAL_NAMESPACE.to_string(),
    };
    match handlers::handle_look_routed(
        home_namespace.as_str(),
        request,
        configuration_roots,
        bundle_catalog,
        principal.as_ref(),
    ) {
        Ok(value) => value,
        Err(error) => RelayResponse::Error { error },
    }
}

/// Dispatches a `Raww` through the namespace-centric path.
///
/// Like `dispatch_look`, the requester is identified by its home namespace
/// (bound bundle, or `GLOBAL` for a relay-wide principal) and authorized there;
/// the target's bundle is loaded separately for delivery. This is what lets a
/// cross-namespace raww authorize the sender in its home namespace rather than a
/// borrowed target bundle.
pub(in crate::relay) fn dispatch_raww(
    request: RelayRequest,
    configuration_roots: &ConfigurationRoots,
    bound_bundle: Option<&crate::runtime::paths::BundleRuntimePaths>,
    principal: Option<RequestPrincipal>,
    bundle_catalog: &BundleCatalog,
    peer_connection_manager: &PeerConnectionManager,
) -> RelayResponse {
    let home_namespace = match bound_bundle {
        Some(paths) => paths.bundle_name.clone(),
        None => GLOBAL_NAMESPACE.to_string(),
    };
    match handlers::handle_raww_routed(
        home_namespace.as_str(),
        request,
        configuration_roots,
        bundle_catalog,
        principal.as_ref(),
        Some(peer_connection_manager),
    ) {
        Ok(value) => value,
        Err(error) => RelayResponse::Error { error },
    }
}

/// Dispatches a relay-wide discovery request (`list.relays`, `list.namespaces`,
/// cross-relay `list.principals`). Relay-wide like identity admin: it reads the
/// configured peer aliases and this relay's own catalog/`GLOBAL` registry, and
/// forwards foreign discovery through the peer connection manager. The requester
/// is identified by its authenticated `principal`, never a wire field, so no
/// bound bundle is threaded here.
pub(in crate::relay) fn dispatch_discovery(
    request: RelayRequest,
    configuration_roots: &ConfigurationRoots,
    principal: RequestPrincipal,
    bundle_catalog: &BundleCatalog,
    peer_connection_manager: &PeerConnectionManager,
    configured_relay_aliases: &[String],
) -> RelayResponse {
    let context = handlers::DiscoveryContext {
        configuration_roots,
        bundle_catalog,
        peer_connection_manager,
        configured_relay_aliases,
    };
    let result = match request {
        RelayRequest::ListRelays => handlers::handle_list_relays(&context, &principal),
        RelayRequest::DiscoverNamespaces { relay } => {
            handlers::handle_discover_namespaces(&context, &principal, relay)
        }
        RelayRequest::DiscoverPrincipals { relay, namespace } => {
            handlers::handle_discover_principals(&context, &principal, relay, namespace)
        }
        _ => Err(relay_error(
            "internal_unexpected_request",
            "non-discovery request routed to the discovery dispatcher",
            None,
        )),
    };
    match result {
        Ok(value) => value,
        Err(error) => RelayResponse::Error { error },
    }
}

/// Dispatches a relay-wide identity administration request (`new peer`,
/// `change psk`), which mutates the relay-level principal store and has no
/// bundle context. `requester_principal_id` is the full claimed identity of the
/// caller, used to resolve operator authorization relay-wide.
pub(in crate::relay) fn dispatch_identity_admin(
    request: RelayRequest,
    configuration_roots: &ConfigurationRoots,
    state_root: &Path,
    requester_principal_id: &str,
    identity_admin_lock: &std::sync::Mutex<()>,
) -> RelayResponse {
    // Serialize the whole load/stage/persist/rename transaction at relay scope.
    // Recover from a poisoned lock rather than propagating: a prior panic must
    // not wedge all future credential administration. Held across the blocking
    // transaction below (this runs on the blocking pool).
    let _guard = identity_admin_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match handlers::handle_identity_admin_request(
        request,
        configuration_roots,
        state_root,
        requester_principal_id,
    ) {
        Ok(value) => value,
        Err(error) => RelayResponse::Error { error },
    }
}

/// Dispatches a relay-wide `IdentityIntrospect` request. Introspection reads the
/// relay-level principal store and its target may be a bundle-less principal, so
/// like the identity admin requests it bypasses the per-bundle path. The gate is
/// the connection's recorded `introspect_rights`, carried on `principal`.
pub(in crate::relay) fn dispatch_identity_introspect(
    request: RelayRequest,
    state_root: &Path,
    principal: &RequestPrincipal,
) -> RelayResponse {
    let RelayRequest::IdentityIntrospect { target_session } = request else {
        return RelayResponse::Error {
            error: relay_error(
                "internal_unexpected_request",
                "non-introspect request routed to identity introspect dispatcher",
                None,
            ),
        };
    };
    match handlers::handle_identity_introspect(state_root, principal, target_session.as_str()) {
        Ok(value) => value,
        Err(error) => RelayResponse::Error { error },
    }
}

/// Prunes expired records from the relay-level principal store.
///
/// Loads the store at `<state-root>/identity/principals.json`, drops records
/// whose `expires_at` has passed (or cannot be parsed), and rewrites the file
/// only when something was pruned. A missing store loads empty and no file is
/// created. Intended to run once at relay startup; per-connection access prunes
/// in memory, and store mutations persist the pruned set.
pub fn prune_principal_store(state_root: &Path) -> Result<usize, RelayError> {
    let mut store = PrincipalStore::load(crate::runtime::paths::principal_store_path(state_root))?;
    let pruned = store.prune_expired(time::OffsetDateTime::now_utc());
    if pruned > 0 {
        store.persist()?;
    }
    Ok(pruned)
}
