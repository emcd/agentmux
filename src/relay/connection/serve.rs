//! One connection's lifetime: registration ownership, the frame loop, and the
//! state a Hello establishes for every frame that follows.
//!
//! The loop itself decides nothing about frame semantics. It reads a line,
//! parses it, hands the frame to [`super::hello`] or [`super::requests`], and
//! acts on the [`FrameOutcome`] it gets back. Both handlers need to end the
//! connection or move to the next frame, which a lifted function cannot express
//! as `break` or `continue`, so that choice is returned rather than performed.

use std::{io, path::Path, sync::Arc};

use serde_json::json;
use tokio::{
    io::BufReader,
    net::{UnixStream, unix::OwnedReadHalf},
    sync::Notify,
};

use crate::{
    configuration::ConfigurationRoots,
    runtime::{inscriptions::emit_inscription, paths::BundleRuntimePaths},
};

use super::super::drain::ConnectionWorkerSlot;
use super::super::stream::{
    IncomingFrame, OutgoingFrame, SharedStreamWriter, StreamRegistration, StreamRevokeSignal,
    parse_incoming_frame, spawn_stream_writer, unregister_stream, write_stream_frame_to_writer,
};
use super::super::{IdentityIntrospectRights, RelayResponse, relay_error};
use super::context::ConnectionServeContext;
use super::framing::{ReadLineOutcome, read_next_line};
use super::{hello, requests};

/// Connection state established by a verified Hello and read by every request
/// frame that follows it on the same connection.
///
/// These four values were locals of the frame loop before the per-frame
/// handlers were lifted out of it. Grouping them names what they already were —
/// the connection's identity, as opposed to the per-frame or per-connection
/// handles — and gives the Hello handler one `&mut` to establish rather than
/// four out-parameters.
#[derive(Default)]
pub(super) struct ConnectionBinding {
    /// Bound bundle for session principals; `None` for relay-wide principals,
    /// whose requests must carry an explicit target bundle.
    pub(super) bound_bundle: Option<BundleRuntimePaths>,
    /// Verified `principal_id` of the connection, set on a store-backed Hello
    /// and attached to each dispatched request for sender attribution; stays
    /// `None` for socket-trust connections.
    pub(super) authenticated_identity: Option<String>,
    /// `principal_id` the connection was admitted under, set on every accepted
    /// Hello whether or not a store credential backed it. Cross-relay forwarding
    /// attributes the origin from this, so a peer learns who a message is from
    /// on a relay that accepts socket-trust.
    ///
    /// Deliberately not a widening of `authenticated_identity`. That field
    /// records whether a credential backed the identity — live-stream revocation
    /// matches on it, and the sender-attribution schema requires it absent for an
    /// unverified session — so the two answer different questions and must stay
    /// separately sourced.
    pub(super) admitted_identity: Option<String>,
    /// Introspection rights for an application principal, recorded at Hello and
    /// attached to each dispatched request so dispatch can gate
    /// `IdentityIntrospect`; stays `None` for every other connection.
    pub(super) introspect_rights: Option<IdentityIntrospectRights>,
    /// Cross-relay ingress scope for a peer relay (`<id>@RELAY`) connection,
    /// recorded at Hello and attached to each dispatched request so a forwarded
    /// Send/Raww is gated to the peer's scope; stays `None` for every other
    /// connection.
    pub(super) ingress_scope: Option<String>,
}

/// What the frame loop does once a handler returns.
///
/// A handler lifted out of the loop cannot `break` or `continue` it, so it says
/// which was intended and the loop performs it.
pub(super) enum FrameOutcome {
    /// Read the next frame on this connection.
    Next,
    /// Stop serving this connection.
    Stop,
}

/// The handles every frame on one connection is served against.
///
/// `configuration_roots` and `state_root` are the shared-ownership copies the
/// loop makes once: each request dispatch moves them onto the blocking pool
/// (`'static + Send`), and re-copying the path data per request is the cost this
/// avoids. Everything else is reachable through `context`.
pub(super) struct FrameContext<'a> {
    pub(super) writer: &'a SharedStreamWriter,
    pub(super) context: &'a ConnectionServeContext,
    pub(super) configuration_roots: &'a Arc<ConfigurationRoots>,
    pub(super) state_root: &'a Arc<Path>,
}

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
///
/// `worker_slot` is this connection's registration with the host's drain
/// coordinator: the frame loop observes its shutdown signal cooperatively — an
/// idle read exits immediately, an in-flight request finishes and flushes its
/// response first — and dropping it on exit reports the worker as drained.
pub async fn serve_connection(
    stream: UnixStream,
    context: &ConnectionServeContext,
    mut worker_slot: ConnectionWorkerSlot,
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
            context,
            revoke.clone(),
            &mut worker_slot,
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
pub(super) struct RegistrationGuard {
    registration: Option<StreamRegistration>,
}

impl RegistrationGuard {
    pub(super) fn set(&mut self, registration: StreamRegistration) {
        self.registration = Some(registration);
    }

    pub(super) fn current(&self) -> Option<&StreamRegistration> {
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
    context: &ConnectionServeContext,
    revoke: StreamRevokeSignal,
    worker_slot: &mut ConnectionWorkerSlot,
) -> Result<(), io::Error> {
    let pre_hello_idle_timeout = context.pre_hello_idle_timeout;
    // Shared-ownership copies of the root paths so each request dispatch can be
    // moved onto the blocking pool (`'static + Send`) without re-copying the
    // path data per request.
    let configuration_roots = Arc::new(context.configuration_roots.clone());
    let state_root: Arc<Path> = Arc::from(context.state_root.as_path());
    let frame_context = FrameContext {
        writer,
        context,
        configuration_roots: &configuration_roots,
        state_root: &state_root,
    };
    let mut binding = ConnectionBinding::default();
    let mut line = String::new();
    loop {
        line.clear();
        // Cooperative drain: between frames, a signaled worker exits before
        // reading further. An in-flight frame is never abandoned — the signal
        // is observed only here and inside the parked read below, so the
        // response for the previous frame has already been handed to the
        // writer task (and flushes during teardown).
        if worker_slot.shutdown_signaled() {
            break;
        }
        let read = match read_next_line(
            &mut reader,
            &mut line,
            guard.current().is_some(),
            pre_hello_idle_timeout,
            worker_slot,
        )
        .await
        {
            ReadLineOutcome::Read(read) => read,
            ReadLineOutcome::Eof => break,
            ReadLineOutcome::PreHelloIdleTimeout => break,
            ReadLineOutcome::ShutdownRequested => break,
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

        // Marks this worker as mid-request until the frame's processing (and
        // response write handoff) completes, so a drain-timeout report can
        // distinguish serving workers from parked ones.
        let _serving = worker_slot.begin_serving();
        let outcome = match frame {
            IncomingFrame::Hello(incoming) => {
                hello::handle_hello(incoming, &frame_context, guard, &mut binding, &revoke).await?
            }
            IncomingFrame::Request {
                request_id,
                namespace: target_namespace,
                request,
            } => {
                requests::handle_request(
                    request_id,
                    target_namespace,
                    request,
                    &frame_context,
                    guard,
                    &binding,
                )
                .await?
            }
        };
        if matches!(outcome, FrameOutcome::Stop) {
            break;
        }
    }

    Ok(())
}
