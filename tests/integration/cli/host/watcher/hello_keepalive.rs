use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use super::super::*;

/// Opens a persistent stream connection, sends one Hello, and returns the live
/// stream, a buffered reader (with a read timeout so a missing frame fails the
/// test rather than hanging), and the first server frame. Held open so a
/// later watcher-driven eviction frame can be observed on the same connection.
pub(super) fn relay_hello_keepalive(
    state_root: &Path,
    principal_id: &str,
    identity_token: &str,
) -> (UnixStream, BufReader<UnixStream>, Value) {
    let socket = agentmux::runtime::paths::RelayRuntimePaths::resolve(state_root).relay_socket;
    let mut stream = UnixStream::connect(&socket).expect("connect relay socket");
    let reader_stream = stream.try_clone().expect("clone relay stream");
    reader_stream
        .set_read_timeout(Some(WATCHER_SIGNAL_WAIT_BUDGET))
        .expect("set relay read timeout");
    let hello = json!({
        "frame": "hello",
        "schema_version": "1",
        "principal_id": principal_id,
        "identity_token": identity_token,
    });
    let encoded = serde_json::to_string(&hello).expect("encode hello");
    stream
        .write_all(format!("{encoded}\n").as_bytes())
        .expect("write hello");
    stream.flush().expect("flush hello");
    let mut reader = BufReader::new(reader_stream);
    let frame = read_next_frame(&mut reader).expect("hello frame");
    (stream, reader, frame)
}

/// Reads the next newline-delimited frame, or `None` on EOF or read timeout.
pub(super) fn read_next_frame(reader: &mut BufReader<UnixStream>) -> Option<Value> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => None,
        Ok(_) => Some(serde_json::from_str(line.trim_end()).expect("decode relay frame")),
        Err(_) => None,
    }
}

/// Repeatedly opens a fresh Hello connection until `accept` matches the returned
/// frame or the deadline passes. Used to observe the watcher's eventual catalog
/// state (a newly loaded bundle accepting Hello, or an unloaded one rejecting).
pub(super) fn poll_hello_first_frame(
    state_root: &Path,
    principal_id: &str,
    identity_token: &str,
    accept: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + WATCHER_SIGNAL_WAIT_BUDGET;
    loop {
        let frame = relay_hello_first_frame(state_root, principal_id, identity_token);
        if accept(&frame) || Instant::now() >= deadline {
            return frame;
        }
        thread::sleep(Duration::from_millis(50));
    }
}
