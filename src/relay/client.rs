use std::{
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use super::{RelayRequest, RelayResponse, RelayStreamEvent, SCHEMA_VERSION};

const RELAY_STREAM_HELLO_ACK_TIMEOUT: Duration = Duration::from_secs(2);
const RELAY_STREAM_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
const RELAY_STREAM_READ_POLL_INTERVAL: Duration = Duration::from_millis(100);
const HELLO_CONFLICT_RETRY_INTERVAL_MS: u64 = 50;
const HELLO_CONFLICT_RETRY_TIMEOUT_MS: u64 = 1_000;

#[derive(Debug)]
pub struct RelayStreamSession {
    socket_path: PathBuf,
    bundle_name: String,
    session_id: String,
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
    pub fn new(socket_path: PathBuf, bundle_name: String, session_id: String) -> Self {
        Self {
            socket_path,
            bundle_name,
            session_id,
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
                        // Surface an exhausted conflict retry as a timeout so
                        // `map_relay_request_failure` reports `relay_timeout`
                        // with the conflict message, rather than the opaque
                        // `internal_unexpected_failure` that `io::Error::other`
                        // falls through to.
                        return Err(io::Error::new(io::ErrorKind::TimedOut, message));
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
