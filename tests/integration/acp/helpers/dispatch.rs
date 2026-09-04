use agentmux::configuration::ConfigurationRoots;
use agentmux::relay::{RelayRequest, RelayResponse, handle_request};
use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};

use super::observation::count_logged_method;
use super::state::{read_worker_state, startup_bundle};
fn dispatch_request(
    request: RelayRequest,
    configuration_roots: &ConfigurationRoots,
    bundle_name: &str,
    tmux_socket: &Path,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    let runtime_directory = tmux_socket.parent().unwrap_or_else(|| Path::new("."));
    handle_request(request, configuration_roots, bundle_name, runtime_directory)
}

fn acp_send_request() -> RelayRequest {
    RelayRequest::Send {
        request_id: Some("req-acp".to_string()),
        requester_session: "alpha".to_string(),
        message: "status?".to_string(),
        targets: vec!["bravo@party".to_string()],
        broadcast: false,
        on_behalf_of: None,
    }
}

/// Starts the bundle, dispatches an async send to `bravo`, then blocks until
/// the persistent ACP worker has acted on the queued task -- its
/// `session/prompt` reached the stub, or the worker settled `unavailable` --
/// so callers can inspect post-delivery side effects deterministically.
pub(in crate::acp) fn dispatch_send(
    config_root: &ConfigurationRoots,
    tmux_socket: &Path,
) -> RelayResponse {
    let root = tmux_socket.parent().unwrap_or_else(|| Path::new("."));
    let log_path = root.join("acp_requests.log");
    let baseline_prompts = count_logged_method(&log_path, "session/prompt");
    let response =
        dispatch_send_result(config_root, tmux_socket).expect("relay request should parse");
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if count_logged_method(&log_path, "session/prompt") > baseline_prompts
            || read_worker_state(root, "bravo").as_deref() == Some("unavailable")
        {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    response
}

/// Starts the bundle and dispatches an async send to `bravo`, returning the
/// immediate relay response without waiting for the worker to act.
pub(in crate::acp) fn dispatch_send_result(
    config_root: &ConfigurationRoots,
    tmux_socket: &Path,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    startup_bundle(config_root, tmux_socket)?;
    dispatch_request(acp_send_request(), config_root, "party", tmux_socket)
}

/// Starts the bundle and dispatches a send whose body is `body_bytes` long.
///
/// The size is the point rather than the content: a body larger than the pipe
/// buffer between the relay and an agent that has stopped reading is what parks
/// the relay's own executor inside its framed write, which is the only state the
/// execution watchdog exists to bound. Well under the 256 KiB payload maximum,
/// so admission accepts it.
pub(in crate::acp) fn dispatch_sized_send_result(
    config_root: &ConfigurationRoots,
    tmux_socket: &Path,
    body_bytes: usize,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    startup_bundle(config_root, tmux_socket)?;
    dispatch_sized_send_without_startup_result(config_root, tmux_socket, body_bytes)
}

/// Dispatches a sized send against an already-started bundle.
pub(in crate::acp) fn dispatch_sized_send_without_startup_result(
    config_root: &ConfigurationRoots,
    tmux_socket: &Path,
    body_bytes: usize,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    let mut request = acp_send_request();
    if let RelayRequest::Send { message, .. } = &mut request {
        *message = "x".repeat(body_bytes);
    }
    dispatch_request(request, config_root, "party", tmux_socket)
}

pub(in crate::acp) fn dispatch_send_without_startup_result(
    config_root: &ConfigurationRoots,
    tmux_socket: &Path,
) -> Result<RelayResponse, agentmux::relay::RelayError> {
    dispatch_request(acp_send_request(), config_root, "party", tmux_socket)
}

fn qualify_party_target(target_session: &str) -> String {
    if target_session.contains('@') {
        target_session.to_string()
    } else {
        format!("{target_session}@party")
    }
}

pub(in crate::acp) fn dispatch_look(
    config_root: &ConfigurationRoots,
    tmux_socket: &Path,
    requester_session: &str,
    target_session: &str,
    lines: Option<usize>,
) -> RelayResponse {
    dispatch_look_with_offset(
        config_root,
        tmux_socket,
        requester_session,
        target_session,
        lines,
        None,
    )
}

pub(in crate::acp) fn dispatch_look_with_offset(
    config_root: &ConfigurationRoots,
    tmux_socket: &Path,
    requester_session: &str,
    target_session: &str,
    lines: Option<usize>,
    offset: Option<usize>,
) -> RelayResponse {
    startup_bundle(config_root, tmux_socket).expect("relay startup should parse");
    dispatch_request(
        RelayRequest::Look {
            requester_session: requester_session.to_string(),
            target_session: qualify_party_target(target_session),
            lines,
            offset,
        },
        config_root,
        "party",
        tmux_socket,
    )
    .expect("relay look should parse")
}

pub(in crate::acp) fn dispatch_look_without_startup(
    config_root: &ConfigurationRoots,
    tmux_socket: &Path,
    requester_session: &str,
    target_session: &str,
    lines: Option<usize>,
) -> RelayResponse {
    dispatch_request(
        RelayRequest::Look {
            requester_session: requester_session.to_string(),
            target_session: qualify_party_target(target_session),
            lines,
            offset: None,
        },
        config_root,
        "party",
        tmux_socket,
    )
    .expect("relay look should parse")
}

pub(in crate::acp) fn dispatch_raww(
    config_root: &ConfigurationRoots,
    tmux_socket: &Path,
    requester_session: &str,
    target_session: &str,
    text: &str,
    no_enter: bool,
) -> RelayResponse {
    startup_bundle(config_root, tmux_socket).expect("relay startup should parse");
    dispatch_request(
        RelayRequest::Raww {
            request_id: Some("req-acp-raww".to_string()),
            requester_session: requester_session.to_string(),
            target_session: qualify_party_target(target_session),
            text: text.to_string(),
            no_enter,
            on_behalf_of: None,
        },
        config_root,
        "party",
        tmux_socket,
    )
    .expect("relay raww should parse")
}
