//! NDJSON framing, outbox bounds, and the control lane.
//!
//! Reads newline-delimited UTF-8 JSON-RPC 2.0 frames, bounding the
//! accumulated inbound bytes before the terminating newline (no length
//! prefix on the wire): the 64KiB bootstrap ceiling before negotiation,
//! the negotiated `max_frame_bytes` as the operative bound after. Owns
//! the transmit queue caps with the perspective rule, the bounded drain
//! wait, and the control lane exempt from data caps within its own bound.

use std::{
    error::Error,
    fmt::{Display, Formatter},
    io::{self, BufRead, Write},
};

use serde_json::{Value, json};

/// Pre-negotiation inbound frame ceiling: the bootstrap bound that
/// applies to the hello exchange before negotiated limits exist.
pub const BOOTSTRAP_MAX_FRAME_BYTES: usize = 64 * 1024;

/// Framing failure for NDJSON read, parse, and write paths.
#[derive(Debug)]
pub enum FrameError {
    Oversize { bound: usize },
    Unparseable { reason: String },
    MessageTooLarge { bound: usize },
    Io { context: String, source: io::Error },
}

impl FrameError {
    fn oversize(bound: usize) -> Self {
        Self::Oversize { bound }
    }

    fn unparseable(reason: impl Into<String>) -> Self {
        Self::Unparseable {
            reason: reason.into(),
        }
    }

    fn message_too_large(bound: usize) -> Self {
        Self::MessageTooLarge { bound }
    }

    /// Wraps an I/O failure with the operation it interrupted.
    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

impl Display for FrameError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Oversize { bound } => {
                write!(
                    formatter,
                    "frame exceeds the inbound bound of {bound} bytes"
                )
            },
            Self::Unparseable { reason } => {
                write!(formatter, "frame is not parseable JSON: {reason}")
            },
            Self::MessageTooLarge { bound } => {
                write!(
                    formatter,
                    "outbound frame exceeds the bound of {bound} bytes"
                )
            },
            Self::Io { context, source } => {
                write!(formatter, "{context}: {source}")
            },
        }
    }
}

impl Error for FrameError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Reads one NDJSON frame, returning its bytes without the terminator.
///
/// Returns `Ok(None)` on clean EOF (no bytes pending). Bytes accumulate
/// only up to `bound + 1`; a frame that outgrows the bound fails as
/// [`FrameError::Oversize`] without ever holding a body-sized buffer. A
/// nonempty stream that ends before any newline fails as unparseable.
///
/// # Errors
///
/// Returns [`FrameError::Oversize`] past the bound, [`FrameError::Io`]
/// on read failure, [`FrameError::Unparseable`] on a truncated frame.
pub fn read_frame(
    reader: &mut (impl BufRead + ?Sized),
    bound: usize,
) -> Result<Option<Vec<u8>>, FrameError> {
    let mut frame: Vec<u8> = Vec::new();
    // Byte-wise over the buffered reader: one syscall per buffer fill,
    // and bytes past the newline stay buffered for the next frame.
    // Allocation never exceeds `bound + 1` no matter how large the
    // peer's frame grows.
    let mut byte = [0_u8; 1];
    loop {
        match reader.read_exact(&mut byte) {
            Ok(()) => {},
            Err(source) if source.kind() == io::ErrorKind::UnexpectedEof => {
                break;
            },
            Err(source) => {
                return Err(FrameError::io("read frame", source));
            },
        }
        if byte[0] == b'\n' {
            if frame.last() == Some(&b'\r') {
                frame.pop();
            }
            return Ok(Some(frame));
        }
        if frame.len() >= bound.saturating_add(1) {
            return Err(FrameError::oversize(bound));
        }
        frame.push(byte[0]);
    }
    if frame.is_empty() {
        return Ok(None);
    }
    Err(FrameError::unparseable(
        "stream ended before the frame newline",
    ))
}

/// Writes one frame plus its newline and flushes, so the peer can read
/// promptly. Payloads over `bound` fail before any byte is written.
///
/// # Errors
///
/// Returns [`FrameError::MessageTooLarge`] past the bound and
/// [`FrameError::Io`] on write or flush failure.
pub fn write_frame(
    writer: &mut (impl Write + ?Sized),
    payload: &[u8],
    bound: usize,
) -> Result<(), FrameError> {
    if payload.len() > bound {
        return Err(FrameError::message_too_large(bound));
    }
    writer
        .write_all(payload)
        .map_err(|source| FrameError::io("write frame", source))?;
    writer
        .write_all(b"\n")
        .map_err(|source| FrameError::io("write frame newline", source))?;
    writer
        .flush()
        .map_err(|source| FrameError::io("flush frame", source))
}

/// Parses frame bytes as JSON.
///
/// # Errors
///
/// Returns [`FrameError::Unparseable`] for empty input or invalid JSON.
pub fn parse_frame(bytes: &[u8]) -> Result<Value, FrameError> {
    if bytes.is_empty() {
        return Err(FrameError::unparseable("empty frame carries no JSON"));
    }
    serde_json::from_slice(bytes)
        .map_err(|source| FrameError::unparseable(source.to_string()))
}

/// Builds the contract `-32000` JSON-RPC error object for a failed call.
///
/// `id` echoes the failed request's id when the frame supplied one;
/// `correlation_id` echoes the failed request's correlation when the
/// request carried one — except `correlation_invalid`, which is exempt
/// from the required echo and is admitted by pending-table match
/// instead.
#[must_use]
pub fn contract_error(
    id: Option<&Value>,
    code: &str,
    reason: &str,
    details: Option<Value>,
    correlation_id: Option<&str>,
) -> Value {
    let mut data = serde_json::Map::new();
    data.insert("code".to_string(), Value::String(code.to_string()));
    data.insert("reason".to_string(), Value::String(reason.to_string()));
    if let Some(details) = details {
        data.insert("details".to_string(), details);
    }
    if let Some(correlation_id) = correlation_id {
        data.insert(
            "correlation_id".to_string(),
            Value::String(correlation_id.to_string()),
        );
    }
    json!({
        "jsonrpc": "2.0",
        "id": id.cloned().unwrap_or(Value::Null),
        "error": {
            "code": -32000,
            "message": reason,
            "data": Value::Object(data),
        },
    })
}
