//! Relay IPC contract and message-routing implementation.

use std::{
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    configuration::{
        BundleConfiguration, ConfigurationError, load_bundle_configuration, load_policy_ids,
        load_tui_configuration,
    },
    envelope::{ENVELOPE_SCHEMA_VERSION, PromptBatchSettings},
    runtime::paths::BundleRuntimePaths,
};

mod authorization;
mod delivery;
mod handlers;
mod lifecycle;
mod startup_state;
mod stream;
mod tmux;

use self::authorization::load_authorization_context;
use self::delivery::QuiescenceOptions;
use self::stream::{
    HelloFrame, IncomingFrame, OutgoingFrame, RegisterStreamOutcome, RelayClientClass,
    StreamRegistration, clone_stream_writer, parse_incoming_frame, register_stream,
    registration_is_current, unregister_stream, write_stream_frame_to_writer,
};

const SCHEMA_VERSION: &str = ENVELOPE_SCHEMA_VERSION;
const POLICIES_FILE: &str = "policies.toml";
const POLICIES_FORMAT_VERSION: u32 = 1;
const RELAY_STREAM_HELLO_ACK_TIMEOUT: Duration = Duration::from_secs(2);
const RELAY_STREAM_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
const RELAY_STREAM_READ_POLL_INTERVAL: Duration = Duration::from_millis(100);
const HELLO_CONFLICT_RETRY_INTERVAL_MS: u64 = 50;
const HELLO_CONFLICT_RETRY_TIMEOUT_MS: u64 = 1_000;

/// Transport class for one listed bundle session.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ListedSessionTransport {
    Tmux,
    Acp,
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

/// Relay stream endpoint class used for persistent client hello registration.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RelayStreamClientClass {
    Agent,
    Ui,
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

#[derive(Debug)]
pub struct RelayStreamSession {
    socket_path: PathBuf,
    bundle_name: String,
    session_id: String,
    client_class: RelayStreamClientClass,
    connection: Option<RelayStreamConnection>,
}

#[derive(Debug)]
struct RelayStreamConnection {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "frame", rename_all = "snake_case")]
enum StreamClientFrame<'a> {
    Hello {
        schema_version: &'a str,
        bundle_name: &'a str,
        session_id: &'a str,
        client_class: RelayStreamClientClass,
    },
    Request {
        request_id: &'a str,
        request: &'a RelayRequest,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "frame", rename_all = "snake_case")]
enum StreamServerFrame {
    HelloAck {
        schema_version: String,
        bundle_name: String,
        session_id: String,
        client_class: RelayStreamClientClass,
    },
    Response {
        request_id: Option<String>,
        response: RelayResponse,
    },
    Event {
        event: RelayStreamEvent,
    },
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
        delivery_mode: ChatDeliveryMode,
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
    tmux_socket: PathBuf,
    completion_sender: Option<std::sync::mpsc::Sender<Result<ChatResult, RelayError>>>,
}

/// Handles one relay socket request/response exchange on a connected stream.
pub fn serve_connection(
    stream: &mut UnixStream,
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
) -> Result<(), io::Error> {
    let writer = clone_stream_writer(stream)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    let mut registration = None::<StreamRegistration>;

    loop {
        line.clear();
        let read = match reader.read_line(&mut line) {
            Ok(read) => read,
            Err(source)
                if matches!(
                    source.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) && registration.is_none() =>
            {
                break;
            }
            Err(source) => return Err(source),
        };
        if read == 0 {
            break;
        }

        let trimmed = line.trim_end();
        let frame = match parse_incoming_frame(trimmed) {
            Ok(frame) => frame,
            Err(source) => {
                let response = RelayResponse::Error {
                    error: relay_error(
                        "validation_invalid_arguments",
                        "failed to parse relay request",
                        Some(json!({"cause": source.to_string()})),
                    ),
                };
                write_response(stream, &response)?;
                break;
            }
        };

        match frame {
            IncomingFrame::LegacyRequest(request) => {
                let response = dispatch_request(
                    request,
                    configuration_root,
                    &bundle_paths.bundle_name,
                    &bundle_paths.tmux_socket,
                );
                write_response(stream, &response)?;
            }
            IncomingFrame::Hello(hello) => {
                let response = handle_hello_frame(configuration_root, bundle_paths, &hello);
                match response {
                    Ok(()) => {
                        stream.set_read_timeout(None)?;
                        match register_stream(&hello, writer.clone())? {
                            RegisterStreamOutcome::Registered(value) => {
                                registration = Some(value);
                            }
                            RegisterStreamOutcome::IdentityClaimConflict {
                                existing_connection_id,
                            } => {
                                let mut details = serde_json::Map::new();
                                details.insert(
                                    "bundle_name".to_string(),
                                    Value::String(hello.bundle_name.clone()),
                                );
                                details.insert(
                                    "session_id".to_string(),
                                    Value::String(hello.session_id.clone()),
                                );
                                details.insert(
                                    "reason".to_string(),
                                    Value::String(
                                        "existing identity owner is still live".to_string(),
                                    ),
                                );
                                if let Some(value) = existing_connection_id {
                                    details.insert(
                                        "existing_connection_id".to_string(),
                                        Value::String(value),
                                    );
                                }
                                let error = relay_error(
                                    "runtime_identity_claim_conflict",
                                    "stream identity is already claimed by a live connection",
                                    Some(Value::Object(details)),
                                );
                                write_stream_frame_to_writer(
                                    &writer,
                                    OutgoingFrame::Response {
                                        request_id: None,
                                        response: &RelayResponse::Error { error },
                                    },
                                )?;
                                break;
                            }
                        }
                        write_stream_frame_to_writer(
                            &writer,
                            OutgoingFrame::HelloAck {
                                schema_version: SCHEMA_VERSION,
                                bundle_name: hello.bundle_name.as_str(),
                                session_id: hello.session_id.as_str(),
                                client_class: hello.client_class,
                            },
                        )?;
                    }
                    Err(error) => {
                        write_stream_frame_to_writer(
                            &writer,
                            OutgoingFrame::Response {
                                request_id: None,
                                response: &RelayResponse::Error { error },
                            },
                        )?;
                        break;
                    }
                }
            }
            IncomingFrame::Request {
                request_id,
                request,
            } => {
                let Some(active_registration) = registration.as_ref() else {
                    let error = relay_error(
                        "validation_missing_hello",
                        "stream request requires hello registration",
                        None,
                    );
                    write_stream_frame_to_writer(
                        &writer,
                        OutgoingFrame::Response {
                            request_id: request_id.as_deref(),
                            response: &RelayResponse::Error { error },
                        },
                    )?;
                    continue;
                };
                if !registration_is_current(active_registration)? {
                    let error = relay_error(
                        "validation_stale_stream_binding",
                        "stream binding has been replaced by a newer hello registration",
                        Some(json!({
                            "bundle_name": active_registration.bundle_name,
                            "session_id": active_registration.session_id,
                        })),
                    );
                    write_stream_frame_to_writer(
                        &writer,
                        OutgoingFrame::Response {
                            request_id: request_id.as_deref(),
                            response: &RelayResponse::Error { error },
                        },
                    )?;
                    break;
                }
                let response = dispatch_request(
                    request,
                    configuration_root,
                    &bundle_paths.bundle_name,
                    &bundle_paths.tmux_socket,
                );
                write_stream_frame_to_writer(
                    &writer,
                    OutgoingFrame::Response {
                        request_id: request_id.as_deref(),
                        response: &response,
                    },
                )?;
            }
        }
    }

    if let Some(registration) = registration.as_ref() {
        unregister_stream(registration)?;
    }
    Ok(())
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

impl RelayStreamSession {
    /// Creates a persistent relay stream session descriptor.
    #[must_use]
    pub fn new(
        socket_path: PathBuf,
        bundle_name: String,
        session_id: String,
        client_class: RelayStreamClientClass,
    ) -> Self {
        Self {
            socket_path,
            bundle_name,
            session_id,
            client_class,
            connection: None,
        }
    }

    /// Sends one request over a persistent stream and returns response.
    ///
    /// Stream events received while waiting for response are discarded.
    ///
    /// # Errors
    ///
    /// Returns IO errors when relay transport or frame exchange fails.
    pub fn request(&mut self, request: &RelayRequest) -> Result<RelayResponse, io::Error> {
        let (response, _events) = self.request_with_events(request)?;
        Ok(response)
    }

    /// Sends one request over a persistent stream and returns response + events.
    ///
    /// # Errors
    ///
    /// Returns IO errors when relay transport or frame exchange fails.
    pub fn request_with_events(
        &mut self,
        request: &RelayRequest,
    ) -> Result<(RelayResponse, Vec<RelayStreamEvent>), io::Error> {
        self.ensure_connected()?;
        let request_id = uuid::Uuid::new_v4().to_string();
        let result = {
            let connection = self
                .connection
                .as_mut()
                .ok_or_else(|| io::Error::other("relay stream connection is missing"))?;
            send_stream_client_frame(
                &mut connection.stream,
                StreamClientFrame::Request {
                    request_id: request_id.as_str(),
                    request,
                },
            )?;
            read_stream_response_frame(connection, request_id.as_str())
        };
        if let Err(source) = &result
            && is_retriable_stream_error(Some(source))
        {
            // Preserve deterministic request semantics: if transport fails after a
            // request is written, do not auto-replay side-effecting operations.
            // Drop the connection so the next call performs a fresh hello/connect.
            self.connection = None;
        }
        result
    }

    /// Polls pending relay stream events without sending a request.
    ///
    /// Non-event frames are ignored.
    ///
    /// # Errors
    ///
    /// Returns IO errors when the stream cannot be established or read.
    pub fn poll_events(&mut self) -> Result<Vec<RelayStreamEvent>, io::Error> {
        self.ensure_connected()?;
        let result = {
            let connection = self
                .connection
                .as_mut()
                .ok_or_else(|| io::Error::other("relay stream connection is missing"))?;
            poll_stream_events_nonblocking(connection)
        };
        if let Err(source) = &result
            && is_retriable_stream_error(Some(source))
        {
            self.connection = None;
        }
        result
    }

    fn ensure_connected(&mut self) -> Result<(), io::Error> {
        if self.connection.is_some() {
            return Ok(());
        }
        let deadline = Instant::now() + Duration::from_millis(HELLO_CONFLICT_RETRY_TIMEOUT_MS);
        loop {
            match self.try_connect_once() {
                Ok(connection) => {
                    self.connection = Some(connection);
                    return Ok(());
                }
                Err(ConnectAttemptError::IdentityClaimConflict { message }) => {
                    if Instant::now() >= deadline {
                        return Err(io::Error::other(message));
                    }
                    thread::sleep(Duration::from_millis(HELLO_CONFLICT_RETRY_INTERVAL_MS));
                }
                Err(ConnectAttemptError::Io(source)) => {
                    if is_retriable_connect_error(&source) && Instant::now() < deadline {
                        thread::sleep(Duration::from_millis(HELLO_CONFLICT_RETRY_INTERVAL_MS));
                        continue;
                    }
                    return Err(source);
                }
            }
        }
    }

    fn try_connect_once(&self) -> Result<RelayStreamConnection, ConnectAttemptError> {
        let mut stream = UnixStream::connect(&self.socket_path).map_err(ConnectAttemptError::Io)?;
        send_stream_client_frame(
            &mut stream,
            StreamClientFrame::Hello {
                schema_version: SCHEMA_VERSION,
                bundle_name: self.bundle_name.as_str(),
                session_id: self.session_id.as_str(),
                client_class: self.client_class,
            },
        )
        .map_err(ConnectAttemptError::Io)?;
        let mut reader = BufReader::new(stream.try_clone().map_err(ConnectAttemptError::Io)?);
        stream
            .set_read_timeout(Some(RELAY_STREAM_HELLO_ACK_TIMEOUT))
            .map_err(ConnectAttemptError::Io)?;
        loop {
            let mut line = String::new();
            let read = match reader.read_line(&mut line) {
                Ok(read) => read,
                Err(source) if source.kind() == io::ErrorKind::Interrupted => continue,
                Err(source)
                    if source.kind() == io::ErrorKind::TimedOut
                        || source.kind() == io::ErrorKind::WouldBlock =>
                {
                    return Err(ConnectAttemptError::Io(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "relay hello acknowledgement timed out",
                    )));
                }
                Err(source) => return Err(ConnectAttemptError::Io(source)),
            };
            if read == 0 {
                return Err(ConnectAttemptError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "relay stream closed before hello acknowledgement",
                )));
            }
            let server_frame =
                parse_server_frame(line.trim_end()).map_err(ConnectAttemptError::Io)?;
            match server_frame {
                StreamServerFrame::HelloAck {
                    schema_version,
                    bundle_name,
                    session_id,
                    client_class,
                } => {
                    if schema_version != SCHEMA_VERSION {
                        return Err(ConnectAttemptError::Io(io::Error::other(format!(
                            "relay hello acknowledgement schema version mismatch: expected {}, got {}",
                            SCHEMA_VERSION, schema_version
                        ))));
                    }
                    if bundle_name != self.bundle_name || session_id != self.session_id {
                        return Err(ConnectAttemptError::Io(io::Error::other(
                            "relay hello acknowledgement identity mismatch",
                        )));
                    }
                    if client_class != self.client_class {
                        return Err(ConnectAttemptError::Io(io::Error::other(
                            "relay hello acknowledgement class mismatch",
                        )));
                    }
                    if let Err(source) = stream.set_read_timeout(None)
                        && !is_ignorable_socket_option_error(&source)
                    {
                        return Err(ConnectAttemptError::Io(source));
                    }
                    return Ok(RelayStreamConnection { stream, reader });
                }
                StreamServerFrame::Response {
                    response: RelayResponse::Error { error },
                    ..
                } => {
                    let message =
                        format!("relay hello rejected [{}]: {}", error.code, error.message);
                    if error.code == "runtime_identity_claim_conflict" {
                        return Err(ConnectAttemptError::IdentityClaimConflict { message });
                    }
                    return Err(ConnectAttemptError::Io(io::Error::other(message)));
                }
                StreamServerFrame::Response { response, .. } => {
                    return Err(ConnectAttemptError::Io(io::Error::other(format!(
                        "unexpected relay hello response frame: {response:?}",
                    ))));
                }
                StreamServerFrame::Event { .. } => {}
            }
        }
    }
}

enum ConnectAttemptError {
    Io(io::Error),
    IdentityClaimConflict { message: String },
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
    tmux_socket: &Path,
) -> Result<BundleStartupReport, RelayError> {
    lifecycle::startup_bundle(configuration_root, bundle_name, tmux_socket)
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

fn write_response(stream: &mut UnixStream, response: &RelayResponse) -> Result<(), io::Error> {
    let encoded = serde_json::to_string(response).map_err(io::Error::other)?;
    stream.write_all(encoded.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()
}

fn send_stream_client_frame(
    stream: &mut UnixStream,
    frame: StreamClientFrame<'_>,
) -> Result<(), io::Error> {
    let encoded = serde_json::to_string(&frame).map_err(io::Error::other)?;
    stream.write_all(encoded.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()
}

fn parse_server_frame(line: &str) -> Result<StreamServerFrame, io::Error> {
    serde_json::from_str::<StreamServerFrame>(line).map_err(io::Error::other)
}

fn read_stream_response_frame(
    connection: &mut RelayStreamConnection,
    request_id: &str,
) -> Result<(RelayResponse, Vec<RelayStreamEvent>), io::Error> {
    connection
        .stream
        .set_read_timeout(Some(RELAY_STREAM_READ_POLL_INTERVAL))?;
    let deadline = Instant::now() + RELAY_STREAM_RESPONSE_TIMEOUT;
    let mut events = Vec::new();
    let result = loop {
        if Instant::now() >= deadline {
            break Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "relay stream response timed out",
            ));
        }
        let mut line = String::new();
        let read = match connection.reader.read_line(&mut line) {
            Ok(read) => read,
            Err(source) if source.kind() == io::ErrorKind::Interrupted => continue,
            Err(source)
                if source.kind() == io::ErrorKind::TimedOut
                    || source.kind() == io::ErrorKind::WouldBlock =>
            {
                continue;
            }
            Err(source) => break Err(source),
        };
        if read == 0 {
            break Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "relay stream closed while waiting for response",
            ));
        }
        let parsed = parse_server_frame(line.trim_end())?;
        match parsed {
            StreamServerFrame::Event { event } => events.push(event),
            StreamServerFrame::HelloAck { .. } => {}
            StreamServerFrame::Response {
                request_id: frame_request_id,
                response,
            } => {
                if frame_request_id.as_deref() == Some(request_id) {
                    break Ok((response, events));
                }
            }
        }
    };
    let reset = connection.stream.set_read_timeout(None);
    if let Err(source) = reset
        && result.is_ok()
        && !is_ignorable_socket_option_error(&source)
    {
        return Err(source);
    }
    result
}

fn is_retriable_stream_error(error: Option<&io::Error>) -> bool {
    let Some(error) = error else {
        return false;
    };
    matches!(
        error.kind(),
        io::ErrorKind::NotConnected
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::TimedOut
            | io::ErrorKind::UnexpectedEof
    )
}

fn is_retriable_connect_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotConnected
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::TimedOut
            | io::ErrorKind::WouldBlock
            | io::ErrorKind::Interrupted
            | io::ErrorKind::InvalidInput
    )
}

fn poll_stream_events_nonblocking(
    connection: &mut RelayStreamConnection,
) -> Result<Vec<RelayStreamEvent>, io::Error> {
    connection.stream.set_nonblocking(true)?;
    let mut events = Vec::new();
    let read_result = loop {
        let mut line = String::new();
        match connection.reader.read_line(&mut line) {
            Ok(0) => {
                break Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "relay stream closed while polling events",
                ));
            }
            Ok(_) => {
                let frame = parse_server_frame(line.trim_end())?;
                if let StreamServerFrame::Event { event } = frame {
                    events.push(event);
                }
            }
            Err(source) if source.kind() == io::ErrorKind::WouldBlock => break Ok(()),
            Err(source) if source.kind() == io::ErrorKind::Interrupted => continue,
            Err(source) => break Err(source),
        }
    };
    let reset = connection.stream.set_nonblocking(false);
    read_result?;
    if let Err(source) = reset
        && !is_ignorable_socket_option_error(&source)
    {
        return Err(source);
    }
    Ok(events)
}

fn is_ignorable_socket_option_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotConnected
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::TimedOut
            | io::ErrorKind::UnexpectedEof
            | io::ErrorKind::InvalidInput
    )
}

fn dispatch_request(
    request: RelayRequest,
    configuration_root: &Path,
    bundle_name: &str,
    tmux_socket: &Path,
) -> RelayResponse {
    match handle_request(request, configuration_root, bundle_name, tmux_socket) {
        Ok(value) => value,
        Err(error) => RelayResponse::Error { error },
    }
}

fn handle_hello_frame(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    hello: &HelloFrame,
) -> Result<(), RelayError> {
    if hello.schema_version != SCHEMA_VERSION {
        return Err(relay_error(
            "validation_invalid_schema_version",
            "hello schema_version is not supported",
            Some(json!({
                "schema_version": hello.schema_version,
                "supported_schema_version": SCHEMA_VERSION,
            })),
        ));
    }
    if hello.bundle_name != bundle_paths.bundle_name {
        return Err(relay_error(
            "validation_cross_bundle_unsupported",
            "hello bundle_name does not match associated bundle",
            Some(json!({
                "associated_bundle_name": bundle_paths.bundle_name,
                "hello_bundle_name": hello.bundle_name,
            })),
        ));
    }
    match hello.client_class {
        RelayClientClass::Agent => {
            let bundle = load_bundle_configuration(configuration_root, &bundle_paths.bundle_name)
                .map_err(map_config)?;
            if bundle
                .members
                .iter()
                .any(|member| member.id == hello.session_id)
            {
                return Ok(());
            }
            Err(relay_error(
                "validation_unknown_sender",
                "hello session_id is not configured in associated bundle",
                Some(json!({
                    "bundle_name": bundle.bundle_name,
                    "session_id": hello.session_id,
                })),
            ))
        }
        RelayClientClass::Ui => {
            let Some(tui_configuration) =
                load_tui_configuration(configuration_root).map_err(map_tui_config)?
            else {
                return Err(relay_error(
                    "validation_unknown_sender",
                    "hello session_id is not configured in global tui sessions",
                    Some(json!({
                        "bundle_name": bundle_paths.bundle_name,
                        "session_id": hello.session_id,
                    })),
                ));
            };
            let Some(tui_session) = tui_configuration.session_by_id(hello.session_id.as_str())
            else {
                return Err(relay_error(
                    "validation_unknown_sender",
                    "hello session_id is not configured in global tui sessions",
                    Some(json!({
                        "bundle_name": bundle_paths.bundle_name,
                        "session_id": hello.session_id,
                    })),
                ));
            };
            let policy_ids = load_policy_ids(configuration_root).map_err(map_tui_config)?;
            if policy_ids.contains(tui_session.policy_id.as_str()) {
                return Ok(());
            }
            Err(relay_error(
                "validation_unknown_policy",
                "ui session policy references unknown policy id",
                Some(json!({
                    "session_id": tui_session.id,
                    "policy_id": tui_session.policy_id,
                })),
            ))
        }
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

fn map_tui_config(error: ConfigurationError) -> RelayError {
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
