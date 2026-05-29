use std::{
    collections::HashMap,
    io,
    sync::{Mutex, OnceLock},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::AsyncWriteExt,
    net::unix::OwnedWriteHalf,
    sync::mpsc::{self, UnboundedSender, error::SendError},
    task::JoinHandle,
    time::timeout,
};
use uuid::Uuid;

use crate::configuration::SessionType;
use crate::runtime::inscriptions::emit_inscription;

use super::identity::{PrincipalType, classify_principal_id};
use super::{RelayRequest, RelayResponse};

// Bounded write timeout for relay-to-client writes. A stalled client whose
// receive buffer is full must not pin the per-connection writer task
// indefinitely; this timeout bounds every write attempt. Override with
// `AGENTMUX_RELAY_CONNECTION_WRITE_TIMEOUT_MS`.
const RELAY_CONNECTION_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Hello handshake frame. Identity is fully described by `principal_id`
/// (`<id>@<namespace>` form); `identity_token` is the presented credential
/// (a raw PSK, or the `"socket-trust"` sentinel for unprovisioned sessions).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct HelloFrame {
    pub(super) schema_version: String,
    pub(super) principal_id: String,
    pub(super) identity_token: String,
}

/// Registry key for a live stream connection.
///
/// Session principals are keyed by `(bundle_name, session_id)` because their
/// identity is bundle-local. Non-session principals (`@GLOBAL`, `@EXTERNAL`,
/// `@RELAY`) are relay-wide and keyed by `principal_id` alone; a single relay
/// socket serves every bundle, so these connections are not bundle-bound.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) enum RegistryKey {
    Session {
        bundle_name: String,
        session_id: String,
    },
    RelayWide {
        principal_id: String,
    },
}

/// Live registration handle returned to the connection worker.
#[derive(Clone, Debug)]
pub(super) struct StreamRegistration {
    pub(super) key: RegistryKey,
    pub(super) stream_id: String,
}

impl StreamRegistration {
    /// Returns the requester identity to attribute requests to: the bundle-local
    /// `session_id` for session principals, or the full `principal_id` for
    /// relay-wide principals.
    pub(super) fn requester_session_id(&self) -> &str {
        match &self.key {
            RegistryKey::Session { session_id, .. } => session_id.as_str(),
            RegistryKey::RelayWide { principal_id } => principal_id.as_str(),
        }
    }

    /// Returns the bound bundle name for session principals; `None` for
    /// relay-wide principals, which carry no connection bundle binding.
    pub(super) fn bundle_name(&self) -> Option<&str> {
        match &self.key {
            RegistryKey::Session { bundle_name, .. } => Some(bundle_name.as_str()),
            RegistryKey::RelayWide { .. } => None,
        }
    }
}

/// Sender side of a per-connection writer task. Cloned into the stream registry
/// and into delivery paths so events can be dispatched without holding any
/// stream-level mutex. A cloned sender keeps the writer task alive; the task
/// exits when every clone is dropped or when a write attempt fails.
pub(super) type SharedStreamWriter = UnboundedSender<Vec<u8>>;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum IncomingFrame {
    Hello(HelloFrame),
    Request {
        request_id: Option<String>,
        /// Explicit target bundle for this request. When present it selects the
        /// routing bundle regardless of any connection binding; when absent the
        /// connection's bound bundle is used (and its absence is an error).
        bundle_name: Option<String>,
        request: RelayRequest,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "frame", rename_all = "snake_case")]
enum IncomingEnvelope {
    Hello {
        schema_version: String,
        principal_id: String,
        identity_token: String,
    },
    Request {
        #[serde(default)]
        request_id: Option<String>,
        #[serde(default)]
        bundle_name: Option<String>,
        request: RelayRequest,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "frame", rename_all = "snake_case")]
pub(super) enum OutgoingFrame<'a> {
    HelloAck {
        schema_version: &'a str,
        principal_id: &'a str,
    },
    Response {
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<&'a str>,
        response: &'a RelayResponse,
    },
    Event {
        event: &'a RelayStreamEvent,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub(super) struct RelayStreamEvent {
    pub(super) event_type: String,
    pub(super) bundle_name: String,
    pub(super) target_session: String,
    pub(super) created_at: String,
    pub(super) payload: Value,
}

#[derive(Clone, Debug)]
struct RegistryEntry {
    stream_id: Option<String>,
    session_type: SessionType,
    writer: Option<SharedStreamWriter>,
}

#[derive(Default)]
struct StreamRegistry {
    entries: Mutex<HashMap<RegistryKey, RegistryEntry>>,
}

static STREAM_REGISTRY: OnceLock<StreamRegistry> = OnceLock::new();

/// Builds the registry key for a delivery target.
///
/// Non-session principals (`@GLOBAL`/`@EXTERNAL`/`@RELAY`) resolve to a
/// relay-wide key; bare member ids and bundle-local sessions resolve to a
/// `(bundle_name, session_id)` key.
fn registry_key_for_target(bundle_name: &str, session_id: &str) -> RegistryKey {
    match classify_principal_id(session_id) {
        Some(PrincipalType::User | PrincipalType::Application | PrincipalType::Relay) => {
            RegistryKey::RelayWide {
                principal_id: session_id.to_string(),
            }
        }
        _ => RegistryKey::Session {
            bundle_name: bundle_name.to_string(),
            session_id: session_id.to_string(),
        },
    }
}

pub(super) fn parse_incoming_frame(line: &str) -> Result<IncomingFrame, io::Error> {
    let envelope = serde_json::from_str::<IncomingEnvelope>(line).map_err(io::Error::other)?;
    match envelope {
        IncomingEnvelope::Hello {
            schema_version,
            principal_id,
            identity_token,
        } => Ok(IncomingFrame::Hello(HelloFrame {
            schema_version,
            principal_id,
            identity_token,
        })),
        IncomingEnvelope::Request {
            request_id,
            bundle_name,
            request,
        } => Ok(IncomingFrame::Request {
            request_id,
            bundle_name,
            request,
        }),
    }
}

pub(super) fn encode_outgoing_frame(frame: OutgoingFrame<'_>) -> Result<String, io::Error> {
    serde_json::to_string(&frame).map_err(io::Error::other)
}

/// Spawns a per-connection writer task that owns the write half of a relay
/// stream and serially drains framed bytes from an mpsc queue. Returns the
/// sender half so the connection loop, the stream registry, and delivery
/// workers can share write access without holding a per-stream lock across an
/// `.await`.
///
/// The task applies `relay_connection_write_timeout` to every write so a
/// stalled client cannot pin the writer indefinitely. On any write error or
/// timeout the task exits, dropping its receiver; further `send` calls from
/// any cloned sender then return `SendError`, which is the signal callers use
/// to recognize a closed writer.
pub(super) fn spawn_stream_writer(
    mut write_half: OwnedWriteHalf,
) -> (SharedStreamWriter, JoinHandle<()>) {
    let (sender, mut receiver) = mpsc::unbounded_channel::<Vec<u8>>();
    let handle = tokio::spawn(async move {
        while let Some(bytes) = receiver.recv().await {
            let write = async {
                write_half.write_all(&bytes).await?;
                write_half.flush().await
            };
            match timeout(relay_connection_write_timeout(), write).await {
                Ok(Ok(())) => {}
                Ok(Err(source)) => {
                    note_write_timeout(&source);
                    break;
                }
                Err(_elapsed) => {
                    let timeout_error =
                        io::Error::new(io::ErrorKind::TimedOut, "relay connection write timed out");
                    note_write_timeout(&timeout_error);
                    break;
                }
            }
        }
    });
    (sender, handle)
}

/// Resolves the relay-side write timeout for the per-connection writer task.
///
/// A stalled client whose receive buffer is full must not pin a writer task
/// (or, via registered event senders, a delivery worker) indefinitely; this
/// timeout bounds every relay-to-client write. Override with
/// `AGENTMUX_RELAY_CONNECTION_WRITE_TIMEOUT_MS`.
fn relay_connection_write_timeout() -> Duration {
    std::env::var("AGENTMUX_RELAY_CONNECTION_WRITE_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(Duration::from_millis)
        .unwrap_or(RELAY_CONNECTION_WRITE_TIMEOUT)
}

pub(super) fn register_stream(
    key: RegistryKey,
    session_type: SessionType,
    writer: SharedStreamWriter,
) -> Result<RegisterStreamOutcome, io::Error> {
    let registry = stream_registry();
    let mut entries = registry
        .entries
        .lock()
        .map_err(|_| io::Error::other("failed to lock stream registry"))?;
    if let Some(entry) = entries.get(&key)
        && entry.stream_id.is_some()
        && let Some(existing_writer) = entry.writer.as_ref()
        && !existing_writer.is_closed()
    {
        // A registry entry can briefly outlive its connection: when a client
        // drops and immediately reconnects, the owning connection task may not
        // have observed EOF yet, so the stale entry still looks live. Async
        // reads + a drop-guard unregister narrow the race window to scheduling
        // jitter; the conflict surfaces through the standard client retry path
        // (`runtime_identity_claim_conflict`) instead of a synchronous probe.
        return Ok(RegisterStreamOutcome::IdentityClaimConflict {
            existing_connection_id: entry.stream_id.clone(),
        });
    }
    let stream_id = Uuid::new_v4().to_string();
    entries.insert(
        key.clone(),
        RegistryEntry {
            stream_id: Some(stream_id.clone()),
            session_type,
            writer: Some(writer),
        },
    );
    Ok(RegisterStreamOutcome::Registered(StreamRegistration {
        key,
        stream_id,
    }))
}

#[derive(Clone, Debug)]
pub(super) enum RegisterStreamOutcome {
    Registered(StreamRegistration),
    IdentityClaimConflict {
        existing_connection_id: Option<String>,
    },
}

pub(super) fn registration_is_current(
    registration: &StreamRegistration,
) -> Result<bool, io::Error> {
    let registry = stream_registry();
    let entries = registry
        .entries
        .lock()
        .map_err(|_| io::Error::other("failed to lock stream registry"))?;
    Ok(entries
        .get(&registration.key)
        .is_some_and(|entry| entry.stream_id.as_deref() == Some(registration.stream_id.as_str())))
}

pub(super) fn unregister_stream(registration: &StreamRegistration) -> Result<(), io::Error> {
    let registry = stream_registry();
    let mut entries = registry
        .entries
        .lock()
        .map_err(|_| io::Error::other("failed to lock stream registry"))?;
    if let Some(entry) = entries.get_mut(&registration.key)
        && entry
            .stream_id
            .as_deref()
            .is_some_and(|stream_id| stream_id == registration.stream_id.as_str())
    {
        entry.stream_id = None;
        entry.writer = None;
    }
    Ok(())
}

pub(super) fn resolve_registered_session_type(
    bundle_name: &str,
    session_id: &str,
) -> Result<Option<SessionType>, io::Error> {
    let registry = stream_registry();
    let entries = registry
        .entries
        .lock()
        .map_err(|_| io::Error::other("failed to lock stream registry"))?;
    let key = registry_key_for_target(bundle_name, session_id);
    Ok(entries.get(&key).map(|entry| entry.session_type))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StreamEventSendOutcome {
    Delivered,
    NoUiEndpoint,
    Disconnected,
}

// Returns the ids of UI-class subscribers that should receive events for the
// bundle: bundle-local UI sessions plus every relay-wide UI principal (which
// receives events across all bundles). Session ids are returned for
// bundle-local entries and `principal_id`s for relay-wide entries; both forms
// round-trip through `send_event_to_registered_ui`, which re-derives the key.
pub(super) fn list_registered_ui_sessions_for_bundle(bundle_name: &str) -> Vec<String> {
    let registry = stream_registry();
    let Ok(entries) = registry.entries.lock() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|(key, entry)| {
            if entry.session_type != SessionType::Ui {
                return None;
            }
            entry.writer.as_ref()?;
            match key {
                RegistryKey::Session {
                    bundle_name: entry_bundle,
                    session_id,
                } if entry_bundle == bundle_name => Some(session_id.clone()),
                RegistryKey::RelayWide { principal_id } => Some(principal_id.clone()),
                RegistryKey::Session { .. } => None,
            }
        })
        .collect()
}

// Fans an event out to every UI-class subscriber relevant to the bundle
// (bundle-local sessions and relay-wide principals). The per-recipient event is
// cloned with `target_session` rewritten to the recipient id so existing
// per-session filtering still works.
pub(super) fn broadcast_event_to_bundle_ui(
    bundle_name: &str,
    template: &RelayStreamEvent,
) -> Vec<String> {
    let ui_session_ids = list_registered_ui_sessions_for_bundle(bundle_name);
    let mut delivered = Vec::new();
    for ui_session_id in ui_session_ids {
        let mut event = template.clone();
        event.target_session = ui_session_id.clone();
        if matches!(
            send_event_to_registered_ui(bundle_name, ui_session_id.as_str(), &event),
            Ok(StreamEventSendOutcome::Delivered)
        ) {
            delivered.push(ui_session_id);
        }
    }
    delivered
}

pub(super) fn send_event_to_registered_ui(
    bundle_name: &str,
    session_id: &str,
    event: &RelayStreamEvent,
) -> Result<StreamEventSendOutcome, io::Error> {
    let registry = stream_registry();
    let key = registry_key_for_target(bundle_name, session_id);
    let (session_type, writer) = {
        let entries = registry
            .entries
            .lock()
            .map_err(|_| io::Error::other("failed to lock stream registry"))?;
        let Some(entry) = entries.get(&key) else {
            return Ok(StreamEventSendOutcome::NoUiEndpoint);
        };
        (entry.session_type, entry.writer.clone())
    };
    if session_type != SessionType::Ui {
        return Ok(StreamEventSendOutcome::NoUiEndpoint);
    }
    let Some(writer) = writer else {
        return Ok(StreamEventSendOutcome::Disconnected);
    };
    if write_stream_frame_to_writer(&writer, OutgoingFrame::Event { event }).is_ok() {
        return Ok(StreamEventSendOutcome::Delivered);
    }
    let mut entries = registry
        .entries
        .lock()
        .map_err(|_| io::Error::other("failed to lock stream registry"))?;
    if let Some(entry) = entries.get_mut(&key) {
        entry.stream_id = None;
        entry.writer = None;
    }
    Ok(StreamEventSendOutcome::Disconnected)
}

/// Encodes an outgoing frame and hands the bytes to the connection's writer
/// task. Returns an error only when the writer task has already exited (its
/// receiver is dropped); that signals a broken pipe or write timeout observed
/// by the writer, and callers use it to evict the registry entry.
pub(super) fn write_stream_frame_to_writer(
    writer: &SharedStreamWriter,
    frame: OutgoingFrame<'_>,
) -> Result<(), io::Error> {
    let mut bytes = encode_outgoing_frame(frame)?.into_bytes();
    bytes.push(b'\n');
    enqueue_bytes(writer, bytes)
}

fn enqueue_bytes(writer: &SharedStreamWriter, bytes: Vec<u8>) -> Result<(), io::Error> {
    writer.send(bytes).map_err(|SendError(_)| {
        io::Error::new(io::ErrorKind::BrokenPipe, "stream writer task has closed")
    })
}

// Records an inscription when a relay-to-client write failed because the write
// timeout fired. A stalled client (full socket buffer) surfaces here as a
// `WouldBlock` or `TimedOut` error; capturing it makes connection-pool and
// delivery-worker saturation traceable to the offending client.
fn note_write_timeout(error: &io::Error) {
    if matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    ) {
        emit_inscription(
            "relay.connection.write_timeout",
            &serde_json::json!({ "cause": error.to_string() }),
        );
    }
}

fn stream_registry() -> &'static StreamRegistry {
    STREAM_REGISTRY.get_or_init(StreamRegistry::default)
}
