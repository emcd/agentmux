use std::{
    fs,
    io::{self, BufRead, BufReader, Write},
    os::unix::{io::AsRawFd, net::UnixStream},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::runtime::{inscriptions::emit_inscription, paths::session_identity_psk_path};

use super::identity::SOCKET_TRUST_TOKEN;
use super::{RelayRequest, RelayResponse, RelayStreamEvent, SCHEMA_VERSION, canonical_session_id};

const RELAY_STREAM_HELLO_ACK_TIMEOUT: Duration = Duration::from_secs(2);
const RELAY_STREAM_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
const RELAY_STREAM_READ_POLL_INTERVAL: Duration = Duration::from_millis(100);
const HELLO_CONFLICT_RETRY_TIMEOUT_MS: u64 = 1_000;
/// First nominal backoff interval for connection retries. Doubles each attempt
/// up to `CONNECT_RETRY_BACKOFF_CAP_MS`.
const CONNECT_RETRY_BACKOFF_BASE_MS: u64 = 50;
/// Ceiling on a single nominal backoff interval so a long deadline still yields
/// several attempts rather than one oversized sleep.
const CONNECT_RETRY_BACKOFF_CAP_MS: u64 = 400;

/// Exponential backoff with equal jitter for connection retries inside the
/// hello-conflict deadline. A duplicate-principal Hello storm — many clients
/// claiming the same identity at once — would otherwise hammer the relay with a
/// fixed-interval accept/close storm; jittered backoff decorrelates the herd.
/// The nominal interval doubles each attempt up to `CONNECT_RETRY_BACKOFF_CAP_MS`,
/// and each sleep is drawn uniformly from `[nominal/2, nominal]` so retries
/// always make forward progress and never collapse back into a tight loop.
struct ConnectRetryBackoff {
    nominal_ms: u64,
}

impl ConnectRetryBackoff {
    fn new() -> Self {
        Self {
            nominal_ms: CONNECT_RETRY_BACKOFF_BASE_MS,
        }
    }

    /// Returns the next jittered sleep and advances the backoff. The sleep is
    /// clamped to `remaining` so the overall retry never overshoots the deadline.
    fn next_sleep(&mut self, remaining: Duration) -> Duration {
        let nominal = self.nominal_ms;
        let jittered = rand::random_range((nominal / 2)..=nominal);
        self.nominal_ms = nominal.saturating_mul(2).min(CONNECT_RETRY_BACKOFF_CAP_MS);
        Duration::from_millis(jittered).min(remaining)
    }
}

#[derive(Debug)]
pub struct RelayStreamSession {
    socket_path: PathBuf,
    namespace: String,
    session_id: String,
    /// An explicit identity token that overrides the credential-path lookup.
    /// Set for an outbound peer relay connection, which presents a peer PSK
    /// rather than a session's provisioned credential or the socket-trust
    /// sentinel; `None` for an ordinary session client.
    identity_token_override: Option<String>,
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
        principal_id: &'a str,
        identity_token: &'a str,
    },
    Request {
        request_id: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        namespace: Option<&'a str>,
        request: &'a RelayRequest,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "frame", rename_all = "snake_case")]
enum StreamServerFrame {
    HelloAck {
        schema_version: String,
        principal_id: String,
    },
    Response {
        request_id: Option<String>,
        response: RelayResponse,
    },
    Event {
        event: RelayStreamEvent,
    },
}

impl RelayStreamSession {
    /// Creates a persistent relay stream session descriptor.
    #[must_use]
    pub fn new(socket_path: PathBuf, namespace: String, session_id: String) -> Self {
        Self {
            socket_path,
            namespace,
            session_id,
            identity_token_override: None,
            connection: None,
        }
    }

    /// Creates an outbound peer relay session that presents `<relay_id>@RELAY`
    /// with an explicit peer PSK.
    ///
    /// Unlike [`RelayStreamSession::new`], the identity token is supplied
    /// directly (read from the peer credential path by the caller) rather than
    /// resolved from a session credential path, since a peer relay authenticates
    /// as this relay's own `@RELAY` principal, not a bundle session.
    #[must_use]
    pub(crate) fn for_peer_relay(
        socket_path: PathBuf,
        relay_id: String,
        identity_token: String,
    ) -> Self {
        Self {
            socket_path,
            namespace: super::RELAY_NAMESPACE.to_string(),
            session_id: relay_id,
            identity_token_override: Some(identity_token),
            connection: None,
        }
    }

    /// Forces connection establishment (dial + Hello handshake) without sending
    /// a request, so a caller can validate reachability and authentication up
    /// front. Used by the outbound peer connection manager for lazy
    /// establishment; a still-unestablished connection is retried on the next
    /// call with the same jittered backoff as [`RelayStreamSession::request`].
    pub(crate) fn connect(&mut self) -> Result<(), io::Error> {
        self.ensure_connected()
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
    /// The wire-envelope namespace is resolved from the request type: omitted
    /// for suffix-routed `Send`/`Raww`, otherwise the session's bound bundle.
    /// Callers that need to route a namespace-selected request elsewhere (notably
    /// `List` with the relay-wide `GLOBAL` namespace) use
    /// `request_with_namespace_and_events`.
    ///
    /// # Errors
    ///
    /// Returns IO errors when relay transport or frame exchange fails.
    pub fn request_with_events(
        &mut self,
        request: &RelayRequest,
    ) -> Result<(RelayResponse, Vec<RelayStreamEvent>), io::Error> {
        self.request_with_namespace_and_events(request, None)
    }

    /// Sends one request, resolving the wire-envelope namespace from the request
    /// type and an optional override.
    ///
    /// Suffix-routed operations (`Send`, `Raww`) carry no wire namespace: the
    /// relay derives their routing bundle from each target's `@<bundle>` suffix,
    /// so a namespace is meaningless and is always omitted regardless of caller.
    /// Every other (namespace-selected) operation uses `namespace` when provided,
    /// else falls back to the connection's bound bundle. The override exists for
    /// routing exceptions such as `List` against the relay-wide `GLOBAL`
    /// namespace.
    ///
    /// # Errors
    ///
    /// Returns IO errors when relay transport or frame exchange fails.
    pub fn request_with_namespace_and_events(
        &mut self,
        request: &RelayRequest,
        namespace: Option<&str>,
    ) -> Result<(RelayResponse, Vec<RelayStreamEvent>), io::Error> {
        self.ensure_connected()?;
        let request_id = uuid::Uuid::new_v4().to_string();
        let bound = self.namespace.clone();
        let wire_namespace = match request {
            RelayRequest::Send { .. } | RelayRequest::Raww { .. } => None,
            _ => Some(namespace.unwrap_or(bound.as_str())),
        };
        let result = {
            let connection = self
                .connection
                .as_mut()
                .ok_or_else(|| io::Error::other("relay stream connection is missing"))?;
            send_stream_client_frame(
                &mut connection.stream,
                StreamClientFrame::Request {
                    request_id: request_id.as_str(),
                    namespace: wire_namespace,
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

    /// Probes the held connection with a non-blocking 1-byte peek so a dropped
    /// relay socket is observed before a request write. Without this, the next
    /// `request_*` call writes into a half-closed socket, then blocks on the
    /// response read until the 5-second response timeout fires.
    ///
    /// Uses `libc::recv` with `MSG_PEEK | MSG_DONTWAIT` because the standard
    /// `UnixStream::peek` is still unstable.
    fn peek_connection_liveness(&mut self) -> Liveness {
        let Some(connection) = self.connection.as_ref() else {
            return Liveness::Dead {
                reason: "no_connection",
            };
        };
        let fd = connection.stream.as_raw_fd();
        let mut buf = [0u8; 1];
        // SAFETY: `fd` is owned by the live `UnixStream` for the duration of
        // this call; `buf` is a valid writable byte slice.
        let observed = unsafe {
            libc::recv(
                fd,
                buf.as_mut_ptr().cast(),
                buf.len(),
                libc::MSG_PEEK | libc::MSG_DONTWAIT,
            )
        };
        if observed == 0 {
            return Liveness::Dead { reason: "eof" };
        }
        if observed > 0 {
            return Liveness::Alive;
        }
        let source = io::Error::last_os_error();
        if source.kind() == io::ErrorKind::WouldBlock {
            return Liveness::Alive;
        }
        if is_retriable_stream_error(Some(&source)) {
            return Liveness::Dead { reason: "io_error" };
        }
        Liveness::Error(source)
    }

    fn ensure_connected(&mut self) -> Result<(), io::Error> {
        if self.connection.is_some() {
            match self.peek_connection_liveness() {
                Liveness::Alive => return Ok(()),
                Liveness::Dead { reason } => {
                    emit_inscription(
                        "mcp.relay.reconnecting",
                        &json!({
                            "namespace": self.namespace,
                            "session_id": self.session_id,
                            "reason": reason,
                        }),
                    );
                    self.connection = None;
                }
                Liveness::Error(source) => return Err(source),
            }
        }
        let deadline = Instant::now() + Duration::from_millis(HELLO_CONFLICT_RETRY_TIMEOUT_MS);
        let mut backoff = ConnectRetryBackoff::new();
        loop {
            match self.try_connect_once() {
                Ok(connection) => {
                    self.connection = Some(connection);
                    return Ok(());
                }
                Err(ConnectAttemptError::IdentityClaimConflict { message }) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        // Surface an exhausted conflict retry as a timeout so
                        // `map_relay_request_failure` reports `relay_timeout`
                        // with the conflict message, rather than the opaque
                        // `internal_unexpected_failure` that `io::Error::other`
                        // falls through to.
                        return Err(io::Error::new(io::ErrorKind::TimedOut, message));
                    }
                    thread::sleep(backoff.next_sleep(remaining));
                }
                Err(ConnectAttemptError::Io(source)) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if is_retriable_connect_error(&source) && !remaining.is_zero() {
                        thread::sleep(backoff.next_sleep(remaining));
                        continue;
                    }
                    return Err(source);
                }
            }
        }
    }

    /// Reads this session's PSK from the well-known credential path, falling
    /// back to the socket-trust sentinel when no credential file is present or
    /// readable. The relay socket lives at `<state-root>/relay.sock`, so the
    /// state root is the socket's parent directory.
    fn read_identity_token(&self) -> String {
        if let Some(token) = self.identity_token_override.as_deref() {
            return token.to_string();
        }
        let Some(state_root) = self.socket_path.parent() else {
            return SOCKET_TRUST_TOKEN.to_string();
        };
        let psk_path = session_identity_psk_path(
            state_root,
            self.namespace.as_str(),
            self.session_id.as_str(),
        );
        match fs::read_to_string(&psk_path) {
            Ok(contents) => {
                let trimmed = contents.trim();
                if trimmed.is_empty() {
                    SOCKET_TRUST_TOKEN.to_string()
                } else {
                    trimmed.to_string()
                }
            }
            Err(_) => SOCKET_TRUST_TOKEN.to_string(),
        }
    }

    fn try_connect_once(&self) -> Result<RelayStreamConnection, ConnectAttemptError> {
        let principal_id = canonical_session_id(self.session_id.as_str(), self.namespace.as_str());
        let identity_token = self.read_identity_token();
        let mut stream = UnixStream::connect(&self.socket_path).map_err(ConnectAttemptError::Io)?;
        send_stream_client_frame(
            &mut stream,
            StreamClientFrame::Hello {
                schema_version: SCHEMA_VERSION,
                principal_id: principal_id.as_str(),
                identity_token: identity_token.as_str(),
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
                    principal_id: acked_principal_id,
                } => {
                    if schema_version != SCHEMA_VERSION {
                        return Err(ConnectAttemptError::Io(io::Error::other(format!(
                            "relay hello acknowledgement schema version mismatch: expected {}, got {}",
                            SCHEMA_VERSION, schema_version
                        ))));
                    }
                    if acked_principal_id != principal_id {
                        return Err(ConnectAttemptError::Io(io::Error::other(
                            "relay hello acknowledgement identity mismatch",
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

enum Liveness {
    Alive,
    Dead { reason: &'static str },
    Error(io::Error),
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

/// Sends one Hello + request frame to the relay socket and returns the parsed
/// response. The connection is opened, used for one exchange, and dropped.
///
/// # Errors
///
/// Returns IO errors when the relay transport or Hello/Request exchange fails.
pub fn request_relay(
    socket_path: &Path,
    namespace: &str,
    session_id: &str,
    request: &RelayRequest,
) -> Result<RelayResponse, io::Error> {
    let mut session = RelayStreamSession::new(
        socket_path.to_path_buf(),
        namespace.to_string(),
        session_id.to_string(),
    );
    session.request(request)
}
