use std::{
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::json;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::{UnixStream, unix::OwnedReadHalf},
    sync::Notify,
    time::error::Elapsed,
};

use crate::{
    configuration::{ConfigurationRoots, SessionType},
    runtime::{inscriptions::emit_inscription, paths::BundleRuntimePaths},
};

use super::drain::ConnectionWorkerSlot;
use super::stream::{
    HelloFrame, IncomingFrame, OutgoingFrame, RegisterStreamOutcome, SharedStreamWriter,
    StreamRegistration, StreamRevokeSignal, parse_incoming_frame, register_stream,
    registration_is_current, spawn_stream_writer, unregister_stream, write_stream_frame_to_writer,
};
use super::{
    IdentityIntrospectRights, PeerConnectionManager, RelayError, RelayRequest, RelayResponse,
    RequestPrincipal, SCHEMA_VERSION, dispatch_discovery, dispatch_identity_admin,
    dispatch_identity_introspect, dispatch_list, dispatch_look, dispatch_raww, dispatch_request,
    dispatch_send, handlers, relay_error,
};

/// Free-fn helpers used by `serve_connection_frames`: namespace routing,
/// principal resolution, and error shaping. Extracted to a sibling module so
/// the state-machine code below can stay focused on stream registration and
/// frame dispatch; imported here so callers continue to invoke the helpers
/// without a `helpers::` prefix.
pub(super) mod helpers;
use self::helpers::*;

use super::catalog::BundleCatalog;

/// Shared, connection-independent context threaded to every connection worker:
/// the config/state roots, the live bundle catalog, the outbound peer connection
/// manager, and the resolved relay-wide serving controls. Grouping these into one
/// value keeps the connection-serving signatures within argument limits (rather
/// than suppressing the lint) and gives each accepted connection a cheap clone of
/// the shared handles.
#[derive(Clone, Debug)]
pub struct ConnectionServeContext {
    configuration_roots: ConfigurationRoots,
    state_root: PathBuf,
    bundle_catalog: BundleCatalog,
    peer_connection_manager: Arc<PeerConnectionManager>,
    /// This relay's configured outbound peer aliases, sorted, read from the
    /// normalized `[[peers]]` configuration at startup. `list.relays` enumerates
    /// them directly rather than dialing or querying the connection manager.
    relay_aliases: Arc<Vec<String>>,
    require_session_credentials: bool,
    pre_hello_idle_timeout: Duration,
    /// Relay-scope mutex serializing identity-admin (`new peer` / `change psk`)
    /// store transactions. Each admin op runs a load/stage/persist/rename
    /// sequence on the blocking pool against the on-disk principal store; without
    /// serialization two concurrent ops could interleave store persists and
    /// credential renames, publishing a credential whose PSK no longer matches
    /// the stored hash or losing an unrelated registration. Shared across every
    /// cloned per-connection context via the `Arc`.
    identity_admin_lock: Arc<Mutex<()>>,
}

impl ConnectionServeContext {
    /// Assembles the shared serving context from resolved relay runtime state; it
    /// is cloned per accepted connection.
    #[must_use]
    pub fn new(
        configuration_roots: ConfigurationRoots,
        state_root: PathBuf,
        bundle_catalog: BundleCatalog,
        peer_connection_manager: Arc<PeerConnectionManager>,
        relay_aliases: Vec<String>,
        require_session_credentials: bool,
        pre_hello_idle_timeout: Duration,
    ) -> Self {
        let mut relay_aliases = relay_aliases;
        relay_aliases.sort();
        relay_aliases.dedup();
        Self {
            configuration_roots,
            state_root,
            bundle_catalog,
            peer_connection_manager,
            relay_aliases: Arc::new(relay_aliases),
            require_session_credentials,
            pre_hello_idle_timeout,
            identity_admin_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Configuration layers the catalog was built against, used by the serve
    /// phase to spawn watchers without holding a `RuntimeRoots`.
    #[must_use]
    pub fn configuration_roots(&self) -> &ConfigurationRoots {
        &self.configuration_roots
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
    let configuration_roots = Arc::new(context.configuration_roots.clone());
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
                    &configuration_roots,
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
                        &configuration_roots,
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
                        let configuration_roots = Arc::clone(&configuration_roots);
                        let state_root = Arc::clone(&state_root);
                        let identity_admin_lock = Arc::clone(&context.identity_admin_lock);
                        dispatch_on_blocking_pool(move || {
                            dispatch_identity_admin(
                                request,
                                &configuration_roots,
                                &state_root,
                                requester_principal_id.as_str(),
                                &identity_admin_lock,
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
                        let configuration_roots = Arc::clone(&configuration_roots);
                        dispatch_on_blocking_pool(move || {
                            dispatch_list(
                                request,
                                &configuration_roots,
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
                        let configuration_roots = Arc::clone(&configuration_roots);
                        let bound_bundle = bound_bundle.clone();
                        let bundle_catalog = bundle_catalog.clone();
                        let peer_connection_manager = Arc::clone(peer_connection_manager);
                        dispatch_on_blocking_pool(move || {
                            dispatch_send(
                                request,
                                &configuration_roots,
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
                        let configuration_roots = Arc::clone(&configuration_roots);
                        let bound_bundle = bound_bundle.clone();
                        let bundle_catalog = bundle_catalog.clone();
                        dispatch_on_blocking_pool(move || {
                            dispatch_look(
                                request,
                                &configuration_roots,
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
                        let configuration_roots = Arc::clone(&configuration_roots);
                        let bound_bundle = bound_bundle.clone();
                        let bundle_catalog = bundle_catalog.clone();
                        let peer_connection_manager = Arc::clone(peer_connection_manager);
                        dispatch_on_blocking_pool(move || {
                            dispatch_raww(
                                request,
                                &configuration_roots,
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
                // Relay-wide discovery (`list.relays`, `list.namespaces`,
                // cross-relay `list.principals`) is not a bundle-subject
                // operation: it reads the configured peer aliases and this relay's
                // own catalog/registry and forwards foreign discovery through the
                // peer connection manager. The requester is its authenticated
                // principal, so it bypasses the bundle-routing path below.
                if matches!(
                    request,
                    RelayRequest::ListRelays
                        | RelayRequest::DiscoverNamespaces { .. }
                        | RelayRequest::DiscoverPrincipals { .. }
                ) {
                    // Discovery authorization resolves the requester's controls
                    // relay-wide, so it needs the full canonical `<id>@<namespace>`
                    // principal id — a bundle session's stored id is bundle-local.
                    let principal = RequestPrincipal {
                        session_id: full_requester_principal_id(active_registration),
                        authenticated_identity: authenticated_identity.clone(),
                        introspect_rights: introspect_rights.clone(),
                        ingress_scope: ingress_scope.clone(),
                    };
                    let response = {
                        let configuration_roots = Arc::clone(&configuration_roots);
                        let bundle_catalog = bundle_catalog.clone();
                        let peer_connection_manager = Arc::clone(peer_connection_manager);
                        let relay_aliases = Arc::clone(&context.relay_aliases);
                        dispatch_on_blocking_pool(move || {
                            dispatch_discovery(
                                request,
                                &configuration_roots,
                                principal,
                                &bundle_catalog,
                                peer_connection_manager.as_ref(),
                                relay_aliases.as_slice(),
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
                    let configuration_roots = Arc::clone(&configuration_roots);
                    let bundle_catalog = bundle_catalog.clone();
                    dispatch_on_blocking_pool(move || {
                        dispatch_request(
                            request,
                            &configuration_roots,
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
