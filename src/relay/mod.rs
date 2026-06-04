//! Relay IPC contract and message-routing implementation.

use std::{path::Path, time::Duration};

use crate::configuration::load_bundle_configuration;

mod authorization;
mod client;
mod connection;
mod constants;
mod context;
mod contract;
mod delivery;
mod errors;
mod handlers;
mod identity;
mod lifecycle;
mod startup_state;
mod stream;
mod tmux;
mod watcher;

use self::authorization::load_authorization_context;

pub use self::client::{RelayStreamSession, request_relay};
pub use self::connection::{BundleCatalog, serve_connection};
use self::constants::*;
use self::context::*;
pub use self::contract::*;
pub use self::delivery::AcpWorkerReadinessState;
pub use self::delivery::install_pending_permission_request_for_testing;
pub use self::delivery::observability::{
    PermissionQueueEvent, subscribe_acp_worker_state, subscribe_permission_queue_events,
};
use self::errors::*;
use self::identity::*;
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
    // catalog through `dispatch_request`. An empty catalog confines this path
    // to same-bundle and relay-wide (`@GLOBAL`) targets.
    let bundle_catalog = BundleCatalog::default();
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
    let bundle = load_bundle_configuration(configuration_root, bundle_name).map_err(map_config)?;
    let authorization = load_authorization_context(configuration_root, &bundle)?;
    handlers::handle_request(
        request,
        &bundle,
        &authorization,
        runtime_directory,
        principal,
        configuration_root,
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

/// Attempts startup for all configured bundle sessions and reports outcomes.
pub fn startup_bundle(
    configuration_root: &Path,
    bundle_name: &str,
    runtime_directory: &Path,
) -> Result<BundleStartupReport, RelayError> {
    lifecycle::startup_bundle(configuration_root, bundle_name, runtime_directory)
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

/// Reads the in-memory ACP worker readiness state for an observability check.
///
/// Returns one of "initializing", "available", "busy", "recovering",
/// "unavailable" when a worker is registered for the (bundle_name,
/// runtime_directory, target_session) triple, or `None` when no worker is
/// registered or no readiness state has been recorded yet. The "recovering"
/// value indicates the worker observed a transport failure and is rebuilding
/// the ACP child process; clients that do not recognize the value should
/// treat it as non-ready.
#[must_use]
pub fn read_acp_worker_state(
    bundle_name: &str,
    runtime_directory: &Path,
    target_session: &str,
) -> Option<&'static str> {
    delivery::get_acp_worker_state(bundle_name, runtime_directory, target_session).map(|state| {
        match state {
            delivery::AcpWorkerReadinessState::Initializing => "initializing",
            delivery::AcpWorkerReadinessState::Available => "available",
            delivery::AcpWorkerReadinessState::Busy => "busy",
            delivery::AcpWorkerReadinessState::Recovering => "recovering",
            delivery::AcpWorkerReadinessState::Unavailable => "unavailable",
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
    let RelayRequest::IdentityIntrospect {
        target_session,
        bundle_name,
    } = request
    else {
        return RelayResponse::Error {
            error: relay_error(
                "internal_unexpected_request",
                "non-introspect request routed to identity introspect dispatcher",
                None,
            ),
        };
    };
    match handlers::handle_identity_introspect(
        state_root,
        principal,
        target_session.as_str(),
        bundle_name.as_deref(),
    ) {
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
