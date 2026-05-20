use std::{
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

use serde_json::{Value, json};

use crate::{
    configuration::{
        SessionType, load_bundle_configuration, load_policy_ids, load_tui_configuration,
    },
    runtime::paths::BundleRuntimePaths,
};

use super::stream::{
    HelloFrame, IncomingFrame, OutgoingFrame, RegisterStreamOutcome, SharedStreamWriter,
    StreamRegistration, clone_stream_writer, note_write_timeout, parse_incoming_frame,
    register_stream, registration_is_current, unregister_stream, write_stream_frame_to_writer,
};
use super::{
    GLOBAL_SESSION_SUFFIX, RelayError, RelayResponse, RequestPrincipal, SCHEMA_VERSION,
    dispatch_request, handlers, map_config, map_tui_config, relay_error,
};

const RELAY_CONNECTION_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Handles one relay socket request/response exchange on a connected stream.
pub fn serve_connection(
    stream: &mut UnixStream,
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
) -> Result<(), io::Error> {
    stream.set_write_timeout(Some(relay_connection_write_timeout()))?;
    let writer = clone_stream_writer(stream)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut registration = None::<StreamRegistration>;

    let outcome = serve_connection_frames(
        stream,
        writer,
        &mut reader,
        &mut registration,
        configuration_root,
        bundle_paths,
    );

    // Always release the registry entry, including on the error-return paths
    // (write timeout, invalid frame bytes). A leaked entry would force every
    // subsequent reconnect with the same identity into an identity-claim
    // conflict until an event write incidentally cleared it.
    if let Some(registration) = registration.as_ref()
        && let Err(source) = unregister_stream(registration)
        && outcome.is_ok()
    {
        return Err(source);
    }
    outcome
}

/// Resolves the relay-side write timeout for client connections.
///
/// A stalled client whose receive buffer is full must not pin a connection-pool
/// worker (or, via registered event writers, a delivery worker) indefinitely;
/// this timeout bounds every relay-to-client write. Override with
/// `AGENTMUX_RELAY_CONNECTION_WRITE_TIMEOUT_MS`.
fn relay_connection_write_timeout() -> Duration {
    std::env::var("AGENTMUX_RELAY_CONNECTION_WRITE_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(Duration::from_millis)
        .unwrap_or(RELAY_CONNECTION_WRITE_TIMEOUT)
}

fn serve_connection_frames(
    stream: &mut UnixStream,
    writer: SharedStreamWriter,
    reader: &mut BufReader<UnixStream>,
    registration: &mut Option<StreamRegistration>,
    configuration_root: &Path,
    bundle_paths: &BundleRuntimePaths,
) -> Result<(), io::Error> {
    let mut line = String::new();
    loop {
        line.clear();
        let read = match reader.read_line(&mut line) {
            Ok(read) => read,
            Err(source)
                if matches!(
                    source.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) && registration.is_none() =>
            {
                break;
            }
            Err(source) => return Err(source),
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
                write_response(stream, &response)?;
                break;
            }
        };

        match frame {
            IncomingFrame::LegacyRequest(request) => {
                let response = dispatch_request(
                    request,
                    configuration_root,
                    &bundle_paths.bundle_name,
                    &bundle_paths.runtime_directory,
                    None,
                );
                write_response(stream, &response)?;
            }
            IncomingFrame::Hello(hello) => {
                let response = handle_hello_frame(configuration_root, bundle_paths, &hello);
                match response {
                    Ok(session_type) => {
                        stream.set_read_timeout(None)?;
                        match register_stream(&hello, session_type, writer.clone())? {
                            RegisterStreamOutcome::Registered(value) => {
                                *registration = Some(value);
                            }
                            RegisterStreamOutcome::IdentityClaimConflict {
                                existing_connection_id,
                            } => {
                                let error =
                                    identity_claim_conflict_error(&hello, existing_connection_id);
                                write_stream_frame_to_writer(
                                    &writer,
                                    OutgoingFrame::Response {
                                        request_id: None,
                                        response: &RelayResponse::Error { error },
                                    },
                                )?;
                                break;
                            }
                        }
                        write_stream_frame_to_writer(
                            &writer,
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
                                &writer,
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
                            &writer,
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
                let Some(active_registration) = registration.as_ref() else {
                    let error = relay_error(
                        "validation_missing_hello",
                        "stream request requires hello registration",
                        None,
                    );
                    write_stream_frame_to_writer(
                        &writer,
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
                        &writer,
                        OutgoingFrame::Response {
                            request_id: request_id.as_deref(),
                            response: &RelayResponse::Error { error },
                        },
                    )?;
                    break;
                }
                let response = dispatch_request(
                    request,
                    configuration_root,
                    &bundle_paths.bundle_name,
                    &bundle_paths.runtime_directory,
                    Some(RequestPrincipal {
                        session_id: active_registration.session_id.clone(),
                    }),
                );
                write_stream_frame_to_writer(
                    &writer,
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

fn write_response(stream: &mut UnixStream, response: &RelayResponse) -> Result<(), io::Error> {
    let encoded = serde_json::to_string(response).map_err(io::Error::other)?;
    stream
        .write_all(encoded.as_bytes())
        .and_then(|()| stream.write_all(b"\n"))
        .and_then(|()| stream.flush())
        .inspect_err(note_write_timeout)
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
