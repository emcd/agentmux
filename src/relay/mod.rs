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

/// Executes one relay request for a configured bundle.
pub fn handle_request(
    request: RelayRequest,
    configuration_root: &Path,
    bundle_name: &str,
    runtime_directory: &Path,
) -> Result<RelayResponse, RelayError> {
    handle_request_with_principal(
        request,
        configuration_root,
        bundle_name,
        runtime_directory,
        None,
    )
}

fn handle_request_with_principal(
    request: RelayRequest,
    configuration_root: &Path,
    bundle_name: &str,
    runtime_directory: &Path,
    principal: Option<RequestPrincipal>,
) -> Result<RelayResponse, RelayError> {
    let bundle = load_bundle_configuration(configuration_root, bundle_name).map_err(map_config)?;
    let authorization = load_authorization_context(configuration_root, &bundle)?;
    handlers::handle_request(
        request,
        &bundle,
        &authorization,
        runtime_directory,
        principal,
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
) -> RelayResponse {
    match handle_request_with_principal(
        request,
        configuration_root,
        bundle_name,
        runtime_directory,
        principal,
    ) {
        Ok(value) => value,
        Err(error) => RelayResponse::Error { error },
    }
}
