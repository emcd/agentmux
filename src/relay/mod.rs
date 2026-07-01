//! Relay IPC contract and message-routing implementation.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use crate::configuration::load_bundle_configuration;
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
mod routing;
mod startup_state;
mod stream;
mod watcher;

use self::authorization::load_authorization_context;
pub use self::authorization::{
    PeerConfiguration, RelayRuntimeConfiguration, load_relay_runtime_configuration,
    parse_relay_bool_env_value, resolve_relay_bool_setting,
};

pub use self::client::{RelayStreamSession, request_relay};
pub use self::connection::{BundleCatalog, HostingIntent, serve_connection};
use self::constants::*;
use self::context::*;
pub use self::contract::*;
pub use self::delivery::install_pending_choice_request_for_testing;
pub use self::delivery::observability::{
    ChoicesQueueEvent, subscribe_choices_queue_events, subscribe_worker_readiness,
};
pub use self::drain::{ConnectionDrainCoordinator, ConnectionDrainReport, ConnectionWorkerSlot};
use self::errors::*;
use self::identity::*;
pub use self::stream::second_claim_is_live_conflict_for_testing;
pub use self::watcher::{BundleWatcher, spawn_bundle_watcher};

/// Executes one relay request for a configured bundle.
pub fn handle_request(
    request: RelayRequest,
    configuration_root: &Path,
    bundle_name: &str,
    runtime_directory: &Path,
) -> Result<RelayResponse, RelayError> {
    // The non-stream entry point routes within a single bundle; cross-bundle
    // fan-out is reachable only over the stream path, which threads the live
    // catalog through `dispatch_request`. The catalog carries only the home
    // bundle — delivery resolves it like any other catalog entry, and targets
    // beyond it are confined to relay-wide (`@GLOBAL`) sessions. `state_root`
    // is a placeholder: delivery consumes only `runtime_directory`.
    let bundle_catalog = BundleCatalog::from_paths([BundleRuntimePaths {
        state_root: PathBuf::new(),
        bundle_name: bundle_name.to_string(),
        runtime_directory: runtime_directory.to_path_buf(),
        tmux_socket: tmux_socket_path_for_runtime_directory(runtime_directory),
    }]);
    handle_request_with_principal(
        request,
        configuration_root,
        bundle_name,
        runtime_directory,
        None,
        &bundle_catalog,
    )
}

fn handle_request_with_principal(
    request: RelayRequest,
    configuration_root: &Path,
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
            return handlers::handle_send_routed(
                bundle_name,
                request,
                configuration_root,
                bundle_catalog,
                principal.as_ref(),
            );
        }
        RelayRequest::Look { .. } => {
            return handlers::handle_look_routed(
                bundle_name,
                Some(runtime_directory),
                request,
                configuration_root,
                bundle_catalog,
                principal.as_ref(),
            );
        }
        RelayRequest::Raww { .. } => {
            return handlers::handle_raww_routed(
                bundle_name,
                Some(runtime_directory),
                request,
                configuration_root,
                bundle_catalog,
            );
        }
        _ => {}
    }
    let bundle = load_bundle_configuration(configuration_root, bundle_name).map_err(map_config)?;
    let authorization = load_authorization_context(configuration_root, Some(&bundle))?;
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
/// # Errors
///
/// Returns structured validation/configuration errors when bundle loading
/// fails, and internal failures when tmux session operations fail.
pub fn reconcile_bundle(
    configuration_root: &Path,
    bundle_name: &str,
    tmux_socket: &Path,
) -> Result<ReconciliationReport, RelayError> {
    lifecycle::reconcile_bundle(configuration_root, bundle_name, tmux_socket)
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
    configuration_root: &Path,
    bundle_name: &str,
) -> Result<(), RelayError> {
    lifecycle::preflight_bundle_configuration(configuration_root, bundle_name)
}

/// Attempts startup for all configured bundle sessions and reports outcomes.
pub fn startup_bundle(
    configuration_root: &Path,
    bundle_name: &str,
    runtime_directory: &Path,
) -> Result<BundleStartupReport, RelayError> {
    lifecycle::startup_bundle(configuration_root, bundle_name, runtime_directory)
}

/// Registers the relay-wide principals declared in `users.toml` as static
/// (offline) unified-registry entries at relay startup, so look/raww resolve
/// their capabilities from the registry and a declared-but-disconnected principal
/// is a known target rather than an unknown one.
pub fn register_configured_relay_wide_principals(
    configuration_root: &Path,
) -> Result<(), RelayError> {
    lifecycle::register_configured_relay_wide_principals(configuration_root)
}

/// Registers a bundle's configured members as static (offline) unified-registry
/// shells without starting transports, for the process-only / `--no-autostart`
/// host path where no startup or reconcile runs. Keeps the registry holding every
/// configured principal before the relay serves requests.
pub fn register_configured_bundle(
    configuration_root: &Path,
    bundle_name: &str,
) -> Result<(), RelayError> {
    lifecycle::register_configured_bundle(configuration_root, bundle_name)
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

/// Prunes managed sessions and reaps tmux server when safe during shutdown.
///
/// # Errors
///
/// Returns internal failures when tmux session operations fail.
pub fn shutdown_bundle_runtime(tmux_socket: &Path) -> Result<ShutdownReport, RelayError> {
    lifecycle::shutdown_bundle_runtime(tmux_socket)
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
    configuration_root: &Path,
    bundle_name: &str,
    runtime_directory: &Path,
    principal: Option<RequestPrincipal>,
    bundle_catalog: &BundleCatalog,
) -> RelayResponse {
    match handle_request_with_principal(
        request,
        configuration_root,
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
    configuration_root: &Path,
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
            load_bundle_configuration(configuration_root, &dispatch_paths.bundle_name)
                .map_err(map_config)?;
        let dispatch_authorization =
            load_authorization_context(configuration_root, Some(&dispatch_bundle))?;
        let enumerate_bundle =
            load_bundle_configuration(configuration_root, &enumerate_paths.bundle_name)
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
    configuration_root: &Path,
    bound_bundle: Option<&crate::runtime::paths::BundleRuntimePaths>,
    principal: Option<RequestPrincipal>,
    bundle_catalog: &BundleCatalog,
) -> RelayResponse {
    let home_namespace = match bound_bundle {
        Some(paths) => paths.bundle_name.clone(),
        None => GLOBAL_NAMESPACE.to_string(),
    };
    match handlers::handle_send_routed(
        home_namespace.as_str(),
        request,
        configuration_root,
        bundle_catalog,
        principal.as_ref(),
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
    configuration_root: &Path,
    bound_bundle: Option<&crate::runtime::paths::BundleRuntimePaths>,
    principal: Option<RequestPrincipal>,
    bundle_catalog: &BundleCatalog,
) -> RelayResponse {
    let (home_namespace, home_runtime) = match bound_bundle {
        Some(paths) => (
            paths.bundle_name.clone(),
            Some(paths.runtime_directory.clone()),
        ),
        None => (GLOBAL_NAMESPACE.to_string(), None),
    };
    match handlers::handle_look_routed(
        home_namespace.as_str(),
        home_runtime.as_deref(),
        request,
        configuration_root,
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
    configuration_root: &Path,
    bound_bundle: Option<&crate::runtime::paths::BundleRuntimePaths>,
    bundle_catalog: &BundleCatalog,
) -> RelayResponse {
    let (home_namespace, home_runtime) = match bound_bundle {
        Some(paths) => (
            paths.bundle_name.clone(),
            Some(paths.runtime_directory.clone()),
        ),
        None => (GLOBAL_NAMESPACE.to_string(), None),
    };
    match handlers::handle_raww_routed(
        home_namespace.as_str(),
        home_runtime.as_deref(),
        request,
        configuration_root,
        bundle_catalog,
    ) {
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
    configuration_root: &Path,
    state_root: &Path,
    requester_principal_id: &str,
) -> RelayResponse {
    match handlers::handle_identity_admin_request(
        request,
        configuration_root,
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
