use std::{collections::HashMap, io, path::Path, sync::Arc, time::Duration};

use serde_json::{Value, json};
use time::OffsetDateTime;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::{UnixStream, unix::OwnedReadHalf},
    time::error::Elapsed,
};

use crate::{
    configuration::{
        SessionType, load_bundle_configuration, load_policy_ids, load_tui_configuration,
    },
    runtime::paths::{BundleRuntimePaths, principal_store_path},
};

use super::identity::{PrincipalStore, PrincipalType, split_principal_id, verify_hello_credential};
use super::stream::{
    HelloFrame, IncomingFrame, OutgoingFrame, RegisterStreamOutcome, RegistryKey,
    SharedStreamWriter, StreamRegistration, parse_incoming_frame, register_stream,
    registration_is_current, spawn_stream_writer, unregister_stream, write_stream_frame_to_writer,
};
use super::{
    RelayError, RelayRequest, RelayResponse, RequestPrincipal, SCHEMA_VERSION,
    canonical_session_id, dispatch_identity_admin, dispatch_request, handlers, map_config,
    map_tui_config, relay_error,
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
    let (writer, writer_handle) = spawn_stream_writer(write_half);
    let reader = BufReader::new(read_half);
    let mut guard = RegistrationGuard::default();
    let outcome = serve_connection_frames(
        reader,
        &writer,
        &mut guard,
        configuration_root,
        state_root,
        bundle_catalog,
        require_session_credentials,
        pre_hello_idle_timeout,
    )
    .await;
    // Drop the local writer clone and unregister synchronously before awaiting
    // the writer task. The registry entry's writer clone is released here so
    // the writer task can observe its receiver close and drain remaining bytes
    // (e.g., a final error response) before exiting. Without this ordering,
    // the writer task would either be cancelled by runtime drop or wait for
    // senders that the caller has not yet released.
    drop(writer);
    drop(guard);
    let _ = writer_handle.await;
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
) -> Result<(), io::Error> {
    let mut bound_bundle: Option<BundleRuntimePaths> = None;
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
                match register_stream(binding.key.clone(), binding.session_type, writer.clone())? {
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
                bound_bundle = binding.bound_bundle;
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
                let bundle_paths = match resolve_effective_bundle(
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
                let session_id = active_registration.requester_session_id().to_string();
                let response = dispatch_request(
                    request,
                    configuration_root,
                    &bundle_paths.bundle_name,
                    &bundle_paths.runtime_directory,
                    Some(RequestPrincipal { session_id }),
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

/// Resolves the routing bundle for a stream request from its namespace selector.
///
/// The `namespace` selects the routing context regardless of any connection
/// binding (so relay-wide principals can address any bundle): a bundle name does
/// a catalog lookup, while `GLOBAL`/`EXTERNAL`/`RELAY` name the relay-wide
/// namespaces. `EXTERNAL` and `RELAY` are reserved for relay-internal routing
/// (extension protocol handling, peer-relay forwarding), so a client selecting
/// them is rejected with `validation_unsupported_namespace` (D7). `GLOBAL` is
/// intended to be client-routable, but the relay-wide delivery path is not yet
/// built — it is deferred to a follow-up slice (with the MCP `namespace`
/// parameter and its integration tests) — so selecting it returns the distinct,
/// temporary `validation_namespace_routing_unavailable` rather than a misleading
/// catalog miss. When no namespace is given, the connection's bound bundle is
/// used; a relay-wide connection with no namespace is rejected.
fn resolve_effective_bundle(
    bundle_catalog: &BundleCatalog,
    namespace: Option<&str>,
    bound_bundle: Option<&BundleRuntimePaths>,
) -> Result<BundleRuntimePaths, RelayError> {
    if let Some(namespace) = namespace {
        return match namespace {
            "GLOBAL" => Err(relay_error(
                "validation_namespace_routing_unavailable",
                "relay-wide GLOBAL routing is not yet available on this relay",
                Some(json!({ "namespace": namespace })),
            )),
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
    match verified.principal_type {
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
            })
        }
        PrincipalType::Application | PrincipalType::Relay => Ok(HelloBinding {
            session_type: SessionType::Pubsub,
            key: RegistryKey::RelayWide {
                principal_id: hello.principal_id.clone(),
            },
            bound_bundle: None,
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
