//! Relay IPC contract and message-routing implementation.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    acp::AcpSnapshotEntry,
    configuration::{
        BundleConfiguration, ConfigurationError, SessionType, load_bundle_configuration,
    },
    envelope::{ENVELOPE_SCHEMA_VERSION, PromptBatchSettings},
};

mod authorization;
mod client;
mod connection;
mod delivery;
mod handlers;
mod lifecycle;
mod startup_state;
mod stream;
mod tmux;

use self::authorization::load_authorization_context;
use self::delivery::QuiescenceOptions;

pub use self::client::{RelayStreamSession, request_relay};
pub use self::connection::serve_connection;

const SCHEMA_VERSION: &str = ENVELOPE_SCHEMA_VERSION;
const POLICIES_FILE: &str = "policies.toml";
const POLICIES_FORMAT_VERSION: u32 = 1;
const GLOBAL_SESSION_SUFFIX: &str = "@GLOBAL";

/// Returns the canonical `session@bundle` identity for a session id.
///
/// Global-user identities already carry the `@GLOBAL` suffix and are their own
/// canonical form; bundle-local identities are qualified with the bundle name.
pub(super) fn canonical_session_id(session_id: &str, bundle_name: &str) -> String {
    if session_id.ends_with(GLOBAL_SESSION_SUFFIX) {
        session_id.to_string()
    } else {
        format!("{session_id}@{bundle_name}")
    }
}

/// Returns the bundle-local session id for a possibly-canonical identity.
///
/// Strips a trailing `@{bundle_name}` qualifier so internal lookups match
/// configured member ids; global-user (`@GLOBAL`) identities and already-bare
/// ids are returned unchanged.
pub(super) fn bare_session_id(session_id: &str, bundle_name: &str) -> String {
    if session_id.ends_with(GLOBAL_SESSION_SUFFIX) {
        return session_id.to_string();
    }
    let qualifier = format!("@{bundle_name}");
    session_id
        .strip_suffix(qualifier.as_str())
        .unwrap_or(session_id)
        .to_string()
}

/// Declared session type for one listed bundle session.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ListedSessionTransport {
    Tmux,
    Acp,
    Ui,
    Pubsub,
}

impl From<SessionType> for ListedSessionTransport {
    fn from(value: SessionType) -> Self {
        match value {
            SessionType::Tmux => Self::Tmux,
            SessionType::Acp => Self::Acp,
            SessionType::Ui => Self::Ui,
            SessionType::Pubsub => Self::Pubsub,
        }
    }
}

/// One configured session entry in list-sessions payloads.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ListedSession {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub transport: ListedSessionTransport,
}

/// Bundle live state in list-sessions payloads.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ListedBundleState {
    Up,
    Down,
}

/// Startup health marker for an `up` bundle in list-sessions payloads.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ListedBundleStartupHealth {
    Healthy,
    Degraded,
}

/// Freshness status for ACP-backed look snapshot responses.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AcpLookFreshness {
    Fresh,
    Stale,
}

/// Source marker for ACP-backed look snapshot responses.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AcpLookSnapshotSource {
    LiveBuffer,
    None,
}

/// Snapshot payload variant for look responses.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "snapshot_format", rename_all = "snake_case")]
pub enum LookSnapshotPayload {
    Lines {
        snapshot_lines: Vec<String>,
    },
    AcpEntriesV1 {
        snapshot_entries: Vec<AcpSnapshotEntry>,
        freshness: AcpLookFreshness,
        snapshot_source: AcpLookSnapshotSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        stale_reason_code: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        snapshot_age_ms: Option<u64>,
    },
}

/// One persisted startup-failure record surfaced in list-sessions payloads.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StartupFailureRecord {
    pub bundle_name: String,
    pub session_id: String,
    pub transport: ListedSessionTransport,
    pub code: String,
    pub reason: String,
    pub timestamp: String,
    pub sequence: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Canonical listed bundle payload for session-listing responses.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ListedBundle {
    pub id: String,
    pub state: ListedBundleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_health: Option<ListedBundleStartupHealth>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_reason_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_reason: Option<String>,
    pub startup_failure_count: usize,
    pub recent_startup_failures: Vec<StartupFailureRecord>,
    pub sessions: Vec<ListedSession>,
}

/// Per-target delivery result for one `chat` request.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ChatResult {
    pub target_session: String,
    pub message_id: String,
    pub outcome: ChatOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Reconciliation results for one bundle lifecycle pass.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReconciliationReport {
    pub bootstrap_session: Option<String>,
    pub created_sessions: Vec<String>,
    pub pruned_sessions: Vec<String>,
}

/// Managed-session cleanup results for relay shutdown.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ShutdownReport {
    pub pruned_sessions: Vec<String>,
    pub killed_tmux_server: bool,
}

/// Per-bundle startup pass outcome for relay host autostart.
#[derive(Clone, Debug, PartialEq)]
pub struct BundleStartupReport {
    pub ready_session_count: usize,
    pub failed_startups: Vec<StartupFailureRecord>,
}

/// Per-target delivery outcome for `chat`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatOutcome {
    Queued,
    Delivered,
    Timeout,
    DroppedOnShutdown,
    Failed,
}

/// Payload handling mode for one async delivery task.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryPayloadMode {
    EnvelopeMessage,
    RawInput,
}

/// Structured relay error object.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RelayError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Relay-pushed stream event payload.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RelayStreamEvent {
    pub event_type: String,
    pub bundle_name: String,
    pub target_session: String,
    pub created_at: String,
    pub payload: Value,
}

/// Per-bundle lifecycle transition result for `up`/`down`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LifecycleBundleResult {
    pub bundle_name: String,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Relay request protocol.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum RelayRequest {
    Up,
    Down,
    List {
        sender_session: Option<String>,
    },
    Chat {
        request_id: Option<String>,
        sender_session: String,
        message: String,
        targets: Vec<String>,
        broadcast: bool,
        #[serde(default)]
        quiet_window_ms: Option<u64>,
        #[serde(default)]
        quiescence_timeout_ms: Option<u64>,
        #[serde(default)]
        acp_turn_timeout_ms: Option<u64>,
    },
    Look {
        requester_session: String,
        target_session: String,
        #[serde(default)]
        lines: Option<usize>,
        #[serde(default)]
        bundle_name: Option<String>,
    },
    Raww {
        request_id: Option<String>,
        sender_session: String,
        target_session: String,
        text: String,
        #[serde(default)]
        no_enter: bool,
        #[serde(default)]
        bundle_name: Option<String>,
    },
    PermissionResolve {
        permission_request_id: String,
        outcome: String,
        #[serde(default)]
        option_id: Option<String>,
        #[serde(default)]
        bundle_name: Option<String>,
        #[serde(default)]
        ui_session_id: Option<String>,
    },
    PermissionList {
        #[serde(default)]
        bundle_name: Option<String>,
    },
}

/// Relay response protocol.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RelayResponse {
    Lifecycle {
        schema_version: String,
        action: String,
        bundles: Vec<LifecycleBundleResult>,
        changed_bundle_count: usize,
        skipped_bundle_count: usize,
        failed_bundle_count: usize,
        changed_any: bool,
    },
    List {
        schema_version: String,
        bundle: ListedBundle,
    },
    Chat {
        schema_version: String,
        bundle_name: String,
        request_id: Option<String>,
        sender_session: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        sender_display_name: Option<String>,
        results: Vec<ChatResult>,
    },
    Look {
        schema_version: String,
        bundle_name: String,
        requester_session: String,
        target_session: String,
        captured_at: String,
        #[serde(flatten)]
        snapshot: LookSnapshotPayload,
    },
    Raww {
        schema_version: String,
        status: String,
        target_session: String,
        transport: ListedSessionTransport,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
    },
    PermissionDecision {
        schema_version: String,
        status: String,
        permission_request_id: String,
        outcome: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason_code: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    PermissionList {
        schema_version: String,
        bundle_name: String,
        pending_requests: Vec<PendingPermissionEntry>,
    },
    Error {
        error: RelayError,
    },
}

/// One pending permission request entry returned by `RelayResponse::PermissionList`.
///
/// Field set mirrors the `permission.requested` stream event payload.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PendingPermissionEntry {
    pub message_id: String,
    pub permission_request_id: String,
    pub target_session: String,
    pub requested_kind: String,
    pub requested_details: Value,
    pub enqueued_at: String,
}

#[derive(Clone, Debug)]
pub(super) struct ChatRequestContext {
    request_id: Option<String>,
    sender_session: String,
    message: String,
    targets: Vec<String>,
    broadcast: bool,
    quiet_window_ms: Option<u64>,
    quiescence_timeout_ms: Option<u64>,
    acp_turn_timeout_ms: Option<u64>,
}

#[derive(Clone, Debug)]
pub(super) struct LookRequestContext {
    requester_session: String,
    target_session: String,
    lines: Option<usize>,
    bundle_name: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct RawwRequestContext {
    request_id: Option<String>,
    sender_session: String,
    target_session: String,
    text: String,
    no_enter: bool,
    bundle_name: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct PermissionDecisionRequestContext {
    permission_request_id: String,
    outcome: String,
    option_id: Option<String>,
    bundle_name: Option<String>,
    ui_session_id: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct RequestPrincipal {
    session_id: String,
}

#[derive(Clone, Debug)]
pub(super) struct AsyncDeliveryTask {
    bundle: BundleConfiguration,
    sender: crate::configuration::BundleMember,
    all_target_sessions: Vec<String>,
    target_session: String,
    target_is_ui: bool,
    message: String,
    message_id: String,
    quiescence: QuiescenceOptions,
    batch_settings: PromptBatchSettings,
    runtime_directory: PathBuf,
    completion_sender: Option<std::sync::mpsc::Sender<Result<ChatResult, RelayError>>>,
    payload_mode: DeliveryPayloadMode,
    append_enter: bool,
    permission_decider_sessions: Vec<String>,
    permission_max_pending: usize,
}

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

pub(super) fn dispatch_request(
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

pub(super) fn map_config(error: ConfigurationError) -> RelayError {
    match error {
        ConfigurationError::UnknownBundle { bundle_name, path } => relay_error(
            "validation_unknown_bundle",
            "bundle is not configured",
            Some(json!({"bundle_name": bundle_name, "path": path})),
        ),
        ConfigurationError::InvalidConfiguration { path, message } => relay_error(
            "internal_unexpected_failure",
            "bundle configuration is invalid",
            Some(json!({"path": path, "cause": message})),
        ),
        ConfigurationError::InvalidGroupName { path, group_name } => relay_error(
            "validation_invalid_group_name",
            "bundle configuration uses invalid group name",
            Some(json!({"path": path, "group_name": group_name})),
        ),
        ConfigurationError::ReservedGroupName { path, group_name } => relay_error(
            "validation_reserved_group_name",
            "bundle configuration uses reserved group name",
            Some(json!({"path": path, "group_name": group_name})),
        ),
        ConfigurationError::AmbiguousSender {
            working_directory,
            matches,
        } => relay_error(
            "validation_unknown_sender",
            "sender association is ambiguous",
            Some(json!({"working_directory": working_directory, "matches": matches})),
        ),
        ConfigurationError::Io { context, source } => relay_error(
            "internal_unexpected_failure",
            "bundle configuration could not be loaded",
            Some(json!({"context": context, "cause": source.to_string()})),
        ),
    }
}

pub(super) fn relay_error(code: &str, message: &str, details: Option<Value>) -> RelayError {
    RelayError {
        code: code.to_string(),
        message: message.to_string(),
        details,
    }
}

/// Builds the structured error for a session whose declared session type does
/// not yet have an implemented delivery path (`ui`, `pubsub`).
pub(super) fn session_type_not_implemented(
    session_id: &str,
    session_type: SessionType,
) -> RelayError {
    relay_error(
        "runtime_session_type_not_implemented",
        "session type delivery is not yet implemented",
        Some(json!({
            "session_id": session_id,
            "session_type": session_type,
        })),
    )
}

pub(super) fn map_tui_config(error: ConfigurationError) -> RelayError {
    match error {
        ConfigurationError::InvalidConfiguration { path, message } => relay_error(
            "validation_invalid_arguments",
            "tui configuration is invalid",
            Some(json!({"path": path, "cause": message})),
        ),
        ConfigurationError::Io { context, source } => relay_error(
            "validation_invalid_arguments",
            "failed to load tui configuration",
            Some(json!({"context": context, "cause": source.to_string()})),
        ),
        other => relay_error(
            "validation_invalid_arguments",
            "failed to load tui configuration",
            Some(json!({"cause": other.to_string()})),
        ),
    }
}
