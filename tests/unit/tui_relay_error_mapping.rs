//! Regression coverage for TUI relay-error code preservation.
//!
//! The TUI must surface the canonical terminal relay codes the `tui-surface`
//! raww taxonomy enumerates — notably `authorization_forbidden` — as stable,
//! machine-readable status codes rather than flattening a relay-enforced
//! permission denial into a generic IO status with no code.
//!
//! These exercise the real public boundary (`Workbench::dispatch_event`): a
//! stub relay answers the recipient-refresh `list` request with a canonical
//! error, and the mapped `RuntimeError` propagates out of the refresh path
//! (`Ctrl+R`). Every TUI relay request path — send, raww, look, choices.pick,
//! and refresh — funnels the relay `Error` variant through the same private
//! `map_relay_error`, so pinning the classification through one path is
//! sufficient; there is no per-path mapping to diverge.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use agentmux::runtime::error::RuntimeError;
use agentmux::tui::{TuiLaunchOptions, workbench::Workbench};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

static SOCKET_COUNTER: AtomicU64 = AtomicU64::new(0);

struct SocketGuard {
    path: PathBuf,
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn temporary_socket_path() -> (SocketGuard, PathBuf) {
    let counter = SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = PathBuf::from("/tmp").join(format!(
        "amx-tui-relay-err-{}-{counter}.sock",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    (SocketGuard { path: path.clone() }, path)
}

fn read_json_line(reader: &mut BufReader<UnixStream>) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).expect("read json line");
    serde_json::from_str(line.trim_end()).expect("decode json line")
}

fn write_json_line(stream: &mut UnixStream, value: &Value) {
    let text = serde_json::to_string(value).expect("encode json line");
    stream
        .write_all(format!("{text}\n").as_bytes())
        .expect("write json line");
    stream.flush().expect("flush json line");
}

/// Stub relay: complete the hello handshake, then answer the first `list`
/// request with a canonical relay error carrying `code`.
fn serve_list_error(listener: UnixListener, code: &'static str) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept client");
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));

        let hello = read_json_line(&mut reader);
        assert_eq!(hello["frame"], "hello");
        write_json_line(
            &mut stream,
            &json!({
                "frame": "hello_ack",
                "schema_version": "1",
                "principal_id": hello["principal_id"],
            }),
        );

        let request = read_json_line(&mut reader);
        assert_eq!(request["frame"], "request");
        assert_eq!(request["request"]["operation"], "list");
        let request_id = request["request_id"]
            .as_str()
            .expect("request id")
            .to_string();
        write_json_line(
            &mut stream,
            &json!({
                "frame": "response",
                "request_id": request_id,
                "response": {
                    "kind": "error",
                    "error": { "code": code, "message": "denied by relay" },
                },
            }),
        );
    })
}

fn workbench_for(socket: PathBuf) -> Workbench {
    Workbench::new(TuiLaunchOptions {
        namespace: "agentmux".to_string(),
        sender_session: "tui".to_string(),
        relay_socket: socket,
        look_lines: None,
        // A single available bundle keeps refresh from fanning out cross-bundle
        // list requests; the error path returns before that fan-out anyway.
        available_bundles: vec!["agentmux".to_string()],
    })
}

fn ctrl_r() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL))
}

#[test]
fn refresh_preserves_authorization_forbidden_code() {
    let (_guard, socket) = temporary_socket_path();
    let listener = UnixListener::bind(&socket).expect("bind stub relay");
    let server = serve_list_error(listener, "authorization_forbidden");

    let mut workbench = workbench_for(socket);
    let error = workbench
        .dispatch_event(ctrl_r())
        .expect_err("a relay authorization denial must surface as an error");

    match error {
        RuntimeError::Validation { code, .. } => {
            assert_eq!(
                code, "authorization_forbidden",
                "relay authorization code must be preserved verbatim",
            );
        }
        other => panic!("expected a preserved machine-readable code, got {other:?}"),
    }

    server.join().expect("join stub relay");
}

#[test]
fn refresh_flattens_noncanonical_code_to_generic_io() {
    let (_guard, socket) = temporary_socket_path();
    let listener = UnixListener::bind(&socket).expect("bind stub relay");
    let server = serve_list_error(listener, "relay_internal_unexpected");

    let mut workbench = workbench_for(socket);
    let error = workbench
        .dispatch_event(ctrl_r())
        .expect_err("a relay error must surface as an error");

    assert!(
        matches!(error, RuntimeError::Io { .. }),
        "a non-canonical relay code must flatten to a generic IO status, got {error:?}",
    );

    server.join().expect("join stub relay");
}
