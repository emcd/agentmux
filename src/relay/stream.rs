use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::AsyncWriteExt,
    net::unix::OwnedWriteHalf,
    sync::{
        Notify,
        mpsc::{self, UnboundedSender, error::SendError},
    },
    task::JoinHandle,
    time::timeout,
};
use uuid::Uuid;

use crate::configuration::SessionType;
use crate::runtime::inscriptions::emit_inscription;

use super::identity::{
    PrincipalType, canonical_session_id, classify_principal_id, scope_permits, split_principal_id,
};
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

/// How a unified registry entry came to exist.
///
/// Every entry — bundle coder session, UI session, or relay-wide principal — is
/// keyed by canonical `principal_id`; the source records its lifecycle so
/// eviction can decide whether to keep the entry's static shell after a stream
/// disconnects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RegistrationSource {
    /// Static entry for a configured principal — a bundle session, or a relay-wide
    /// principal declared in `users.toml` — created at startup/reconcile. It
    /// persists across stream (dis)connects (offline is a state, not absence) and
    /// is removed only when its bundle is unloaded/reloaded (bundle sessions) or
    /// the relay restarts.
    Configured,
    /// Dynamic entry created at stream Hello for a principal with no static
    /// declaration (e.g. an application/relay principal, or a dynamically-created
    /// principal). It is removed entirely when its connection drops, since it has
    /// no static configuration to fall back on.
    Stream,
}

/// Live registration handle returned to the connection worker.
#[derive(Clone, Debug)]
pub(super) struct StreamRegistration {
    /// Canonical `principal_id` key of the registry entry.
    pub(super) principal_id: String,
    /// Bound bundle namespace for session principals; `None` for relay-wide
    /// principals, which carry no connection bundle binding.
    bound_namespace: Option<String>,
    /// Identity to attribute requests to: the bundle-local `session_id` for
    /// session principals, or the full `principal_id` for relay-wide principals.
    requester_session: String,
    pub(super) stream_id: String,
}

impl StreamRegistration {
    /// Returns the requester identity to attribute requests to: the bundle-local
    /// `session_id` for session principals, or the full `principal_id` for
    /// relay-wide principals.
    pub(super) fn requester_session_id(&self) -> &str {
        self.requester_session.as_str()
    }

    /// Returns the bound bundle name for session principals; `None` for
    /// relay-wide principals, which carry no connection bundle binding.
    pub(super) fn namespace(&self) -> Option<&str> {
        self.bound_namespace.as_deref()
    }
}

/// Sender side of a per-connection writer task. Cloned into the stream registry
/// and into delivery paths so events can be dispatched without holding any
/// stream-level mutex. A cloned sender keeps the writer task alive; the task
/// exits when every clone is dropped or when a write attempt fails.
pub(super) type SharedStreamWriter = UnboundedSender<Vec<u8>>;

/// Per-connection teardown signal. The connection task races this against its
/// read loop (see `serve_connection`); firing it closes the connection. A clone
/// is held in the stream registry so credential revocation can tear the
/// connection down from another connection's request path.
pub(super) type StreamRevokeSignal = Arc<Notify>;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum IncomingFrame {
    Hello(HelloFrame),
    Request {
        request_id: Option<String>,
        /// Explicit routing namespace for this request: a bundle name, or a
        /// relay-wide specifier (`GLOBAL`/`EXTERNAL`/`RELAY`). When present it
        /// selects the routing context regardless of any connection binding;
        /// when absent the connection's bound bundle is used (and its absence on
        /// a relay-wide connection is an error).
        namespace: Option<String>,
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
        namespace: Option<String>,
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
    pub(super) target_session: String,
    pub(super) created_at: String,
    pub(super) payload: Value,
}

/// One entry in the unified namespace-keyed session registry.
///
/// The registry holds one entry per canonical `principal_id` regardless of
/// whether the principal is a bundle coder session, a UI session, or a relay-wide
/// principal. An entry is a routing/capabilities record: it carries the parsed
/// identity attributes, the transport binding (from which capabilities are
/// derived), and the runtime directory for coder-backed entries, plus the
/// dynamic stream state (writer, revoke signal, verified identity) while a
/// connection is attached. It deliberately stores no delivery-layer readiness
/// state — readiness is owned by the delivery worker registry and resolved at
/// read time.
#[derive(Clone, Debug)]
struct RegistryEntry {
    principal_id: String,
    session_id: String,
    namespace: String,
    principal_class: PrincipalType,
    source: RegistrationSource,
    /// Transport binding. Capability flags (`can_be_looked`, `can_be_written`,
    /// `can_stream_output`, `can_give_choices`) are pure functions of this type
    /// and are derived at check time rather than stored as duplicate bools.
    session_type: SessionType,
    stream_id: Option<String>,
    writer: Option<SharedStreamWriter>,
    /// Verified `principal_id` of the connection, set only for store-backed
    /// credentials; `None` for socket-trust connections. Indexes the registry
    /// for credential revocation, which targets connections holding a rotated
    /// credential — never socket-trust connections, which hold none.
    authenticated_identity: Option<String>,
    /// Teardown signal for the connection, fired to revoke a rotated credential.
    revoke: Option<StreamRevokeSignal>,
    /// Introspection scope of an application principal's connection, recorded so
    /// revocation fan-out can filter hosts by scope; `None` for every other
    /// principal type (which receives no identity events).
    scope: Option<String>,
}

impl RegistryEntry {
    /// Whether a live stream connection is currently attached.
    fn is_connected(&self) -> bool {
        self.stream_id.is_some()
            && self
                .writer
                .as_ref()
                .is_some_and(|writer| !writer.is_closed())
    }

    /// Clears the dynamic stream state, leaving the static routing/capability
    /// shell intact for a bundle-runtime entry to be reattached on reconnect.
    fn clear_dynamic_state(&mut self) {
        self.stream_id = None;
        self.writer = None;
        self.revoke = None;
        self.authenticated_identity = None;
        self.scope = None;
    }
}

#[derive(Default)]
struct StreamRegistry {
    entries: Mutex<HashMap<String, RegistryEntry>>,
}

static STREAM_REGISTRY: OnceLock<StreamRegistry> = OnceLock::new();

/// Parses a qualified `principal_id` into `(session_id, namespace, class)`.
/// Returns `None` for an unqualified id, which the unified registry rejects:
/// every live entry is keyed by a canonical, fully-qualified principal id.
fn parse_principal_parts(principal_id: &str) -> Option<(String, String, PrincipalType)> {
    let class = classify_principal_id(principal_id)?;
    let (session_id, namespace) = split_principal_id(principal_id)?;
    Some((session_id.to_string(), namespace.to_string(), class))
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
            namespace,
            request,
        } => Ok(IncomingFrame::Request {
            request_id,
            namespace,
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

/// Registers (or refreshes) a static entry for a configured principal — a bundle
/// session or a `users.toml`-declared relay-wide principal — keyed by canonical
/// `principal_id`.
///
/// Called during startup/reconcile so a configured target has a registry entry
/// before any client connects: look/raww resolve its capabilities from the entry,
/// an offline-but-declared principal is a known target (not an unknown one), and
/// a not-yet-ready coder target is not mistaken for an unknown one. The call is
/// idempotent — a reconcile refreshes the static attributes while preserving any
/// attached dynamic stream state. The `principal_id` must be fully qualified.
pub(super) fn register_configured_session(
    principal_id: &str,
    session_type: SessionType,
) -> Result<(), io::Error> {
    let (session_id, namespace, principal_class) = parse_principal_parts(principal_id)
        .ok_or_else(|| io::Error::other("configured principal_id is not qualified"))?;
    let registry = stream_registry();
    let mut entries = registry
        .entries
        .lock()
        .map_err(|_| io::Error::other("failed to lock stream registry"))?;
    match entries.get_mut(principal_id) {
        Some(entry) => {
            entry.source = RegistrationSource::Configured;
            entry.session_type = session_type;
            entry.namespace = namespace;
            entry.session_id = session_id;
            entry.principal_class = principal_class;
        }
        None => {
            entries.insert(
                principal_id.to_string(),
                RegistryEntry {
                    principal_id: principal_id.to_string(),
                    session_id,
                    namespace,
                    principal_class,
                    source: RegistrationSource::Configured,
                    session_type,
                    stream_id: None,
                    writer: None,
                    authenticated_identity: None,
                    revoke: None,
                    scope: None,
                },
            );
        }
    }
    Ok(())
}

/// Attaches dynamic stream state for a connecting principal to the unified
/// registry, keyed by canonical `principal_id`.
///
/// A bundle coder session reuses its static entry (created at startup) and has
/// its writer/revoke/identity attached; a relay-wide or UI principal that has no
/// static entry gets a fresh dynamic entry. A second connection for an already
/// live principal id surfaces the standard identity-claim conflict.
pub(super) fn register_stream(
    principal_id: &str,
    session_type: SessionType,
    writer: SharedStreamWriter,
    authenticated_identity: Option<String>,
    revoke: StreamRevokeSignal,
    scope: Option<String>,
) -> Result<RegisterStreamOutcome, io::Error> {
    let (session_id, namespace, principal_class) = parse_principal_parts(principal_id)
        .ok_or_else(|| io::Error::other("hello principal_id is not a qualified principal"))?;
    let registry = stream_registry();
    let mut entries = registry
        .entries
        .lock()
        .map_err(|_| io::Error::other("failed to lock stream registry"))?;
    // Distinguish a *live* identity-claim conflict from a *stale* one before
    // (re)registering. A registry entry's writer task closes its receiver when it
    // exits -- on a relay-to-client write timeout/error or once every sender
    // clone drops -- so `is_connected()` (an attached `stream_id` whose writer is
    // not closed) is an authoritative live-connection signal:
    //
    //   * Live: the writer is still open, so a genuine second connection is
    //     draining for this principal. Reject with an identity-claim conflict;
    //     the client's bounded retry resolves a true reconnect race once the
    //     prior owner's drop-guard clears the entry.
    //   * Stale: the entry is attached (`stream_id` set) but its writer is
    //     closed, so the prior connection is dead and its drop-guard has not run
    //     yet. Reclaim the entry here rather than forcing the new client through
    //     the conflict-retry path -- the stale takeover no longer depends on
    //     `HELLO_CONFLICT_RETRY_TIMEOUT_MS`.
    //
    // The one case this cannot resolve synchronously is a writer that is still
    // open behind a silently dropped socket (an idle dead connection that has not
    // yet failed a write); it is indistinguishable from a live owner, so it stays
    // a live conflict and is absorbed by the client retry.
    if let Some(entry) = entries.get_mut(principal_id) {
        if entry.is_connected() {
            return Ok(RegisterStreamOutcome::IdentityClaimConflict {
                existing_connection_id: entry.stream_id.clone(),
            });
        }
        if entry.stream_id.is_some() {
            let prior_stream_id = entry.stream_id.clone();
            entry.clear_dynamic_state();
            emit_inscription(
                "relay.stream.stale_claim_reclaimed",
                &serde_json::json!({
                    "principal_id": principal_id,
                    "prior_stream_id": prior_stream_id,
                }),
            );
        }
    }
    let stream_id = Uuid::new_v4().to_string();
    match entries.get_mut(principal_id) {
        Some(entry) => {
            entry.stream_id = Some(stream_id.clone());
            entry.session_type = session_type;
            entry.writer = Some(writer);
            entry.authenticated_identity = authenticated_identity;
            entry.revoke = Some(revoke);
            entry.scope = scope;
        }
        None => {
            entries.insert(
                principal_id.to_string(),
                RegistryEntry {
                    principal_id: principal_id.to_string(),
                    session_id: session_id.clone(),
                    namespace: namespace.clone(),
                    principal_class,
                    source: RegistrationSource::Stream,
                    session_type,
                    stream_id: Some(stream_id.clone()),
                    writer: Some(writer),
                    authenticated_identity,
                    revoke: Some(revoke),
                    scope,
                },
            );
        }
    }
    let bound_namespace = (principal_class == PrincipalType::Session).then(|| namespace.clone());
    let requester_session = if principal_class == PrincipalType::Session {
        session_id
    } else {
        principal_id.to_string()
    };
    Ok(RegisterStreamOutcome::Registered(StreamRegistration {
        principal_id: principal_id.to_string(),
        bound_namespace,
        requester_session,
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
        .get(registration.principal_id.as_str())
        .is_some_and(|entry| entry.stream_id.as_deref() == Some(registration.stream_id.as_str())))
}

/// Detaches the dynamic stream state for a dropped connection. A bundle-runtime
/// entry keeps its static shell so a reconnect reattaches; a purely dynamic
/// (relay-wide / UI) entry is removed entirely.
pub(super) fn unregister_stream(registration: &StreamRegistration) -> Result<(), io::Error> {
    let registry = stream_registry();
    let mut entries = registry
        .entries
        .lock()
        .map_err(|_| io::Error::other("failed to lock stream registry"))?;
    if let Some(entry) = entries.get_mut(registration.principal_id.as_str())
        && entry
            .stream_id
            .as_deref()
            .is_some_and(|stream_id| stream_id == registration.stream_id.as_str())
    {
        if entry.source == RegistrationSource::Stream {
            entries.remove(registration.principal_id.as_str());
        } else {
            entry.clear_dynamic_state();
        }
    }
    Ok(())
}

/// Whether to remove a matched entry entirely during eviction, or only detach
/// its dynamic stream state and keep any static bundle-runtime shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EvictionScope {
    /// Remove only purely dynamic (`Stream`-source) entries; keep bundle-runtime
    /// shells so the configured session can reconnect. Used by credential
    /// revocation.
    DropStreamSourceOnly,
    /// Remove every matched entry, static or dynamic. Used by bundle
    /// unload/reload, where the configuration itself is gone.
    DropEntry,
}

/// Tears down every live stream entry the `selector` matches, writing `response`
/// to each before signalling its read loop to close, then applies `removal` to
/// decide whether the entry is removed or only detached.
///
/// This is the single session-eviction mechanism shared by credential
/// revocation (matched by verified `authenticated_identity`) and bundle
/// unload/reload (matched by namespace). The dynamic stream state is detached
/// before the teardown signal fires so a reconnect is not wedged into an
/// identity-claim conflict against the dying connection. The connection's writer
/// task drains its queue (flushing the `response` frame) before exiting, so the
/// client observes the typed error frame ahead of EOF. Writers and teardown
/// signals are collected under the registry lock and acted on after it is
/// released. Returns the number of connections torn down.
fn evict_streams<F>(selector: F, response: &RelayResponse, removal: EvictionScope) -> usize
where
    F: Fn(&RegistryEntry) -> bool,
{
    let registry = stream_registry();
    let targets = {
        let Ok(mut entries) = registry.entries.lock() else {
            return 0;
        };
        let mut targets = Vec::new();
        let mut to_remove = Vec::new();
        for (key, entry) in entries.iter_mut() {
            if !selector(entry) {
                continue;
            }
            if let (Some(writer), Some(revoke)) = (entry.writer.clone(), entry.revoke.clone()) {
                entry.clear_dynamic_state();
                targets.push((writer, revoke));
            }
            let remove = match removal {
                EvictionScope::DropEntry => true,
                EvictionScope::DropStreamSourceOnly => entry.source == RegistrationSource::Stream,
            };
            if remove {
                to_remove.push(key.clone());
            }
        }
        for key in to_remove {
            entries.remove(&key);
        }
        targets
    };
    let count = targets.len();
    for (writer, revoke) in targets {
        let _ = write_stream_frame_to_writer(
            &writer,
            OutgoingFrame::Response {
                request_id: None,
                response,
            },
        );
        revoke.notify_one();
    }
    count
}

/// Tears down every live stream whose verified `authenticated_identity` matches
/// `principal_id`. Used by `change psk` to revoke a rotated credential: a
/// connection that authenticated with the old credential is force-disconnected.
/// Socket-trust connections carry no `authenticated_identity` and are never
/// matched: they hold no credential to revoke. A bundle-runtime entry keeps its
/// static shell for a reconnect with the new credential; a relay-wide entry is
/// removed. Returns the number torn down.
pub(super) fn revoke_streams_for_identity(principal_id: &str, response: &RelayResponse) -> usize {
    evict_streams(
        |entry| entry.authenticated_identity.as_deref() == Some(principal_id),
        response,
        EvictionScope::DropStreamSourceOnly,
    )
}

/// Evicts every registry entry in `namespace`. Used by the bundle file watcher
/// when a bundle is unloaded (file removed) or reloaded (file modified): each
/// connected session receives the supplied typed error frame
/// (`runtime_bundle_unloaded` / `runtime_bundle_reloaded`) ahead of EOF, and
/// both static and dynamic entries for the namespace are removed so a reload
/// recreates them from the current configuration. Relay-wide principals
/// (`@GLOBAL`/`@EXTERNAL`/`@RELAY`) live in their own reserved namespaces and are
/// never matched. Returns the number of connections torn down.
pub(super) fn evict_streams_for_bundle(namespace: &str, response: &RelayResponse) -> usize {
    evict_streams(
        |entry| entry.namespace == namespace,
        response,
        EvictionScope::DropEntry,
    )
}

/// Fans an `identity.revoked` event out to every live trusted-host (application
/// principal) connection whose registered scope covers `revoked_principal_id`.
///
/// The revoked principal's own session is torn down separately by
/// [`revoke_streams_for_identity`]; this is the notification to *watching*
/// hosts so they can drop any cached view of the revoked identity. Only entries
/// carrying a scope are considered, and `scope` is set only for application
/// principals, so non-host connections are skipped (a `None` scope is
/// fail-closed in [`scope_permits`]). The per-recipient event is cloned with
/// `target_session` rewritten to the recipient host's principal id, matching the
/// relay-wide event convention used by the snapshot. Writers are collected under
/// the registry lock and written after it is released. Returns the number of
/// hosts notified.
pub(super) fn notify_trusted_hosts_of_revocation(
    revoked_principal_id: &str,
    template: &RelayStreamEvent,
) -> usize {
    let registry = stream_registry();
    let targets = {
        let Ok(entries) = registry.entries.lock() else {
            return 0;
        };
        let mut targets = Vec::new();
        for entry in entries.values() {
            let Some(writer) = entry.writer.clone() else {
                continue;
            };
            if !scope_permits(entry.scope.as_deref(), revoked_principal_id) {
                continue;
            }
            if !is_relay_wide(entry.principal_class) {
                continue;
            }
            targets.push((writer, entry.principal_id.clone()));
        }
        targets
    };
    let count = targets.len();
    for (writer, host_principal_id) in targets {
        let mut event = template.clone();
        event.target_session = host_principal_id;
        let _ = write_stream_frame_to_writer(&writer, OutgoingFrame::Event { event: &event });
    }
    count
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StreamEventSendOutcome {
    Delivered,
    NoUiEndpoint,
    Disconnected,
}

/// Whether a principal class is relay-wide (not bundle-bound): a `User`,
/// `Application`, or `Relay` principal in a reserved namespace.
fn is_relay_wide(principal_class: PrincipalType) -> bool {
    matches!(
        principal_class,
        PrincipalType::User | PrincipalType::Application | PrincipalType::Relay
    )
}

/// Reports whether the registry entry for `principal_id` is a relay-wide
/// principal, or `None` when no entry exists yet (e.g. a relay-wide target that
/// has not sent a Hello). The delivery layer uses this to decide stream-vs-coder
/// delivery from the unified registry rather than a carried flag, falling back to
/// namespace classification when the entry is absent.
pub(in crate::relay) fn registry_target_is_relay_wide(principal_id: &str) -> Option<bool> {
    let registry = stream_registry();
    let entries = registry.entries.lock().ok()?;
    entries
        .get(principal_id)
        .map(|entry| is_relay_wide(entry.principal_class))
}

/// Returns the transport binding (`SessionType`) of the registry entry for
/// `principal_id`, or `None` when no entry exists. The target operations use this
/// to resolve a target's advertised capabilities from the unified registry: a
/// registered target gates on its `SessionType` capabilities, an unregistered one
/// is an unknown target.
pub(super) fn lookup_registry_session_type(principal_id: &str) -> Option<SessionType> {
    let registry = stream_registry();
    let entries = registry.entries.lock().ok()?;
    entries.get(principal_id).map(|entry| entry.session_type)
}

// Returns the ids of UI-class subscribers that should receive events for the
// bundle: bundle-local UI sessions plus every relay-wide UI principal (which
// receives events across all bundles). Session ids are returned for
// bundle-local entries and `principal_id`s for relay-wide entries; both forms
// round-trip through `send_event_to_registered_ui`, which re-derives the key.
pub(super) fn list_registered_ui_sessions_for_bundle(namespace: &str) -> Vec<String> {
    let registry = stream_registry();
    let Ok(entries) = registry.entries.lock() else {
        return Vec::new();
    };
    entries
        .values()
        .filter_map(|entry| {
            if entry.session_type != SessionType::Ui {
                return None;
            }
            entry.writer.as_ref()?;
            if is_relay_wide(entry.principal_class) {
                Some(entry.principal_id.clone())
            } else if entry.namespace == namespace {
                Some(entry.session_id.clone())
            } else {
                None
            }
        })
        .collect()
}

// Returns the `(principal_id, session_type, ready)` of every registered principal
// in `namespace`, connected or not. Used by the namespace `list` path (notably
// `GLOBAL`) to enumerate every known principal over the unified registry —
// including declared-but-offline entries — rather than a dedicated relay-wide
// listing. `ready` reflects whether the entry currently holds a live stream
// connection: a relay-wide principal is ready iff online, and a declared-but-
// offline principal is listed with `ready = false` rather than omitted.
pub(super) fn list_namespace_sessions(namespace: &str) -> Vec<(String, SessionType, bool)> {
    let registry = stream_registry();
    let Ok(entries) = registry.entries.lock() else {
        return Vec::new();
    };
    entries
        .values()
        .filter(|entry| entry.namespace == namespace)
        .map(|entry| {
            (
                entry.principal_id.clone(),
                entry.session_type,
                entry.is_connected(),
            )
        })
        .collect()
}

// Fans an event out to every UI-class subscriber relevant to the bundle
// (bundle-local sessions and relay-wide principals). The per-recipient event is
// cloned with `target_session` rewritten to the recipient's canonical id so
// existing per-session filtering still works; bundle-local recipient ids are
// bare and must be qualified, relay-wide principal ids pass through unchanged.
pub(super) fn broadcast_event_to_bundle_ui(
    namespace: &str,
    template: &RelayStreamEvent,
) -> Vec<String> {
    let ui_session_ids = list_registered_ui_sessions_for_bundle(namespace);
    let mut delivered = Vec::new();
    for ui_session_id in ui_session_ids {
        let mut event = template.clone();
        event.target_session = canonical_session_id(ui_session_id.as_str(), namespace);
        if matches!(
            send_event_to_registered_ui(namespace, ui_session_id.as_str(), &event),
            Ok(StreamEventSendOutcome::Delivered)
        ) {
            delivered.push(ui_session_id);
        }
    }
    delivered
}

pub(super) fn send_event_to_registered_ui(
    namespace: &str,
    session_id: &str,
    event: &RelayStreamEvent,
) -> Result<StreamEventSendOutcome, io::Error> {
    let registry = stream_registry();
    let principal_id = canonical_session_id(session_id, namespace);
    let (session_type, writer) = {
        let entries = registry
            .entries
            .lock()
            .map_err(|_| io::Error::other("failed to lock stream registry"))?;
        let Some(entry) = entries.get(principal_id.as_str()) else {
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
    if let Some(entry) = entries.get_mut(principal_id.as_str()) {
        entry.clear_dynamic_state();
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

/// Test-only seam exercising the stale-vs-live identity-claim distinction in
/// [`register_stream`] deterministically, without the connection layer's
/// write-timeout teardown race. Registers a first connection for a unique
/// principal, then either keeps its writer open (a live owner) or closes it (a
/// stale attachment whose writer task has exited), and reports whether a second
/// registration for the same principal is rejected as a live identity-claim
/// conflict (`true`) or reclaims the stale entry and registers (`false`). The
/// registry entry is removed before returning so no global state leaks into
/// other tests.
#[doc(hidden)]
#[must_use]
pub fn second_claim_is_live_conflict_for_testing(prior_writer_open: bool) -> bool {
    let principal_id = format!("staleprobe{}@GLOBAL", Uuid::new_v4().simple());
    let revoke: StreamRevokeSignal = Arc::new(Notify::new());

    let (first_writer, first_receiver) = mpsc::unbounded_channel::<Vec<u8>>();
    let _ = register_stream(
        principal_id.as_str(),
        SessionType::Ui,
        first_writer,
        None,
        revoke.clone(),
        None,
    );
    if !prior_writer_open {
        // Drop the receiver so the prior writer reports closed: the entry is now
        // a stale attachment to a dead connection.
        drop(first_receiver);
    }

    // The second writer's receiver is held (`_second_receiver`) only so the
    // sender stays open through registration; the live/stale decision keys on the
    // *prior* writer, whose receiver is held to end of scope in the open case.
    let (second_writer, _second_receiver) = mpsc::unbounded_channel::<Vec<u8>>();
    let outcome = register_stream(
        principal_id.as_str(),
        SessionType::Ui,
        second_writer,
        None,
        revoke,
        None,
    );

    if let Ok(mut entries) = stream_registry().entries.lock() {
        entries.remove(principal_id.as_str());
    }

    matches!(
        outcome,
        Ok(RegisterStreamOutcome::IdentityClaimConflict { .. })
    )
}
