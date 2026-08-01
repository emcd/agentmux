use std::io::{IsTerminal, Read};

use serde_json::json;

use crate::{
    configuration::load_bundle_configuration,
    relay::{RelayRequest, RelayResponse, request_relay},
    runtime::{
        error::RuntimeError, paths::RelayRuntimePaths,
        starter::ensure_starter_configuration_layout, tui_session::resolve_tui_session_identity,
    },
};

use super::{RuntimeArguments, SendArguments, shared};

pub(super) fn run_agentmux_send(arguments: &[String]) -> Result<(), RuntimeError> {
    if arguments
        .iter()
        .any(|value| value == "--help" || value == "-h")
    {
        print_send_help();
        return Ok(());
    }

    let parsed = parse_send_arguments(arguments)?;
    validate_send_targets(&parsed)?;
    let roots = shared::resolve_roots(&parsed.runtime)?;
    ensure_starter_configuration_layout(&roots)?;
    let resolved_session = resolve_tui_session_identity(
        &roots.configuration_roots,
        parsed.bundle_name.as_deref(),
        parsed.session_selector.as_deref(),
    )?;
    load_bundle_configuration(&roots.configuration_roots, &resolved_session.namespace)
        .map_err(shared::map_bundle_load_error)?;
    let relay_paths = RelayRuntimePaths::resolve(&roots.state_root);
    let response = request_relay(
        &relay_paths.relay_socket,
        &resolved_session.namespace,
        &resolved_session.session_id,
        &RelayRequest::Send {
            request_id: parsed.request_id.clone(),
            requester_session: resolved_session.session_id.clone(),
            message: parsed.message.clone(),
            targets: parsed.targets.clone(),
            broadcast: parsed.broadcast,
            quiet_window_ms: None,
            on_behalf_of: None,
        },
    )
    .map_err(|source| shared::map_relay_request_failure(&relay_paths.relay_socket, source))?;
    let payload = match response {
        RelayResponse::Send {
            schema_version,
            request_id,
            requester_session,
            sender_display_name,
            results,
            ..
        } => json!({
            "schema_version": schema_version,
            "request_id": request_id,
            "requester_session": requester_session,
            "sender_display_name": sender_display_name,
            "results": results,
        }),
        RelayResponse::Error { error } => return Err(shared::map_relay_error(error)),
        other => {
            return Err(RuntimeError::validation(
                "internal_unexpected_failure",
                format!("relay returned unexpected response variant: {other:?}"),
            ));
        }
    };
    if parsed.output_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).map_err(|source| {
                RuntimeError::io("encode send response json", std::io::Error::other(source))
            })?
        );
    } else if let Some(results) = payload["results"].as_array() {
        for result in results {
            let target = result["target_session"].as_str().unwrap_or_default();
            let outcome = result["outcome"].as_str().unwrap_or_default();
            if let Some(reason) = result["reason"].as_str() {
                println!("{target}\t{outcome}\t{reason}");
            } else {
                println!("{target}\t{outcome}");
            }
        }
    }
    Ok(())
}

fn parse_send_arguments(arguments: &[String]) -> Result<SendArguments, RuntimeError> {
    let mut bundle_name = None;
    let mut session_selector = None;
    let mut request_id = None;
    let mut targets = Vec::<String>::new();
    let mut broadcast = false;
    let mut message = None;
    let mut output_json = false;
    let mut runtime = RuntimeArguments::default();
    let mut index = 0usize;

    while index < arguments.len() {
        if shared::parse_runtime_flag(arguments, &mut index, &mut runtime)? {
            index += 1;
            continue;
        }
        match arguments[index].as_str() {
            "--bundle" | "--bundle-name" => {
                bundle_name = Some(shared::take_value(arguments, &mut index, "--bundle")?);
            }
            "--as-session" => {
                session_selector = Some(shared::take_value(arguments, &mut index, "--as-session")?);
            }
            "--request-id" => {
                request_id = Some(shared::take_value(arguments, &mut index, "--request-id")?);
            }
            "--target" => targets.push(shared::take_value(arguments, &mut index, "--target")?),
            "--broadcast" => broadcast = true,
            "--message" => message = Some(shared::take_value(arguments, &mut index, "--message")?),
            "--json" => output_json = true,
            unknown => {
                return Err(RuntimeError::InvalidArgument {
                    argument: unknown.to_string(),
                    message: "unknown argument".to_string(),
                });
            }
        }
        index += 1;
    }

    let message = resolve_send_message(message)?;
    if message.trim().is_empty() {
        return Err(RuntimeError::validation(
            "validation_invalid_arguments",
            "message must be non-empty".to_string(),
        ));
    }
    Ok(SendArguments {
        bundle_name,
        session_selector,
        request_id,
        message,
        targets,
        broadcast,
        output_json,
        runtime,
    })
}

fn resolve_send_message(message_flag: Option<String>) -> Result<String, RuntimeError> {
    let stdin_is_terminal = std::io::stdin().is_terminal();
    if let Some(message) = message_flag {
        if !stdin_is_terminal && stdin_has_message_payload()? {
            return Err(RuntimeError::validation(
                "validation_conflicting_message_input",
                "provide either --message or piped stdin, not both".to_string(),
            ));
        }
        return Ok(message);
    }
    if stdin_is_terminal {
        return Err(RuntimeError::validation(
            "validation_missing_message_input",
            "message input is required via --message or piped stdin".to_string(),
        ));
    }
    let mut buffer = String::new();
    std::io::stdin()
        .read_to_string(&mut buffer)
        .map_err(|source| RuntimeError::io("read send message from stdin", source))?;
    if buffer.trim().is_empty() {
        return Err(RuntimeError::validation(
            "validation_missing_message_input",
            "message input is required via --message or piped stdin".to_string(),
        ));
    }
    Ok(buffer)
}

fn stdin_has_message_payload() -> Result<bool, RuntimeError> {
    let mut buffer = Vec::<u8>::new();
    read_stdin_nonblocking(&mut buffer)?;
    if buffer.is_empty() {
        return Ok(false);
    }
    let payload = String::from_utf8_lossy(&buffer);
    Ok(!payload.trim().is_empty())
}

#[cfg(unix)]
fn read_stdin_nonblocking(buffer: &mut Vec<u8>) -> Result<(), RuntimeError> {
    use std::os::fd::AsRawFd;

    let stdin = std::io::stdin();
    let file_descriptor = stdin.as_raw_fd();
    let original_flags = get_stdin_flags(file_descriptor)?;
    set_stdin_flags(file_descriptor, original_flags | libc::O_NONBLOCK)?;
    let read_result = read_stdin_available_bytes(buffer);
    let restore_result = set_stdin_flags(file_descriptor, original_flags);
    match (read_result, restore_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(_)) => Err(error),
    }
}

#[cfg(unix)]
fn read_stdin_available_bytes(buffer: &mut Vec<u8>) -> Result<(), RuntimeError> {
    let stdin = std::io::stdin();
    let mut handle = stdin.lock();
    let mut chunk = [0u8; 4096];
    loop {
        match handle.read(&mut chunk) {
            Ok(0) => break,
            Ok(read_count) => buffer.extend_from_slice(&chunk[..read_count]),
            Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(source) if source.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(source) => return Err(RuntimeError::io("read send message from stdin", source)),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn get_stdin_flags(file_descriptor: i32) -> Result<i32, RuntimeError> {
    // SAFETY: `fcntl` is called with stdin's live file descriptor and valid
    // command argument.
    let flags = unsafe { libc::fcntl(file_descriptor, libc::F_GETFL) };
    if flags < 0 {
        return Err(RuntimeError::io(
            "get stdin status flags",
            std::io::Error::last_os_error(),
        ));
    }
    Ok(flags)
}

#[cfg(unix)]
fn set_stdin_flags(file_descriptor: i32, flags: i32) -> Result<(), RuntimeError> {
    // SAFETY: `fcntl` is called with stdin's live file descriptor and valid
    // command + flags arguments.
    let result = unsafe { libc::fcntl(file_descriptor, libc::F_SETFL, flags) };
    if result < 0 {
        return Err(RuntimeError::io(
            "set stdin status flags",
            std::io::Error::last_os_error(),
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn read_stdin_nonblocking(_buffer: &mut Vec<u8>) -> Result<(), RuntimeError> {
    Ok(())
}

fn validate_send_targets(arguments: &SendArguments) -> Result<(), RuntimeError> {
    if arguments.broadcast && !arguments.targets.is_empty() {
        return Err(RuntimeError::validation(
            "validation_conflicting_targets",
            "targets must be empty when broadcast=true".to_string(),
        ));
    }
    if !arguments.broadcast && arguments.targets.is_empty() {
        return Err(RuntimeError::validation(
            "validation_empty_targets",
            "provide at least one --target or set --broadcast".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn print_send_help() {
    println!(
        "Usage: agentmux send (--target NAME ... | --broadcast) [--message TEXT] [--request-id ID] [--bundle NAME] [--as-session NAME] [--json] [--configuration-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH]\n\
         \n\
         Send carries no per-call timeout override in v1. The timeout surfaces\n\
         are all per-coder config keys: prime-timeout-ms under\n\
         [coders.<id>.acp], [coders.<id>.tmux], and [coders.<id>.pty], plus\n\
         [coders.<id>.tmux].readiness-timeout-ms, which bounds a tmux\n\
         delivery's entire wait and applies whether or not a prime timeout\n\
         is set."
    );
}
