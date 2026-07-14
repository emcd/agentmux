//! CLI `host relay` integration tests covering the relay subcommand end-to-end
//! through the hosted binary.
//!
//! The cluster files partition the 19 tests by concern:
//! - [`flags`]: CLI argument rejection for positional bundle selectors and
//!   unknown flag combinations (2 tests).
//! - [`startup`]: startup modes (default autostart, no-autostart process-only),
//!   startup-failure records surfaced through `list`, summary folding of
//!   per-session failure reasons, startup-failure clearing on successful
//!   session startup, and per-bundle failure detail inscription (6 tests).
//! - [`credentials`]: `--require-credentials` CLI flag,
//!   `require-session-credentials = true` in `relay.toml`, and the absence
//!   of either (3 tests).
//! - [`watcher`]: bundle-file watcher behavior — load new bundle, load held
//!   non-autostart bundle, unload removed bundle, reload modified bundle,
//!   preserve down-intent across edit, hold non-autostart bundle on edit,
//!   `--no-watch` reconcile-disable, and `watch-bundles = false` reconcile
//!   disable (8 tests).
//!
//! Shared helpers (`relay_hello_first_frame` is used by both [`credentials`]
//! and [`watcher`]) live in this hub. Cluster-specific helpers live with
//! their cluster.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

use agentmux::runtime::paths::RelayRuntimePaths;
use serde_json::{Value, json};

use super::super::support::process;
use super::helpers::*;

mod credentials;
mod flags;
mod startup;
mod watcher;

/// Wait budget for observing a watcher-driven signal: a reload/suppression
/// inscription, an eviction frame on a live stream, or a catalog change probed
/// via Hello. Generous because the pre-commit hook runs the full suite in
/// parallel on arbitrarily loaded machines; every wait returns as soon as the
/// signal arrives, so the budget is only paid on genuine failure.
const WATCHER_SIGNAL_WAIT_BUDGET: Duration = Duration::from_secs(30);

/// Connects a raw client to the live relay socket, sends one Hello, and returns
/// the first server frame (a `hello_ack` on acceptance or an error `response`
/// on rejection). Used to exercise the relay-wide credential-enforcement flag
/// end-to-end through the hosted binary.
fn relay_hello_first_frame(state_root: &Path, principal_id: &str, identity_token: &str) -> Value {
    let socket = RelayRuntimePaths::resolve(state_root).relay_socket;
    let mut stream = UnixStream::connect(&socket).expect("connect relay socket");
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
    let reader_stream = stream.try_clone().expect("clone relay stream");
    reader_stream
        .set_read_timeout(Some(WATCHER_SIGNAL_WAIT_BUDGET))
        .expect("set hello read timeout");
    let mut reader = BufReader::new(reader_stream);
    let mut line = String::new();
    reader.read_line(&mut line).expect("read relay frame");
    serde_json::from_str(line.trim_end()).expect("decode relay frame")
}
