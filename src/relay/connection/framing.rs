//! Line-oriented reads off a connection's socket half, and the hand-off that
//! keeps synchronous request handlers off the async runtime's worker threads.
//!
//! Both concerns sit below frame semantics: nothing here knows what a Hello or a
//! Request means, only how the next line arrives and where a dispatcher runs.

use std::{io, time::Duration};

use serde_json::json;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::unix::OwnedReadHalf,
    time::error::Elapsed,
};

use super::super::drain::ConnectionWorkerSlot;
use super::super::{RelayResponse, relay_error};

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
pub(super) async fn dispatch_on_blocking_pool(
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

pub(super) enum ReadLineOutcome {
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
pub(super) async fn read_next_line(
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
