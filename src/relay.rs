//! Relay IPC contract and message-routing implementation.

use std::{
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    configuration::{BundleConfiguration, ConfigurationError, load_bundle_configuration},
    envelope::{ENVELOPE_SCHEMA_VERSION, PromptBatchSettings},
    runtime::paths::BundleRuntimePaths,
};

mod authorization;
mod delivery;
mod handlers;
mod lifecycle;
mod tmux;

use self::authorization::load_authorization_context;
use self::delivery::QuiescenceOptions;

const SCHEMA_VERSION: &str = ENVELOPE_SCHEMA_VERSION;
const POLICIES_FILE: &str = "policies.toml";
const POLICIES_FORMAT_VERSION: u32 = 1;

/// Recipient metadata returned by `list`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Recipient {
    pub session_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// Per-target delivery result for one `chat` request.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ChatResult {
    pub target_session: String,
    pub message_id: String,
    pub outcome: ChatOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
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

/// Aggregate delivery status for `chat`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatStatus {
    Accepted,
    Success,
    Partial,
    Failure,
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

/// Chat delivery behavior requested by caller.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatDeliveryMode {
    Async,
    #[default]
    Sync,
}

/// Structured relay error object.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RelayError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Relay request protocol.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum RelayRequest {
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
        delivery_mode: ChatDeliveryMode,
        #[serde(default)]
        quiet_window_ms: Option<u64>,
        #[serde(default)]
        quiescence_timeout_ms: Option<u64>,
    },
    Look {
        requester_session: String,
        target_session: String,
        #[serde(default)]
        lines: Option<usize>,
        #[serde(default)]
        bundle_name: Option<String>,
    },
}

/// Relay response protocol.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RelayResponse {
    List {
        schema_version: String,
        bundle_name: String,
        recipients: Vec<Recipient>,
    },
    Chat {
        schema_version: String,
        bundle_name: String,
        request_id: Option<String>,
        sender_session: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        sender_display_name: Option<String>,
        delivery_mode: ChatDeliveryMode,
        status: ChatStatus,
        results: Vec<ChatResult>,
    },
    Look {
        schema_version: String,
        bundle_name: String,
        requester_session: String,
        target_session: String,
        captured_at: String,
        snapshot_lines: Vec<String>,
    },
    Error {
        error: RelayError,
    },
}

#[derive(Clone, Debug)]
pub(super) struct ChatRequestContext {
    request_id: Option<String>,
    sender_session: String,
    message: String,
    targets: Vec<String>,
    broadcast: bool,
    delivery_mode: ChatDeliveryMode,
    quiet_window_ms: Option<u64>,
    quiescence_timeout_ms: Option<u64>,
}

#[derive(Clone, Debug)]
pub(super) struct LookRequestContext {
    requester_session: String,
    target_session: String,
    lines: Option<usize>,
    bundle_name: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct AsyncDeliveryTask {
    bundle: BundleConfiguration,
    sender: crate::configuration::BundleMember,
    all_target_sessions: Vec<String>,
    target_session: String,
    message: String,
    message_id: String,
    quiescence: QuiescenceOptions,
    batch_settings: PromptBatchSettings,
    tmux_socket: PathBuf,
}

/// Handles one relay socket request/response exchange on a connected stream.
pub fn serve_connection(
    stream: &mut UnixStream,
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
) -> Result<(), io::Error> {
    let request = match read_request(stream) {
        Ok(value) => value,
        Err(source) => {
            let response = RelayResponse::Error {
                error: relay_error(
                    "validation_invalid_arguments",
                    "failed to parse relay request",
                    Some(json!({"cause": source.to_string()})),
                ),
            };
            write_response(stream, &response)?;
            return Ok(());
        }
    };

    let response = match handle_request(
        request,
        configuration_root,
        &bundle_paths.bundle_name,
        &bundle_paths.tmux_socket,
    ) {
        Ok(value) => value,
        Err(error) => RelayResponse::Error { error },
    };
    write_response(stream, &response)
}

/// Executes one relay request for a configured bundle.
pub fn handle_request(
    request: RelayRequest,
    configuration_root: &Path,
    bundle_name: &str,
    tmux_socket: &Path,
) -> Result<RelayResponse, RelayError> {
    let bundle = load_bundle_configuration(configuration_root, bundle_name).map_err(map_config)?;
    let authorization = load_authorization_context(configuration_root, &bundle)?;
    handlers::handle_request(request, &bundle, &authorization, tmux_socket)
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

/// Prunes managed sessions and reaps tmux server when safe during shutdown.
///
/// # Errors
///
/// Returns internal failures when tmux session operations fail.
pub fn shutdown_bundle_runtime(tmux_socket: &Path) -> Result<ShutdownReport, RelayError> {
    lifecycle::shutdown_bundle_runtime(tmux_socket)
}

/// Waits for async delivery workers to stop after shutdown is requested.
///
/// Returns the number of workers still running when timeout is reached.
#[must_use]
pub fn wait_for_async_delivery_shutdown(timeout: Duration) -> usize {
    delivery::wait_for_async_delivery_shutdown(timeout)
}

fn read_request(stream: &UnixStream) -> Result<RelayRequest, io::Error> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let read = reader.read_line(&mut line)?;
    if read == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "request line is empty",
        ));
    }
    serde_json::from_str::<RelayRequest>(line.trim_end()).map_err(io::Error::other)
}

fn write_response(stream: &mut UnixStream, response: &RelayResponse) -> Result<(), io::Error> {
    let encoded = serde_json::to_string(response).map_err(io::Error::other)?;
    stream.write_all(encoded.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()
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

/// Sends one request to relay socket and returns the parsed response.
pub fn request_relay(
    socket_path: &Path,
    request: &RelayRequest,
) -> Result<RelayResponse, io::Error> {
    let mut stream = UnixStream::connect(socket_path)?;
    let request_text = serde_json::to_string(request).map_err(io::Error::other)?;
    stream.write_all(request_text.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.shutdown(std::net::Shutdown::Write)?;

    let mut reader = BufReader::new(&mut stream);
    let mut line = String::new();
    let read = reader.read_line(&mut line)?;
    if read == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "relay returned empty response",
        ));
    }
    serde_json::from_str::<RelayResponse>(line.trim_end()).map_err(io::Error::other)
}
