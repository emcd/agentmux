use std::{collections::HashMap, io, path::Path, sync::Arc, time::Duration};

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

use super::identity::{
    IdentityIntrospectRights, PrincipalStore, PrincipalType, VerifiedIdentity,
    classify_principal_id, split_principal_id, verify_hello_credential,
};
use super::stream::{
    HelloFrame, IncomingFrame, OutgoingFrame, RegisterStreamOutcome, RegistryKey,
    SharedStreamWriter, StreamRegistration, StreamRevokeSignal, parse_incoming_frame,
    register_stream, registration_is_current, spawn_stream_writer, unregister_stream,
    write_stream_frame_to_writer,
};
use super::{
    RelayError, RelayRequest, RelayResponse, RequestPrincipal, SCHEMA_VERSION,
    canonical_session_id, dispatch_identity_admin, dispatch_identity_introspect, dispatch_request,
    handlers, map_config, map_tui_config, relay_error,
};

/// Map from configured bundle name to its resolved runtime paths. Shared
/// across all connection workers so each accepted connection can look up
/// its bundle from the Hello frame.
pub type BundleCatalog = Arc<HashMap<String, BundleRuntimePaths>>;

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
pub async fn serve_connection(
    stream: UnixStream,
    configuration_root: &Path,
    state_root: &Path,
    bundle_catalog: &BundleCatalog,
    require_session_credentials: bool,
    pre_hello_idle_timeout: Duration,
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
            configuration_root,
            state_root,
            bundle_catalog,
            require_session_credentials,
            pre_hello_idle_timeout,
            revoke.clone(),
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
    key: RegistryKey,
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
}

#[allow(clippy::too_many_arguments)]
async fn serve_connection_frames(
    mut reader: BufReader<OwnedReadHalf>,
    writer: &SharedStreamWriter,
    guard: &mut RegistrationGuard,
    configuration_root: &Path,
    state_root: &Path,
    bundle_catalog: &BundleCatalog,
    require_session_credentials: bool,
    pre_hello_idle_timeout: Duration,
    revoke: StreamRevokeSignal,
) -> Result<(), io::Error> {
    let mut bound_bundle: Option<BundleRuntimePaths> = None;
    // Verified principal_id of the connection, set on a store-backed Hello and
    // attached to each dispatched request for sender attribution; stays `None`
    // for socket-trust connections.
    let mut authenticated_identity: Option<String> = None;
    // Introspection rights for an application principal, recorded at Hello and
    // attached to each dispatched request so dispatch can gate
    // `IdentityIntrospect` (task 2.5); stays `None` for every other connection.
    let mut introspect_rights: Option<IdentityIntrospectRights> = None;
    let mut line = String::new();
    loop {
        line.clear();
        let read = match read_next_line(
            &mut reader,
            &mut line,
            guard.current().is_some(),
            pre_hello_idle_timeout,
        )
        .await
        {
            ReadLineOutcome::Read(read) => read,
            ReadLineOutcome::Eof => break,
            ReadLineOutcome::PreHelloIdleTimeout => break,
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

        match frame {
            IncomingFrame::Hello(hello) => {
                let binding = match resolve_hello_binding(
                    configuration_root,
                    state_root,
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
                    binding.key.clone(),
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
                    && let Err(error) = emit_registration_permission_snapshots(
                        configuration_root,
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
                bound_bundle = binding.bound_bundle;
                // A trusted-host (application principal) receives an
                // `identity.snapshot` of the active principals within its scope
                // immediately after Hello, so it can seed its view without an
                // initial introspect round-trip. Other principal types carry no
                // introspect rights and get no snapshot.
                if let Some(rights) = introspect_rights.as_ref() {
                    match handlers::build_identity_snapshot_event(
                        state_root,
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
                            "bundle_name": active_registration.bundle_name(),
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
                    let response = dispatch_identity_admin(
                        request,
                        configuration_root,
                        state_root,
                        requester_principal_id.as_str(),
                    );
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
                    };
                    let response = dispatch_identity_introspect(request, state_root, &principal);
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
                let bundle_paths = match resolve_effective_bundle(
                    bundle_catalog,
                    &request,
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
                let session_id = active_registration.requester_session_id().to_string();
                let response = dispatch_request(
                    request,
                    configuration_root,
                    &bundle_paths.bundle_name,
                    &bundle_paths.runtime_directory,
                    Some(RequestPrincipal {
                        session_id,
                        authenticated_identity: authenticated_identity.clone(),
                        introspect_rights: introspect_rights.clone(),
                    }),
                    bundle_catalog,
                );
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

enum ReadLineOutcome {
    Read(usize),
    Eof,
    PreHelloIdleTimeout,
    Error(io::Error),
}

/// Reads the next framed line. Pre-hello reads are bounded by
/// `pre_hello_idle_timeout` so an unresponsive client cannot consume a
/// connection slot indefinitely; post-hello reads block until a frame or EOF
/// arrives.
async fn read_next_line(
    reader: &mut BufReader<OwnedReadHalf>,
    line: &mut String,
    after_hello: bool,
    pre_hello_idle_timeout: Duration,
) -> ReadLineOutcome {
    let read_result = if after_hello {
        reader.read_line(line).await
    } else {
        match tokio::time::timeout(pre_hello_idle_timeout, reader.read_line(line)).await {
            Ok(result) => result,
            Err(Elapsed { .. }) => return ReadLineOutcome::PreHelloIdleTimeout,
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
    match registration.bundle_name() {
        Some(bundle_name) => canonical_session_id(registration.requester_session_id(), bundle_name),
        None => registration.requester_session_id().to_string(),
    }
}

/// Resolves the bundle context to load for a stream request.
///
/// Routing differs by operation:
///
/// - `Send` infers its routing bundle from target principal-id suffixes, not the
///   wire `namespace` field. A session sender always routes to its bound bundle
///   (per-target classification and any cross-namespace conflict are handled
///   downstream in `handle_send`). A relay-wide sender, which carries no bound
///   bundle, routes to the bundle named by the first `@<bundle>` target suffix;
///   a relay-wide sender whose targets are all bare (no suffix) has no routing
///   context and is rejected with `validation_missing_routing_namespace`.
/// - All other operations use the wire `namespace` selector: a bundle name does
///   a catalog lookup, `EXTERNAL`/`RELAY` are reserved for relay-internal
///   routing and rejected with `validation_unsupported_namespace`, and an absent
///   namespace falls back to the connection's bound bundle (a relay-wide
///   connection with no namespace is rejected). `List` with the `GLOBAL`
///   namespace is handled before this function and never reaches it.
fn resolve_effective_bundle(
    bundle_catalog: &BundleCatalog,
    request: &RelayRequest,
    namespace: Option<&str>,
    bound_bundle: Option<&BundleRuntimePaths>,
) -> Result<BundleRuntimePaths, RelayError> {
    if let RelayRequest::Send { targets, .. } = request {
        return resolve_send_routing_bundle(bundle_catalog, targets, bound_bundle);
    }
    resolve_namespace_routing_bundle(bundle_catalog, namespace, bound_bundle)
}

/// Resolves the routing bundle for a `Send` from its target principal-id
/// suffixes (suffix-based routing).
fn resolve_send_routing_bundle(
    bundle_catalog: &BundleCatalog,
    targets: &[String],
    bound_bundle: Option<&BundleRuntimePaths>,
) -> Result<BundleRuntimePaths, RelayError> {
    if let Some(bound) = bound_bundle {
        // A session sender routes within its bound bundle: relay-wide (`@GLOBAL`)
        // targets are delivered via the registry from that context, and any
        // relay-wide/session target mix is rejected in `handle_send`.
        return Ok(bound.clone());
    }
    // A relay-wide sender derives its routing bundle from the first bundle-
    // qualified target. Bare or relay-wide-only targets name no bundle to route
    // against.
    if let Some(bundle_name) = targets
        .iter()
        .find_map(|target| bundle_namespace_suffix(target.as_str()))
    {
        return bundle_catalog
            .get(bundle_name)
            .cloned()
            .ok_or_else(|| unknown_bundle_error(bundle_name));
    }
    Err(relay_error(
        "validation_missing_routing_namespace",
        "send from a relay-wide principal requires at least one bundle-qualified target",
        None,
    ))
}

/// Resolves the routing bundle for namespace-selected operations from the wire
/// `namespace` field and the connection's bound bundle.
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
                .get(bundle_name)
                .cloned()
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

/// Returns the bundle name from a target's `@<bundle>` suffix, or `None` for a
/// bare target (no suffix) or a relay-wide target (`@GLOBAL`/`@EXTERNAL`/
/// `@RELAY`), neither of which names a bundle.
fn bundle_namespace_suffix(target: &str) -> Option<&str> {
    let (_, namespace) = split_principal_id(target)?;
    match classify_principal_id(target) {
        Some(PrincipalType::Session) => Some(namespace),
        _ => None,
    }
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

/// Emits permission snapshots to a freshly registered UI connection.
///
/// Session UI connections receive the snapshot for their bound bundle.
/// Relay-wide UI principals are not bundle-bound, so they replay every
/// configured bundle's snapshot — a global operator sees pending requests
/// across the whole relay on (re)connect.
fn emit_registration_permission_snapshots(
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
    binding: &HelloBinding,
) -> Result<(), RelayError> {
    match &binding.key {
        RegistryKey::Session {
            session_id,
            bundle_name,
        } => {
            if let Some(bundle_paths) = binding.bound_bundle.as_ref() {
                handlers::emit_permission_snapshot_for_ui_registration(
                    configuration_root,
                    bundle_name,
                    &bundle_paths.runtime_directory,
                    session_id,
                )?;
            }
        }
        RegistryKey::RelayWide { principal_id } => {
            for bundle_paths in bundle_catalog.values() {
                handlers::emit_permission_snapshot_for_ui_registration(
                    configuration_root,
                    &bundle_paths.bundle_name,
                    &bundle_paths.runtime_directory,
                    principal_id,
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
    let mut store = PrincipalStore::load(principal_store_path(state_root))?;
    // Prune expired records on access so an expired credential cannot
    // authenticate (in-memory only; the file is rewritten on startup and on
    // store mutations).
    store.prune_expired(OffsetDateTime::now_utc());
    let verified = verify_hello_credential(
        hello.principal_id.as_str(),
        hello.identity_token.as_str(),
        &store,
        require_session_credentials,
    )?;
    let VerifiedIdentity {
        principal_type,
        store_backed,
        introspect_rights,
    } = verified;
    match principal_type {
        PrincipalType::Session => {
            let (session_id, bundle_name) = split_principal_id(hello.principal_id.as_str())
                .ok_or_else(|| {
                    relay_error(
                        "validation_invalid_principal_id",
                        "session principal_id is not in <session>@<bundle> form",
                        Some(json!({ "principal_id": hello.principal_id })),
                    )
                })?;
            let bundle_paths = bundle_catalog
                .get(bundle_name)
                .cloned()
                .ok_or_else(|| unknown_bundle_error(bundle_name))?;
            let session_type =
                resolve_bundle_member_session_type(configuration_root, bundle_name, session_id)?;
            Ok(HelloBinding {
                session_type,
                key: RegistryKey::Session {
                    bundle_name: bundle_name.to_string(),
                    session_id: session_id.to_string(),
                },
                bound_bundle: Some(bundle_paths),
                store_backed,
                introspect_rights,
            })
        }
        PrincipalType::User => {
            let session_type =
                resolve_global_user_session_type(configuration_root, hello.principal_id.as_str())?;
            Ok(HelloBinding {
                session_type,
                key: RegistryKey::RelayWide {
                    principal_id: hello.principal_id.clone(),
                },
                bound_bundle: None,
                store_backed,
                introspect_rights,
            })
        }
        PrincipalType::Application | PrincipalType::Relay => Ok(HelloBinding {
            session_type: SessionType::Pubsub,
            key: RegistryKey::RelayWide {
                principal_id: hello.principal_id.clone(),
            },
            bound_bundle: None,
            store_backed,
            introspect_rights,
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
