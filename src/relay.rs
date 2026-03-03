//! Relay IPC contract and message-routing implementation.

use std::{
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::{
    configuration::{BundleConfiguration, ConfigurationError, load_bundle_configuration},
    runtime::paths::BundleRuntimePaths,
};

const SCHEMA_VERSION: &str = "1";
const DEFAULT_QUIET_WINDOW_MS: u64 = 750;
const DEFAULT_DELIVERY_TIMEOUT_MS: u64 = 30_000;

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

/// Aggregate delivery status for `chat`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatStatus {
    Success,
    Partial,
    Failure,
}

/// Per-target delivery outcome for `chat`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatOutcome {
    Delivered,
    Timeout,
    Failed,
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
        quiet_window_ms: Option<u64>,
        #[serde(default)]
        delivery_timeout_ms: Option<u64>,
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
        status: ChatStatus,
        results: Vec<ChatResult>,
    },
    Error {
        error: RelayError,
    },
}

#[derive(Clone, Debug)]
struct ChatRequestContext {
    request_id: Option<String>,
    sender_session: String,
    message: String,
    targets: Vec<String>,
    broadcast: bool,
    quiet_window_ms: Option<u64>,
    delivery_timeout_ms: Option<u64>,
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
    match request {
        RelayRequest::List { sender_session } => Ok(handle_list(&bundle, sender_session)),
        RelayRequest::Chat {
            request_id,
            sender_session,
            message,
            targets,
            broadcast,
            quiet_window_ms,
            delivery_timeout_ms,
        } => handle_chat(
            &bundle,
            ChatRequestContext {
                request_id,
                sender_session,
                message,
                targets,
                broadcast,
                quiet_window_ms,
                delivery_timeout_ms,
            },
            tmux_socket,
        ),
    }
}

fn handle_list(bundle: &BundleConfiguration, sender_session: Option<String>) -> RelayResponse {
    let recipients = bundle
        .members
        .iter()
        .filter(|member| Some(member.session_name.as_str()) != sender_session.as_deref())
        .map(|member| Recipient {
            session_name: member.session_name.clone(),
            display_name: member.display_name.clone(),
        })
        .collect::<Vec<_>>();

    RelayResponse::List {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle_name: bundle.bundle_name.clone(),
        recipients,
    }
}

fn handle_chat(
    bundle: &BundleConfiguration,
    request: ChatRequestContext,
    tmux_socket: &Path,
) -> Result<RelayResponse, RelayError> {
    let ChatRequestContext {
        request_id,
        sender_session,
        message,
        targets,
        broadcast,
        quiet_window_ms,
        delivery_timeout_ms,
    } = request;

    if message.trim().is_empty() {
        return Err(relay_error(
            "validation_invalid_arguments",
            "message must be non-empty",
            None,
        ));
    }
    if !broadcast && targets.is_empty() {
        return Err(relay_error(
            "validation_empty_targets",
            "targets must contain at least one session",
            None,
        ));
    }
    if broadcast && !targets.is_empty() {
        return Err(relay_error(
            "validation_conflicting_targets",
            "targets must be empty when broadcast=true",
            None,
        ));
    }

    let sender = bundle
        .members
        .iter()
        .find(|member| member.session_name == sender_session)
        .ok_or_else(|| {
            relay_error(
                "validation_unknown_sender",
                "sender_session is not in bundle configuration",
                Some(json!({"sender_session": sender_session})),
            )
        })?;

    let resolved_targets = if broadcast {
        bundle
            .members
            .iter()
            .map(|member| member.session_name.clone())
            .collect::<Vec<_>>()
    } else {
        targets
    };

    let unknown_targets = resolved_targets
        .iter()
        .filter(|target| {
            !bundle
                .members
                .iter()
                .any(|member| member.session_name == **target)
        })
        .cloned()
        .collect::<Vec<_>>();
    if !unknown_targets.is_empty() {
        return Err(relay_error(
            "validation_unknown_recipient",
            "one or more targets are not in bundle configuration",
            Some(json!({"unknown_targets": unknown_targets})),
        ));
    }

    let mut results = Vec::with_capacity(resolved_targets.len());
    let quiescence = QuiescenceOptions::new(quiet_window_ms, delivery_timeout_ms);
    for target_session in resolved_targets {
        let message_id = Uuid::new_v4().to_string();
        match wait_for_quiescent_pane(tmux_socket, &target_session, quiescence) {
            Ok(pane_target) => {
                let envelope = render_json_envelope(
                    &bundle.bundle_name,
                    &sender.session_name,
                    &target_session,
                    &message_id,
                    &message,
                );
                match inject_prompt(tmux_socket, &pane_target, &envelope) {
                    Ok(()) => {
                        results.push(ChatResult {
                            target_session,
                            message_id,
                            outcome: ChatOutcome::Delivered,
                            reason: None,
                        });
                    }
                    Err(reason) => {
                        results.push(ChatResult {
                            target_session,
                            message_id,
                            outcome: ChatOutcome::Failed,
                            reason: Some(reason),
                        });
                    }
                }
            }
            Err(DeliveryWaitError::Timeout { timeout }) => {
                results.push(ChatResult {
                    target_session,
                    message_id,
                    outcome: ChatOutcome::Timeout,
                    reason: Some(format!(
                        "quiescence wait timed out after {}ms",
                        timeout.as_millis()
                    )),
                });
            }
            Err(DeliveryWaitError::Failed { reason }) => {
                results.push(ChatResult {
                    target_session,
                    message_id,
                    outcome: ChatOutcome::Failed,
                    reason: Some(reason),
                });
            }
        }
    }

    Ok(RelayResponse::Chat {
        schema_version: SCHEMA_VERSION.to_string(),
        bundle_name: bundle.bundle_name.clone(),
        request_id,
        sender_session: sender.session_name.clone(),
        sender_display_name: sender.display_name.clone(),
        status: aggregate_chat_status(&results),
        results,
    })
}

fn aggregate_chat_status(results: &[ChatResult]) -> ChatStatus {
    let delivered = results
        .iter()
        .filter(|result| result.outcome == ChatOutcome::Delivered)
        .count();
    if delivered == results.len() {
        return ChatStatus::Success;
    }
    if delivered > 0 {
        return ChatStatus::Partial;
    }
    ChatStatus::Failure
}

fn render_json_envelope(
    bundle_name: &str,
    sender_session: &str,
    target_session: &str,
    message_id: &str,
    message: &str,
) -> String {
    let created_at = time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
    let envelope = json!({
        "schema_version": SCHEMA_VERSION,
        "message_id": message_id,
        "bundle_name": bundle_name,
        "sender_session": sender_session,
        "target_session": target_session,
        "created_at": created_at,
        "body": message,
    });
    serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| message.to_string())
}

#[derive(Clone, Copy, Debug)]
struct QuiescenceOptions {
    quiet_window: Duration,
    delivery_timeout: Duration,
}

impl Default for QuiescenceOptions {
    fn default() -> Self {
        Self {
            quiet_window: Duration::from_millis(DEFAULT_QUIET_WINDOW_MS),
            delivery_timeout: Duration::from_millis(DEFAULT_DELIVERY_TIMEOUT_MS),
        }
    }
}

impl QuiescenceOptions {
    fn new(quiet_window_ms: Option<u64>, delivery_timeout_ms: Option<u64>) -> Self {
        Self {
            quiet_window: Duration::from_millis(
                quiet_window_ms
                    .filter(|value| *value > 0)
                    .unwrap_or(DEFAULT_QUIET_WINDOW_MS),
            ),
            delivery_timeout: Duration::from_millis(
                delivery_timeout_ms
                    .filter(|value| *value > 0)
                    .unwrap_or(DEFAULT_DELIVERY_TIMEOUT_MS),
            ),
        }
    }
}

#[derive(Debug)]
enum DeliveryWaitError {
    Timeout { timeout: Duration },
    Failed { reason: String },
}

fn wait_for_quiescent_pane(
    tmux_socket: &Path,
    target_session: &str,
    options: QuiescenceOptions,
) -> Result<String, DeliveryWaitError> {
    let deadline = Instant::now() + options.delivery_timeout;
    loop {
        let pane_before = resolve_active_pane_target(tmux_socket, target_session)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let snapshot_before = capture_pane_snapshot(tmux_socket, &pane_before)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;

        thread::sleep(options.quiet_window);

        let pane_after = resolve_active_pane_target(tmux_socket, target_session)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        let snapshot_after = capture_pane_snapshot(tmux_socket, &pane_after)
            .map_err(|reason| DeliveryWaitError::Failed { reason })?;
        if pane_before == pane_after && snapshot_before == snapshot_after {
            return Ok(pane_after);
        }

        if Instant::now() >= deadline {
            return Err(DeliveryWaitError::Timeout {
                timeout: options.delivery_timeout,
            });
        }
    }
}

fn resolve_active_pane_target(tmux_socket: &Path, target_session: &str) -> Result<String, String> {
    let output = run_tmux_command(
        tmux_socket,
        &["display-message", "-p", "-t", target_session, "#{pane_id}"],
    )?;
    let pane_target = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if pane_target.is_empty() {
        return Err(format!(
            "tmux did not return an active pane for session {target_session}"
        ));
    }
    Ok(pane_target)
}

fn capture_pane_snapshot(tmux_socket: &Path, pane_target: &str) -> Result<String, String> {
    let output = run_tmux_command(
        tmux_socket,
        &["capture-pane", "-p", "-t", pane_target, "-S", "-200"],
    )?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn inject_prompt(tmux_socket: &Path, pane_target: &str, prompt: &str) -> Result<(), String> {
    run_tmux_command(
        tmux_socket,
        &["send-keys", "-t", pane_target, "--", prompt, "Enter"],
    )?;
    Ok(())
}

fn run_tmux_command(
    tmux_socket: &Path,
    command_arguments: &[&str],
) -> Result<std::process::Output, String> {
    let mut command = Command::new(tmux_program());
    command.arg("-S").arg(tmux_socket).args(command_arguments);
    let output = command.output().map_err(|source| source.to_string())?;
    if output.status.success() {
        return Ok(output);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let command_name = command_arguments.first().copied().unwrap_or("tmux");
    if stderr.is_empty() {
        return Err(format!("tmux {command_name} failed"));
    }
    Err(stderr)
}

fn tmux_program() -> String {
    std::env::var("TMUXMUX_TMUX_COMMAND")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "tmux".to_string())
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

fn map_config(error: ConfigurationError) -> RelayError {
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

fn relay_error(code: &str, message: &str, details: Option<Value>) -> RelayError {
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
