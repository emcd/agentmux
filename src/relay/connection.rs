use std::{io, path::Path, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::{UnixStream, unix::OwnedReadHalf},
    time::error::Elapsed,
};

use crate::{
    configuration::{
        SessionType, load_bundle_configuration, load_policy_ids, load_tui_configuration,
    },
    runtime::paths::BundleRuntimePaths,
};

use super::stream::{
    HelloFrame, IncomingFrame, OutgoingFrame, RegisterStreamOutcome, SharedStreamWriter,
    StreamRegistration, parse_incoming_frame, register_stream, registration_is_current,
    spawn_stream_writer, unregister_stream, write_stream_frame_to_writer,
};
use super::{
    GLOBAL_SESSION_SUFFIX, RelayError, RelayResponse, RequestPrincipal, SCHEMA_VERSION,
    dispatch_request, handlers, map_config, map_tui_config, relay_error,
};

/// Serves one relay socket connection on the async runtime.
///
/// The stream is split into independent halves: a per-connection writer task
/// owns the write half and serializes outgoing frames; this function consumes
/// the read half here. A `RegistrationGuard` ensures the stream registry entry
/// is released on every exit path (including async cancellation), so a
/// reconnecting client with the same identity cannot be wedged into an
/// identity-claim conflict by a stale entry.
pub async fn serve_connection(
    stream: UnixStream,
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
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
        bundle_paths,
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

async fn serve_connection_frames(
    mut reader: BufReader<OwnedReadHalf>,
    writer: &SharedStreamWriter,
    guard: &mut RegistrationGuard,
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    pre_hello_idle_timeout: Duration,
) -> Result<(), io::Error> {
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
                let response = handle_hello_frame(configuration_root, bundle_paths, &hello);
                match response {
                    Ok(session_type) => {
                        match register_stream(&hello, session_type, writer.clone())? {
                            RegisterStreamOutcome::Registered(value) => {
                                guard.set(value);
                            }
                            RegisterStreamOutcome::IdentityClaimConflict {
                                existing_connection_id,
                            } => {
                                let error =
                                    identity_claim_conflict_error(&hello, existing_connection_id);
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
                                bundle_name: hello.bundle_name.as_str(),
                                session_id: hello.session_id.as_str(),
                            },
                        )?;
                        if session_type == SessionType::Ui
                            && let Err(error) =
                                handlers::emit_permission_snapshot_for_ui_registration(
                                    configuration_root,
                                    &bundle_paths.bundle_name,
                                    &bundle_paths.runtime_directory,
                                    hello.session_id.as_str(),
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
                    }
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
            IncomingFrame::Request {
                request_id,
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
                            "bundle_name": active_registration.bundle_name,
                            "session_id": active_registration.session_id,
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
                let session_id = active_registration.session_id.clone();
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

fn identity_claim_conflict_error(
    hello: &HelloFrame,
    existing_connection_id: Option<String>,
) -> RelayError {
    let mut details = serde_json::Map::new();
    details.insert(
        "bundle_name".to_string(),
        Value::String(hello.bundle_name.clone()),
    );
    details.insert(
        "session_id".to_string(),
        Value::String(hello.session_id.clone()),
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

/// Validates a hello frame and resolves the session's configured session type.
///
/// Identity lookup proceeds in order: bundle members for the associated
/// bundle, then global users in `users.toml` when `session_id` carries the
/// `@GLOBAL` suffix.
fn handle_hello_frame(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    hello: &HelloFrame,
) -> Result<SessionType, RelayError> {
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
    if hello.bundle_name != bundle_paths.bundle_name {
        return Err(relay_error(
            "validation_cross_bundle_unsupported",
            "hello bundle_name does not match associated bundle",
            Some(json!({
                "associated_bundle_name": bundle_paths.bundle_name,
                "hello_bundle_name": hello.bundle_name,
            })),
        ));
    }
    if hello.session_id.ends_with(GLOBAL_SESSION_SUFFIX) {
        return resolve_global_user_session_type(configuration_root, bundle_paths, hello);
    }
    resolve_bundle_member_session_type(configuration_root, &bundle_paths.bundle_name, hello)
}

/// Resolves the session type for a hello identity matching a bundle member.
fn resolve_bundle_member_session_type(
    configuration_root: &Path,
    bundle_name: &str,
    hello: &HelloFrame,
) -> Result<SessionType, RelayError> {
    let bundle = load_bundle_configuration(configuration_root, bundle_name).map_err(map_config)?;
    let Some(member) = bundle
        .members
        .iter()
        .find(|member| member.id == hello.session_id)
    else {
        return Err(relay_error(
            "validation_unknown_sender",
            "hello session_id is not configured in associated bundle",
            Some(json!({
                "bundle_name": bundle.bundle_name,
                "session_id": hello.session_id,
            })),
        ));
    };
    Ok(member.target.session_type())
}

/// Resolves the session type for a hello identity carrying the `@GLOBAL`
/// suffix by searching `users.toml` global users.
fn resolve_global_user_session_type(
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
    hello: &HelloFrame,
) -> Result<SessionType, RelayError> {
    let Some(users_configuration) =
        load_tui_configuration(configuration_root).map_err(map_tui_config)?
    else {
        return Err(global_user_missing_error(bundle_paths, hello));
    };
    let Some(user_session) = users_configuration.session_by_id(hello.session_id.as_str()) else {
        return Err(global_user_missing_error(bundle_paths, hello));
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

fn global_user_missing_error(bundle_paths: &BundleRuntimePaths, hello: &HelloFrame) -> RelayError {
    relay_error(
        "validation_unknown_sender",
        "hello session_id is not configured in global users",
        Some(json!({
            "bundle_name": bundle_paths.bundle_name,
            "session_id": hello.session_id,
        })),
    )
}
