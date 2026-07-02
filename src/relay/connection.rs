use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
    time::Duration,
};

use serde_json::{Value, json};
use time::OffsetDateTime;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::{UnixStream, unix::OwnedReadHalf},
    sync::Notify,
    time::error::Elapsed,
};

use crate::{
    configuration::{
        SessionType, load_bundle_configuration, load_policy_ids, load_tui_configuration,
    },
    runtime::{
        inscriptions::emit_inscription,
        paths::{BundleRuntimePaths, principal_store_path},
    },
};

use super::drain::ConnectionWorkerSlot;
use super::identity::{
    IdentityIntrospectRights, PrincipalStore, PrincipalType, VerifiedIdentity, split_principal_id,
    verify_hello_credential,
};
use super::stream::{
    HelloFrame, IncomingFrame, OutgoingFrame, RegisterStreamOutcome, SharedStreamWriter,
    StreamRegistration, StreamRevokeSignal, parse_incoming_frame, register_stream,
    registration_is_current, spawn_stream_writer, unregister_stream, write_stream_frame_to_writer,
};
use super::{
    PeerConnectionManager, RelayError, RelayRequest, RelayResponse, RequestPrincipal,
    SCHEMA_VERSION, canonical_session_id, dispatch_identity_admin, dispatch_identity_introspect,
    dispatch_list, dispatch_look, dispatch_raww, dispatch_request, dispatch_send, handlers,
    map_config, map_tui_config, relay_error,
};

/// Whether the relay should keep a bundle's sessions running. Seeded from the
/// bundle's effective autostart when the bundle enters the catalog — `Run` when
/// it autostarts, `Hold` otherwise (a per-bundle `autostart = false` or a
/// relay-wide `--no-autostart` both yield `Hold`) — and then toggled by the
/// operator at runtime via `up` (`Run`) and `down` (`Hold`).
///
/// It expresses *intent*, not live status: a `Run` bundle may still have zero
/// ready sessions, and a `Hold` bundle is simply one the relay must not bring up
/// on its own. The watcher only (re)starts a bundle whose intent is `Run`; a
/// `Hold` bundle absorbs configuration edits without being started. The intent
/// is per-process: it lives only as long as the catalog entry and is not
/// persisted across a relay restart (out of scope for the file watcher).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostingIntent {
    Run,
    Hold,
}

/// A loaded bundle's runtime paths together with the host-level state the relay
/// tracks for it across reconciliation. Folding the state into the catalog entry
/// binds it structurally to the bundle's lifetime: removing the entry (an
/// unload) drops the state with it, so there is no parallel collection to keep
/// consistent by hand.
struct CatalogEntry {
    paths: BundleRuntimePaths,
    hosting_intent: HostingIntent,
}

/// Shared, mutable map from configured bundle name to its [`CatalogEntry`]
/// (resolved runtime paths plus host-level state). Cloned by reference (`Arc`)
/// across all connection workers so each accepted connection can look up its
/// bundle from the Hello frame.
///
/// The map is wrapped in an `RwLock` so the bundle file watcher can load,
/// unload, and reload bundles at runtime (the write side) while connection
/// handlers take short-lived read guards (the read side). No accessor holds a
/// guard across an `.await`: each one copies out what it needs and drops the
/// guard before returning, so the `await_holding_lock` lint is never tripped.
#[derive(Clone, Default)]
pub struct BundleCatalog {
    bundles: Arc<RwLock<HashMap<String, CatalogEntry>>>,
}

impl std::fmt::Debug for BundleCatalog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // RwLock contents are never observable without acquiring the lock, and
        // Debug-formatting while the lock is held could deadlock any thread
        // already partway through an inspect/print. Surface the structural
        // placeholder only.
        formatter
            .debug_struct("BundleCatalog")
            .finish_non_exhaustive()
    }
}

impl BundleCatalog {
    /// Builds a catalog from hosted bundle paths, defaulting every entry to
    /// `HostingIntent::Run`. Used where the entries are known to be running (the
    /// per-request ephemeral catalog) or where the intent is irrelevant (tests).
    pub fn from_paths(paths: impl IntoIterator<Item = BundleRuntimePaths>) -> Self {
        Self::from_entries(paths.into_iter().map(|paths| (paths, HostingIntent::Run)))
    }

    /// Builds a catalog from hosted bundle paths each paired with its initial
    /// hosting intent. Used by the relay host at startup to seed `Hold` for the
    /// bundles that do not autostart.
    pub fn from_entries(
        entries: impl IntoIterator<Item = (BundleRuntimePaths, HostingIntent)>,
    ) -> Self {
        let bundles = entries
            .into_iter()
            .map(|(paths, hosting_intent)| {
                (
                    paths.bundle_name.clone(),
                    CatalogEntry {
                        paths,
                        hosting_intent,
                    },
                )
            })
            .collect();
        Self {
            bundles: Arc::new(RwLock::new(bundles)),
        }
    }

    /// Returns the runtime paths for `bundle_name`, or `None` when no such
    /// bundle is currently loaded.
    pub(super) fn lookup(&self, bundle_name: &str) -> Option<BundleRuntimePaths> {
        self.read()
            .get(bundle_name)
            .map(|entry| entry.paths.clone())
    }

    /// Returns a snapshot of every currently loaded bundle's paths. Used by the
    /// relay host to derive its shutdown cleanup list from the live catalog (so
    /// bundles loaded or unloaded at runtime are reflected) and internally to
    /// replay relay-wide UI snapshots across every loaded bundle.
    pub fn snapshot(&self) -> Vec<BundleRuntimePaths> {
        self.read()
            .values()
            .map(|entry| entry.paths.clone())
            .collect()
    }

    /// Returns the set of currently loaded bundle names. Used by the watcher to
    /// diff the loaded set against the on-disk set during reconciliation.
    pub(super) fn loaded_bundle_names(&self) -> HashSet<String> {
        self.read().keys().cloned().collect()
    }

    /// Inserts or replaces a loaded bundle with an explicit hosting intent. Held
    /// by the watcher's write side when a new bundle file is detected (intent
    /// derived from the bundle's effective autostart) or a modified bundle is
    /// reloaded (always `Run` — a held bundle's reload is suppressed before it
    /// reaches here).
    pub(super) fn insert(&self, paths: BundleRuntimePaths, hosting_intent: HostingIntent) {
        let bundle_name = paths.bundle_name.clone();
        self.write().insert(
            bundle_name,
            CatalogEntry {
                paths,
                hosting_intent,
            },
        );
    }

    /// Removes a loaded bundle, returning its paths when present. Held by the
    /// watcher's write side when a bundle file disappears. Dropping the entry
    /// also drops any operator down intent recorded for it.
    pub(super) fn remove(&self, bundle_name: &str) -> Option<BundleRuntimePaths> {
        self.write().remove(bundle_name).map(|entry| entry.paths)
    }

    /// Records the operator's hosting intent on the bundle's catalog entry. Set
    /// to `Hold` by the `down` handler and `Run` by the `up` handler. A no-op
    /// when the bundle is not loaded — intent is meaningful only for a bundle
    /// that exists, and a missing entry carries no state to leak.
    pub(super) fn set_intent(&self, bundle_name: &str, hosting_intent: HostingIntent) {
        if let Some(entry) = self.write().get_mut(bundle_name) {
            entry.hosting_intent = hosting_intent;
        }
    }

    /// Returns whether `bundle_name` is currently held — i.e. the relay must not
    /// start it on its own. `false` when the bundle is not loaded.
    pub(super) fn is_held(&self, bundle_name: &str) -> bool {
        self.read()
            .get(bundle_name)
            .is_some_and(|entry| entry.hosting_intent == HostingIntent::Hold)
    }

    /// Acquires the read guard, recovering from poisoning.
    ///
    /// A poisoned lock means a writer panicked mid-update; the map itself stays
    /// internally consistent, so recovering the guard is preferable to
    /// propagating the panic to every connection handler that looks up a bundle.
    fn read(&self) -> RwLockReadGuard<'_, HashMap<String, CatalogEntry>> {
        self.bundles
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, HashMap<String, CatalogEntry>> {
        self.bundles
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Serves one relay socket connection on the async runtime.
///
/// The stream is split into independent halves: a per-connection writer task
/// owns the write half and serializes outgoing frames; this function consumes
/// the read half here. A `RegistrationGuard` ensures the stream registry entry
/// is released on every exit path (including async cancellation), so a
/// reconnecting client with the same identity cannot be wedged into an
/// identity-claim conflict by a stale entry.
///
/// `state_root` locates the relay-level principal store consulted at Hello time
/// for credential verification; `require_session_credentials` enables relay-wide
/// enforcement (rejecting `"socket-trust"` and unrecognized tokens).
///
/// `worker_slot` is this connection's registration with the host's drain
/// coordinator: the frame loop observes its shutdown signal cooperatively — an
/// idle read exits immediately, an in-flight request finishes and flushes its
/// response first — and dropping it on exit reports the worker as drained.
/// Shared, connection-independent context threaded to every connection worker:
/// the config/state roots, the live bundle catalog, the outbound peer connection
/// manager, and the resolved relay-wide serving controls. Grouping these into one
/// value keeps the connection-serving signatures within argument limits (rather
/// than suppressing the lint) and gives each accepted connection a cheap clone of
/// the shared handles.
#[derive(Clone, Debug)]
pub struct ConnectionServeContext {
    configuration_root: PathBuf,
    state_root: PathBuf,
    bundle_catalog: BundleCatalog,
    peer_connection_manager: Arc<PeerConnectionManager>,
    require_session_credentials: bool,
    pre_hello_idle_timeout: Duration,
}

impl ConnectionServeContext {
    /// Assembles the shared serving context from resolved relay runtime state; it
    /// is cloned per accepted connection.
    #[must_use]
    pub fn new(
        configuration_root: PathBuf,
        state_root: PathBuf,
        bundle_catalog: BundleCatalog,
        peer_connection_manager: Arc<PeerConnectionManager>,
        require_session_credentials: bool,
        pre_hello_idle_timeout: Duration,
    ) -> Self {
        Self {
            configuration_root,
            state_root,
            bundle_catalog,
            peer_connection_manager,
            require_session_credentials,
            pre_hello_idle_timeout,
        }
    }

    /// Configuration root the catalog was built against, used by the serve phase
    /// to spawn per-config-directory watchers without holding a `RuntimeRoots`.
    #[must_use]
    pub fn configuration_root(&self) -> &Path {
        &self.configuration_root
    }

    /// State root the catalog was built against; carried alongside the
    /// configuration root so watcher spawning doesn't depend on outer state.
    #[must_use]
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    /// Live bundle catalog; cloned (cheaply, via the inner `Arc<RwLock<...>>`)
    /// when a connection worker needs to resolve a bundle by name.
    #[must_use]
    pub fn bundle_catalog(&self) -> &BundleCatalog {
        &self.bundle_catalog
    }
}

pub async fn serve_connection(
    stream: UnixStream,
    context: &ConnectionServeContext,
    mut worker_slot: ConnectionWorkerSlot,
) -> Result<(), io::Error> {
    let (read_half, write_half) = stream.into_split();
    let (writer, mut writer_handle) = spawn_stream_writer(write_half);
    let reader = BufReader::new(read_half);
    let mut guard = RegistrationGuard::default();
    // Per-connection teardown signal. Registered alongside the stream so a
    // `change psk` rotation on another connection can revoke this one's
    // credential by firing it; the read loop below races it.
    let revoke = Arc::new(Notify::new());

    // Race the read loop against the writer task. The writer task only exits
    // ahead of the read loop when a relay-to-client write fails or times out
    // (`RELAY_CONNECTION_WRITE_TIMEOUT`) -- for example an idle `SessionType::Ui`
    // stream whose peer has stopped draining the events the relay pushes. In
    // that case the read loop is parked on `read_line` with no incoming frame
    // and no EOF, so without this race it would never observe the dead writer
    // and the connection would linger half-open: write side shut, read loop
    // pinned, registry entry stale. Selecting on the writer handle cancels the
    // read loop deterministically; dropping the guard below then unregisters the
    // stream, so the client sees a clean EOF on both halves and reconnects.
    let outcome = {
        let frames = serve_connection_frames(
            reader,
            &writer,
            &mut guard,
            context,
            revoke.clone(),
            &mut worker_slot,
        );
        tokio::pin!(frames);
        tokio::select! {
            biased;
            result = &mut frames => result,
            _ = &mut writer_handle => {
                emit_inscription("relay.connection.writer_exit_teardown", &json!({}));
                Ok(())
            }
            _ = revoke.notified() => {
                emit_inscription("relay.connection.identity_revoked_teardown", &json!({}));
                Ok(())
            }
        }
    };

    // Release our sender clone and unregister on every exit path. On a normal
    // read-loop exit the writer task may still hold queued bytes (e.g. a final
    // error response written just before the loop broke); awaiting it lets those
    // flush before it exits. If the writer task already exited (the write-failure
    // arm above), it is finished, so skip the await to avoid polling a completed
    // `JoinHandle`.
    drop(writer);
    drop(guard);
    if !writer_handle.is_finished() {
        let _ = writer_handle.await;
    }
    outcome
}

/// Drop-guard owner of a `StreamRegistration` that unregisters on every exit
/// path, including a future cancellation. Without this, an awaited frame loop
/// dropped mid-execution would leak a registry entry and wedge the next
/// same-identity reconnect into an identity-claim conflict.
#[derive(Default)]
struct RegistrationGuard {
    registration: Option<StreamRegistration>,
}

impl RegistrationGuard {
    fn set(&mut self, registration: StreamRegistration) {
        self.registration = Some(registration);
    }

    fn current(&self) -> Option<&StreamRegistration> {
        self.registration.as_ref()
    }
}

impl Drop for RegistrationGuard {
    fn drop(&mut self) {
        if let Some(registration) = self.registration.take() {
            let _ = unregister_stream(&registration);
        }
    }
}

/// Connection-level binding established from a verified Hello identity.
struct HelloBinding {
    session_type: SessionType,
    /// Canonical `principal_id` of the connecting principal; the registry key.
    principal_id: String,
    /// Bound bundle for session principals; `None` for relay-wide principals,
    /// whose requests must carry an explicit target bundle.
    bound_bundle: Option<BundleRuntimePaths>,
    /// True when a store-backed credential verified the identity; false for
    /// accepted socket-trust connections. Distinguishes authenticated senders
    /// from socket-trust ones for sender-attribution responses.
    store_backed: bool,
    /// Introspection rights for an application principal; `None` for every
    /// other principal type. Recorded on the connection context so request
    /// dispatch can gate `IdentityIntrospect` (task 2.5).
    introspect_rights: Option<IdentityIntrospectRights>,
    /// Cross-relay ingress scope for a peer relay (`<id>@RELAY`) principal;
    /// `None` for every other principal type. Recorded on the connection context
    /// so a forwarded `Send`/`Raww` from this peer is gated to its scope.
    ingress_scope: Option<String>,
}

async fn serve_connection_frames(
    mut reader: BufReader<OwnedReadHalf>,
    writer: &SharedStreamWriter,
    guard: &mut RegistrationGuard,
    context: &ConnectionServeContext,
    revoke: StreamRevokeSignal,
    worker_slot: &mut ConnectionWorkerSlot,
) -> Result<(), io::Error> {
    let bundle_catalog = &context.bundle_catalog;
    let peer_connection_manager = &context.peer_connection_manager;
    let require_session_credentials = context.require_session_credentials;
    let pre_hello_idle_timeout = context.pre_hello_idle_timeout;
    // Shared-ownership copies of the root paths so each request dispatch can be
    // moved onto the blocking pool (`'static + Send`) without re-copying the
    // path data per request.
    let configuration_root: Arc<Path> = Arc::from(context.configuration_root.as_path());
    let state_root: Arc<Path> = Arc::from(context.state_root.as_path());
    let mut bound_bundle: Option<BundleRuntimePaths> = None;
    // Verified principal_id of the connection, set on a store-backed Hello and
    // attached to each dispatched request for sender attribution; stays `None`
    // for socket-trust connections.
    let mut authenticated_identity: Option<String> = None;
    // Introspection rights for an application principal, recorded at Hello and
    // attached to each dispatched request so dispatch can gate
    // `IdentityIntrospect` (task 2.5); stays `None` for every other connection.
    let mut introspect_rights: Option<IdentityIntrospectRights> = None;
    // Cross-relay ingress scope for a peer relay (`<id>@RELAY`) connection,
    // recorded at Hello and attached to each dispatched request so a forwarded
    // Send/Raww is gated to the peer's scope; stays `None` for every other
    // connection.
    let mut ingress_scope: Option<String> = None;
    let mut line = String::new();
    loop {
        line.clear();
        // Cooperative drain: between frames, a signaled worker exits before
        // reading further. An in-flight frame is never abandoned — the signal
        // is observed only here and inside the parked read below, so the
        // response for the previous frame has already been handed to the
        // writer task (and flushes during teardown).
        if worker_slot.shutdown_signaled() {
            break;
        }
        let read = match read_next_line(
            &mut reader,
            &mut line,
            guard.current().is_some(),
            pre_hello_idle_timeout,
            worker_slot,
        )
        .await
        {
            ReadLineOutcome::Read(read) => read,
            ReadLineOutcome::Eof => break,
            ReadLineOutcome::PreHelloIdleTimeout => break,
            ReadLineOutcome::ShutdownRequested => break,
            ReadLineOutcome::Error(source) => return Err(source),
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
                write_stream_frame_to_writer(
                    writer,
                    OutgoingFrame::Response {
                        request_id: None,
                        response: &response,
                    },
                )?;
                break;
            }
        };

        // Marks this worker as mid-request until the frame's processing (and
        // response write handoff) completes, so a drain-timeout report can
        // distinguish serving workers from parked ones.
        let _serving = worker_slot.begin_serving();
        match frame {
            IncomingFrame::Hello(hello) => {
                let binding = match resolve_hello_binding(
                    &configuration_root,
                    &state_root,
                    bundle_catalog,
                    require_session_credentials,
                    &hello,
                ) {
                    Ok(binding) => binding,
                    Err(error) => {
                        write_stream_frame_to_writer(
                            writer,
                            OutgoingFrame::Response {
                                request_id: None,
                                response: &RelayResponse::Error { error },
                            },
                        )?;
                        break;
                    }
                };
                // Verified `principal_id` of a store-backed connection; `None`
                // for socket-trust. Recorded on the registry entry so a
                // `change psk` rotation can find and revoke this connection.
                let connection_identity = binding.store_backed.then(|| hello.principal_id.clone());
                // Introspection scope of an application principal, recorded on
                // the registry entry so revocation fan-out can filter watching
                // hosts by scope; `None` for every other principal type.
                let connection_scope = binding
                    .introspect_rights
                    .as_ref()
                    .and_then(|rights| rights.scope.clone());
                match register_stream(
                    binding.principal_id.as_str(),
                    binding.session_type,
                    writer.clone(),
                    connection_identity.clone(),
                    revoke.clone(),
                    connection_scope,
                )? {
                    RegisterStreamOutcome::Registered(value) => {
                        guard.set(value);
                    }
                    RegisterStreamOutcome::IdentityClaimConflict {
                        existing_connection_id,
                    } => {
                        let error = identity_claim_conflict_error(&hello, existing_connection_id);
                        write_stream_frame_to_writer(
                            writer,
                            OutgoingFrame::Response {
                                request_id: None,
                                response: &RelayResponse::Error { error },
                            },
                        )?;
                        break;
                    }
                }
                write_stream_frame_to_writer(
                    writer,
                    OutgoingFrame::HelloAck {
                        schema_version: SCHEMA_VERSION,
                        principal_id: hello.principal_id.as_str(),
                    },
                )?;
                if binding.session_type == SessionType::Ui
                    && let Err(error) = emit_registration_choices_snapshots(
                        &configuration_root,
                        bundle_catalog,
                        &binding,
                    )
                {
                    write_stream_frame_to_writer(
                        writer,
                        OutgoingFrame::Response {
                            request_id: None,
                            response: &RelayResponse::Error { error },
                        },
                    )?;
                    break;
                }
                authenticated_identity = connection_identity;
                introspect_rights = binding.introspect_rights;
                ingress_scope = binding.ingress_scope;
                bound_bundle = binding.bound_bundle;
                // A trusted-host (application principal) receives an
                // `identity.snapshot` of the active principals within its scope
                // immediately after Hello, so it can seed its view without an
                // initial introspect round-trip. Other principal types carry no
                // introspect rights and get no snapshot.
                if let Some(rights) = introspect_rights.as_ref() {
                    match handlers::build_identity_snapshot_event(
                        &state_root,
                        hello.principal_id.as_str(),
                        rights,
                    ) {
                        Ok(event) => write_stream_frame_to_writer(
                            writer,
                            OutgoingFrame::Event { event: &event },
                        )?,
                        Err(error) => {
                            write_stream_frame_to_writer(
                                writer,
                                OutgoingFrame::Response {
                                    request_id: None,
                                    response: &RelayResponse::Error { error },
                                },
                            )?;
                            break;
                        }
                    }
                }
            }
            IncomingFrame::Request {
                request_id,
                namespace: target_namespace,
                request,
            } => {
                let Some(active_registration) = guard.current() else {
                    let error = relay_error(
                        "validation_missing_hello",
                        "stream request requires hello registration",
                        None,
                    );
                    write_stream_frame_to_writer(
                        writer,
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
                            "principal_id": active_registration.requester_session_id(),
                            "namespace": active_registration.namespace(),
                        })),
                    );
                    write_stream_frame_to_writer(
                        writer,
                        OutgoingFrame::Response {
                            request_id: request_id.as_deref(),
                            response: &RelayResponse::Error { error },
                        },
                    )?;
                    break;
                }
                // Relay-wide identity administration bypasses bundle routing:
                // it mutates the relay-level principal store and authorizes the
                // operator against their policy preset relay-wide.
                if matches!(
                    request,
                    RelayRequest::NewPeer { .. } | RelayRequest::ChangePsk { .. }
                ) {
                    let requester_principal_id = full_requester_principal_id(active_registration);
                    let response = {
                        let configuration_root = Arc::clone(&configuration_root);
                        let state_root = Arc::clone(&state_root);
                        dispatch_on_blocking_pool(move || {
                            dispatch_identity_admin(
                                request,
                                &configuration_root,
                                &state_root,
                                requester_principal_id.as_str(),
                            )
                        })
                        .await
                    };
                    write_stream_frame_to_writer(
                        writer,
                        OutgoingFrame::Response {
                            request_id: request_id.as_deref(),
                            response: &response,
                        },
                    )?;
                    continue;
                }
                // Identity introspection is relay-wide: it reads the relay-level
                // principal store and its target may be a bundle-less principal,
                // so it bypasses per-bundle routing. The gate is the connection's
                // recorded `introspect_rights`, carried on the request principal.
                if matches!(request, RelayRequest::IdentityIntrospect { .. }) {
                    let principal = RequestPrincipal {
                        session_id: active_registration.requester_session_id().to_string(),
                        authenticated_identity: authenticated_identity.clone(),
                        introspect_rights: introspect_rights.clone(),
                        ingress_scope: ingress_scope.clone(),
                    };
                    let response = {
                        let state_root = Arc::clone(&state_root);
                        dispatch_on_blocking_pool(move || {
                            dispatch_identity_introspect(request, &state_root, &principal)
                        })
                        .await
                    };
                    write_stream_frame_to_writer(
                        writer,
                        OutgoingFrame::Response {
                            request_id: request_id.as_deref(),
                            response: &response,
                        },
                    )?;
                    continue;
                }
                // `List` with the `GLOBAL` namespace enumerates relay-wide
                // sessions, which have no bundle context; it bypasses the
                // per-bundle dispatch path and reads the stream registry
                // directly. Other per-target operations infer their routing
                // bundle from target suffixes inside `resolve_effective_bundle`.
                if matches!(request, RelayRequest::List { .. })
                    && target_namespace.as_deref() == Some("GLOBAL")
                {
                    let response = handlers::handle_global_list();
                    write_stream_frame_to_writer(
                        writer,
                        OutgoingFrame::Response {
                            request_id: request_id.as_deref(),
                            response: &response,
                        },
                    )?;
                    continue;
                }
                // Every other `List` routes its home (dispatch) bundle separately
                // from the enumerated bundle, so a session can list a peer bundle
                // without being looked up in the wrong bundle's members. The
                // enumerated bundle is the wire `namespace` (or the bound bundle);
                // the dispatch bundle is the requester's bound bundle, or — for a
                // relay-wide principal with no home bundle — the enumerated bundle,
                // where its TUI-config controls resolve (preserving relay-wide list
                // reach).
                if matches!(request, RelayRequest::List { .. }) {
                    let enumerate_paths = match resolve_namespace_routing_bundle(
                        bundle_catalog,
                        target_namespace.as_deref(),
                        bound_bundle.as_ref(),
                    ) {
                        Ok(paths) => paths,
                        Err(error) => {
                            write_stream_frame_to_writer(
                                writer,
                                OutgoingFrame::Response {
                                    request_id: request_id.as_deref(),
                                    response: &RelayResponse::Error { error },
                                },
                            )?;
                            continue;
                        }
                    };
                    let dispatch_paths = bound_bundle
                        .clone()
                        .unwrap_or_else(|| enumerate_paths.clone());
                    let response = {
                        let configuration_root = Arc::clone(&configuration_root);
                        dispatch_on_blocking_pool(move || {
                            dispatch_list(
                                request,
                                &configuration_root,
                                &dispatch_paths,
                                &enumerate_paths,
                            )
                        })
                        .await
                    };
                    write_stream_frame_to_writer(
                        writer,
                        OutgoingFrame::Response {
                            request_id: request_id.as_deref(),
                            response: &response,
                        },
                    )?;
                    continue;
                }
                // `Send`, `Look`, and `Raww` route per-target by namespace and are
                // authorized in the requester's home namespace (its bound bundle,
                // or `GLOBAL`), never a borrowed peer/target bundle — so they
                // bypass the bundle-subject `resolve_namespace_routing_bundle`
                // path below and load each target's bundle inside the handler.
                if matches!(request, RelayRequest::Send { .. }) {
                    let principal = RequestPrincipal {
                        session_id: active_registration.requester_session_id().to_string(),
                        authenticated_identity: authenticated_identity.clone(),
                        introspect_rights: introspect_rights.clone(),
                        ingress_scope: ingress_scope.clone(),
                    };
                    let response = {
                        let configuration_root = Arc::clone(&configuration_root);
                        let bound_bundle = bound_bundle.clone();
                        let bundle_catalog = bundle_catalog.clone();
                        let peer_connection_manager = Arc::clone(peer_connection_manager);
                        dispatch_on_blocking_pool(move || {
                            dispatch_send(
                                request,
                                &configuration_root,
                                bound_bundle.as_ref(),
                                Some(principal),
                                &bundle_catalog,
                                peer_connection_manager.as_ref(),
                            )
                        })
                        .await
                    };
                    write_stream_frame_to_writer(
                        writer,
                        OutgoingFrame::Response {
                            request_id: request_id.as_deref(),
                            response: &response,
                        },
                    )?;
                    continue;
                }
                if matches!(request, RelayRequest::Look { .. }) {
                    let principal = RequestPrincipal {
                        session_id: active_registration.requester_session_id().to_string(),
                        authenticated_identity: authenticated_identity.clone(),
                        introspect_rights: introspect_rights.clone(),
                        ingress_scope: ingress_scope.clone(),
                    };
                    let response = {
                        let configuration_root = Arc::clone(&configuration_root);
                        let bound_bundle = bound_bundle.clone();
                        let bundle_catalog = bundle_catalog.clone();
                        dispatch_on_blocking_pool(move || {
                            dispatch_look(
                                request,
                                &configuration_root,
                                bound_bundle.as_ref(),
                                Some(principal),
                                &bundle_catalog,
                            )
                        })
                        .await
                    };
                    write_stream_frame_to_writer(
                        writer,
                        OutgoingFrame::Response {
                            request_id: request_id.as_deref(),
                            response: &response,
                        },
                    )?;
                    continue;
                }
                if matches!(request, RelayRequest::Raww { .. }) {
                    let principal = RequestPrincipal {
                        session_id: active_registration.requester_session_id().to_string(),
                        authenticated_identity: authenticated_identity.clone(),
                        introspect_rights: introspect_rights.clone(),
                        ingress_scope: ingress_scope.clone(),
                    };
                    let response = {
                        let configuration_root = Arc::clone(&configuration_root);
                        let bound_bundle = bound_bundle.clone();
                        let bundle_catalog = bundle_catalog.clone();
                        let peer_connection_manager = Arc::clone(peer_connection_manager);
                        dispatch_on_blocking_pool(move || {
                            dispatch_raww(
                                request,
                                &configuration_root,
                                bound_bundle.as_ref(),
                                Some(principal),
                                &bundle_catalog,
                                peer_connection_manager.as_ref(),
                            )
                        })
                        .await
                    };
                    write_stream_frame_to_writer(
                        writer,
                        OutgoingFrame::Response {
                            request_id: request_id.as_deref(),
                            response: &response,
                        },
                    )?;
                    continue;
                }
                // Bundle-subject operations (`Up`/`Down`, choice decisions)
                // address a bundle the requester is a member of, by the wire
                // `namespace` selector or the bound bundle. This is not a borrow:
                // the bundle is the operation's subject, not a stand-in home.
                let bundle_paths = match resolve_namespace_routing_bundle(
                    bundle_catalog,
                    target_namespace.as_deref(),
                    bound_bundle.as_ref(),
                ) {
                    Ok(bundle_paths) => bundle_paths,
                    Err(error) => {
                        write_stream_frame_to_writer(
                            writer,
                            OutgoingFrame::Response {
                                request_id: request_id.as_deref(),
                                response: &RelayResponse::Error { error },
                            },
                        )?;
                        continue;
                    }
                };
                let principal = RequestPrincipal {
                    session_id: active_registration.requester_session_id().to_string(),
                    authenticated_identity: authenticated_identity.clone(),
                    introspect_rights: introspect_rights.clone(),
                    ingress_scope: ingress_scope.clone(),
                };
                let response = {
                    let configuration_root = Arc::clone(&configuration_root);
                    let bundle_catalog = bundle_catalog.clone();
                    dispatch_on_blocking_pool(move || {
                        dispatch_request(
                            request,
                            &configuration_root,
                            &bundle_paths.bundle_name,
                            &bundle_paths.runtime_directory,
                            Some(principal),
                            &bundle_catalog,
                        )
                    })
                    .await
                };
                write_stream_frame_to_writer(
                    writer,
                    OutgoingFrame::Response {
                        request_id: request_id.as_deref(),
                        response: &response,
                    },
                )?;
            }
        }
    }

    Ok(())
}

/// Runs one synchronous request dispatcher on tokio's blocking thread pool.
///
/// Request handlers do blocking work inline (config file loads, tmux
/// subprocesses, ACP mutex and replay waits); running them directly on runtime
/// worker threads can park every worker and starve timers, accepts, and the
/// poll-based shutdown path (issues/relay/26). Dispatching through
/// `spawn_blocking` keeps the worker threads free to drive I/O regardless of
/// how long a handler blocks. A join failure (handler panic or runtime
/// shutdown) is mapped to an error response rather than tearing down the
/// connection.
async fn dispatch_on_blocking_pool(
    dispatch: impl FnOnce() -> RelayResponse + Send + 'static,
) -> RelayResponse {
    match tokio::task::spawn_blocking(dispatch).await {
        Ok(response) => response,
        Err(join_error) => RelayResponse::Error {
            error: relay_error(
                "internal_unexpected_failure",
                "relay request dispatch task failed to join",
                Some(json!({"cause": join_error.to_string()})),
            ),
        },
    }
}

enum ReadLineOutcome {
    Read(usize),
    Eof,
    PreHelloIdleTimeout,
    ShutdownRequested,
    Error(io::Error),
}

/// Reads the next framed line, racing the cooperative shutdown signal so a
/// worker parked on a long-lived stream read exits promptly when the host
/// begins draining. Pre-hello reads are additionally bounded by
/// `pre_hello_idle_timeout` so an unresponsive client cannot consume a
/// connection slot indefinitely; post-hello reads block until a frame, EOF, or
/// the shutdown signal arrives.
async fn read_next_line(
    reader: &mut BufReader<OwnedReadHalf>,
    line: &mut String,
    after_hello: bool,
    pre_hello_idle_timeout: Duration,
    worker_slot: &mut ConnectionWorkerSlot,
) -> ReadLineOutcome {
    let read_result = if after_hello {
        tokio::select! {
            biased;
            () = worker_slot.shutdown_signal() => return ReadLineOutcome::ShutdownRequested,
            result = reader.read_line(line) => result,
        }
    } else {
        tokio::select! {
            biased;
            () = worker_slot.shutdown_signal() => return ReadLineOutcome::ShutdownRequested,
            result = tokio::time::timeout(pre_hello_idle_timeout, reader.read_line(line)) => {
                match result {
                    Ok(result) => result,
                    Err(Elapsed { .. }) => return ReadLineOutcome::PreHelloIdleTimeout,
                }
            }
        }
    };
    match read_result {
        Ok(0) => ReadLineOutcome::Eof,
        Ok(read) => ReadLineOutcome::Read(read),
        Err(source) => ReadLineOutcome::Error(source),
    }
}

/// Reconstructs the full `<id>@<namespace>` principal_id of the requester from
/// its stream registration. Session principals are stored bundle-local, so the
/// bound bundle is re-applied; relay-wide principals already carry their full
/// `principal_id`.
fn full_requester_principal_id(registration: &StreamRegistration) -> String {
    match registration.namespace() {
        Some(namespace) => canonical_session_id(registration.requester_session_id(), namespace),
        None => registration.requester_session_id().to_string(),
    }
}

/// Resolves the subject bundle for a bundle-addressed operation (`Up`/`Down`,
/// choice decisions, and the `List` enumerate bundle) from the wire
/// `namespace` field and the connection's bound bundle.
///
/// This is the one remaining pre-handler bundle resolution. The target
/// operations (`Send`/`Look`/`Raww`) no longer use it: they are dispatched
/// through their namespace-centric paths, which resolve the requester in its home
/// namespace and load each target's bundle inside the handler. A bundle name does
/// a catalog lookup, `EXTERNAL`/`RELAY` are reserved for relay-internal routing
/// and rejected with `validation_unsupported_namespace`, and an absent namespace
/// falls back to the connection's bound bundle (a relay-wide connection with no
/// namespace is rejected). `List` with the `GLOBAL` namespace is handled before
/// this function and never reaches it.
fn resolve_namespace_routing_bundle(
    bundle_catalog: &BundleCatalog,
    namespace: Option<&str>,
    bound_bundle: Option<&BundleRuntimePaths>,
) -> Result<BundleRuntimePaths, RelayError> {
    if let Some(namespace) = namespace {
        return match namespace {
            "EXTERNAL" | "RELAY" => Err(relay_error(
                "validation_unsupported_namespace",
                "namespace is reserved for relay-internal routing and cannot be selected by a client",
                Some(json!({ "namespace": namespace })),
            )),
            bundle_name => bundle_catalog
                .lookup(bundle_name)
                .ok_or_else(|| unknown_bundle_error(bundle_name)),
        };
    }
    if let Some(bound) = bound_bundle {
        return Ok(bound.clone());
    }
    Err(relay_error(
        "validation_missing_routing_namespace",
        "stream request from a relay-wide principal requires an explicit routing namespace",
        None,
    ))
}

fn unknown_bundle_error(bundle_name: &str) -> RelayError {
    relay_error(
        "validation_unknown_bundle",
        "request target bundle is not configured on this relay",
        Some(json!({ "bundle_name": bundle_name })),
    )
}

fn identity_claim_conflict_error(
    hello: &HelloFrame,
    existing_connection_id: Option<String>,
) -> RelayError {
    let mut details = serde_json::Map::new();
    details.insert(
        "principal_id".to_string(),
        Value::String(hello.principal_id.clone()),
    );
    details.insert(
        "reason".to_string(),
        Value::String("existing identity owner is still live".to_string()),
    );
    if let Some(value) = existing_connection_id {
        details.insert("existing_connection_id".to_string(), Value::String(value));
    }
    relay_error(
        "runtime_identity_claim_conflict",
        "stream identity is already claimed by a live connection",
        Some(Value::Object(details)),
    )
}

/// Emits choices snapshots to a freshly registered UI connection.
///
/// Session UI connections receive the snapshot for their bound bundle.
/// Relay-wide UI principals are not bundle-bound, so they replay every
/// configured bundle's snapshot — a global operator sees pending requests
/// across the whole relay on (re)connect.
fn emit_registration_choices_snapshots(
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
    binding: &HelloBinding,
) -> Result<(), RelayError> {
    match binding.bound_bundle.as_ref() {
        // Session principal: emit the snapshot for its bound bundle.
        Some(bundle_paths) => {
            if let Some((session_id, namespace)) = split_principal_id(binding.principal_id.as_str())
            {
                handlers::emit_choices_snapshot_for_ui_registration(
                    configuration_root,
                    namespace,
                    &bundle_paths.runtime_directory,
                    session_id,
                )?;
            }
        }
        // Relay-wide principal: not bundle-bound, so replay every configured
        // bundle's snapshot — a global operator sees pending requests across the
        // whole relay on (re)connect.
        None => {
            for bundle_paths in bundle_catalog.snapshot() {
                handlers::emit_choices_snapshot_for_ui_registration(
                    configuration_root,
                    &bundle_paths.bundle_name,
                    &bundle_paths.runtime_directory,
                    binding.principal_id.as_str(),
                )?;
            }
        }
    }
    Ok(())
}

/// Verifies the Hello credential, then resolves the connection binding from the
/// verified principal type.
///
/// Session principals (`@<bundle_name>`) do a bundle-catalog lookup and bind to
/// that bundle; non-session principals (`@GLOBAL`, `@EXTERNAL`, `@RELAY`) skip
/// the catalog and are not bundle-bound.
fn resolve_hello_binding(
    configuration_root: &Path,
    state_root: &Path,
    bundle_catalog: &BundleCatalog,
    require_session_credentials: bool,
    hello: &HelloFrame,
) -> Result<HelloBinding, RelayError> {
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
    let store = PrincipalStore::load(principal_store_path(state_root))?;
    // Expiry is enforced inside `verify_hello_credential` (against `now`) rather
    // than by pruning the store first: an expired-but-recognized credential is
    // rejected with the distinct `runtime_identity_expired` error and its
    // connection is closed, instead of being silently indistinguishable from an
    // unregistered one. The on-disk store is rewritten by startup/mutation
    // pruning, so this read-only path leaves the file untouched.
    let verified = verify_hello_credential(
        hello.principal_id.as_str(),
        hello.identity_token.as_str(),
        &store,
        require_session_credentials,
        OffsetDateTime::now_utc(),
    )?;
    let VerifiedIdentity {
        principal_type,
        store_backed,
        introspect_rights,
        ingress_scope,
    } = verified;
    match principal_type {
        PrincipalType::Session => {
            let (session_id, namespace) = split_principal_id(hello.principal_id.as_str())
                .ok_or_else(|| {
                    relay_error(
                        "validation_invalid_principal_id",
                        "session principal_id is not in <session>@<bundle> form",
                        Some(json!({ "principal_id": hello.principal_id })),
                    )
                })?;
            let bundle_paths = bundle_catalog
                .lookup(namespace)
                .ok_or_else(|| unknown_bundle_error(namespace))?;
            let session_type =
                resolve_bundle_member_session_type(configuration_root, namespace, session_id)?;
            Ok(HelloBinding {
                session_type,
                principal_id: hello.principal_id.clone(),
                bound_bundle: Some(bundle_paths),
                store_backed,
                introspect_rights,
                ingress_scope,
            })
        }
        PrincipalType::User => {
            let session_type =
                resolve_global_user_session_type(configuration_root, hello.principal_id.as_str())?;
            Ok(HelloBinding {
                session_type,
                principal_id: hello.principal_id.clone(),
                bound_bundle: None,
                store_backed,
                introspect_rights,
                ingress_scope,
            })
        }
        PrincipalType::Application | PrincipalType::Relay => Ok(HelloBinding {
            session_type: SessionType::Pubsub,
            principal_id: hello.principal_id.clone(),
            bound_bundle: None,
            store_backed,
            introspect_rights,
            ingress_scope,
        }),
    }
}

/// Resolves the session type for a hello identity matching a bundle member.
fn resolve_bundle_member_session_type(
    configuration_root: &Path,
    bundle_name: &str,
    session_id: &str,
) -> Result<SessionType, RelayError> {
    let bundle = load_bundle_configuration(configuration_root, bundle_name).map_err(map_config)?;
    let Some(member) = bundle.members.iter().find(|member| member.id == session_id) else {
        return Err(relay_error(
            "validation_unknown_sender",
            "hello session_id is not configured in associated bundle",
            Some(json!({
                "bundle_name": bundle.bundle_name,
                "session_id": session_id,
            })),
        ));
    };
    Ok(member.target.session_type())
}

/// Resolves the session type for a `@GLOBAL` user principal by searching
/// `users.toml` global users. Global users are not bundle-bound.
fn resolve_global_user_session_type(
    configuration_root: &Path,
    principal_id: &str,
) -> Result<SessionType, RelayError> {
    let Some(users_configuration) =
        load_tui_configuration(configuration_root).map_err(map_tui_config)?
    else {
        return Err(global_user_missing_error(principal_id));
    };
    let Some(user_session) = users_configuration.session_by_id(principal_id) else {
        return Err(global_user_missing_error(principal_id));
    };
    let policy_ids = load_policy_ids(configuration_root).map_err(map_tui_config)?;
    if !policy_ids.contains(user_session.policy.as_str()) {
        return Err(relay_error(
            "validation_unknown_policy",
            "global user policy references unknown policy id",
            Some(json!({
                "session_id": user_session.id,
                "policy_id": user_session.policy,
            })),
        ));
    }
    Ok(user_session.session_type)
}

fn global_user_missing_error(principal_id: &str) -> RelayError {
    relay_error(
        "validation_unknown_sender",
        "hello principal_id is not configured in global users",
        Some(json!({ "principal_id": principal_id })),
    )
}
