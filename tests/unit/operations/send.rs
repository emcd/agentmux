//! Direct public-consumer tests for the canonical `send` operation.
//!
//! This file imports only the public runtime and operations APIs
//! (`agentmux::relay`, `agentmux::operations`): no MCP server, no stdio
//! transport, no rmcp router, no agent harness, no MCP association.
//! Compiled only under `experimental-public-send`, matching the gated
//! surface exercised.

use std::sync::Arc;

use agentmux::operations::{
    BundleScope, OperationError, SendInput, SendOutput, SendTool,
};
use agentmux::relay::{
    DispatchWiring, PeerConfiguration, SendOutcome,
    rotate_principal_credential,
};

use super::support::{
    OperationFixture, authorize_global_operator, operation_fixture,
    verified_context,
};

/// Builds the operation bound to the fixture runtime through the
/// mirrored constructor — the same path any embedded host uses.
/// No wiring is rebuilt per call; the struct holds the shared pair.
fn send_tool(fixture: &OperationFixture) -> SendTool {
    let wiring = DispatchWiring::mirrored(&fixture.runtime, &[], true)
        .expect("mirrored wiring installs");
    SendTool::new(Arc::clone(&fixture.runtime), wiring)
}

fn send_input(targets: &[&str], message: &str) -> SendInput {
    SendInput {
        targets: targets.iter().map(ToString::to_string).collect(),
        broadcast: false,
        message: message.to_string(),
        request_id: Some("c-1".to_string()),
        bundle: BundleScope::Home,
    }
}

/// A direct call executes the full dispatch through the public
/// operation handle and returns the typed outcome: request id echoed,
/// sender attributed to the verified principal, per-target results
/// present.
#[tokio::test]
async fn direct_send_returns_typed_outcome() {
    let fixture = operation_fixture("all");
    let tool = send_tool(&fixture);
    let context = verified_context(&fixture.runtime, "alpha@party");
    let output = tool
        .invoke(&context, send_input(&["bravo"], "hi"))
        .await
        .expect("direct send succeeds");
    assert_eq!(output.request_id.as_deref(), Some("c-1"));
    assert_eq!(output.requester_session, "alpha@party");
    assert!(!output.schema_version.is_empty());
    assert_eq!(output.results.len(), 1);
    assert_eq!(output.results[0].target_session, "bravo@party");
}

/// An absent caller correlation id stays absent in the typed output,
/// never a null.
#[tokio::test]
async fn direct_send_echoes_absent_request_id_as_absent() {
    let fixture = operation_fixture("all");
    let tool = send_tool(&fixture);
    let context = verified_context(&fixture.runtime, "alpha@party");
    let mut input = send_input(&["bravo"], "hi");
    input.request_id = None;
    let output = tool
        .invoke(&context, input)
        .await
        .expect("direct send succeeds");
    assert_eq!(output.request_id, None);
}

/// A named bundle parameter qualifies bare targets against it.
#[tokio::test]
async fn direct_send_maps_named_bundle() {
    let fixture = operation_fixture("all");
    let tool = send_tool(&fixture);
    let context = verified_context(&fixture.runtime, "alpha@party");
    let mut input = send_input(&["carol"], "hi");
    input.bundle = BundleScope::Named("other".to_string());
    let output = tool
        .invoke(&context, input)
        .await
        .expect("direct send succeeds");
    assert_eq!(output.results.len(), 1);
    assert_eq!(output.results[0].target_session, "carol@other");
}

/// Broadcast with the canonical home scope fans out in the
/// principal's home bundle, like the MCP bound-bundle default.
#[tokio::test]
async fn direct_send_broadcast_home_scope() {
    let fixture = operation_fixture("all");
    let tool = send_tool(&fixture);
    let context = verified_context(&fixture.runtime, "alpha@party");
    let output = tool
        .invoke(
            &context,
            SendInput {
                targets: Vec::new(),
                broadcast: true,
                message: "hi".to_string(),
                request_id: None,
                bundle: BundleScope::Home,
            },
        )
        .await
        .expect("direct home broadcast succeeds");
    let mut targets: Vec<&str> = output
        .results
        .iter()
        .map(|result| result.target_session.as_str())
        .collect();
    targets.sort_unstable();
    assert_eq!(targets, vec!["bravo@party"]);
}

/// Broadcast fans out to the named scope's members except the sender.
#[tokio::test]
async fn direct_send_broadcast_fans_out_scope() {
    let fixture = operation_fixture("all");
    let tool = send_tool(&fixture);
    let context = verified_context(&fixture.runtime, "alpha@party");
    let output = tool
        .invoke(
            &context,
            SendInput {
                targets: Vec::new(),
                broadcast: true,
                message: "hi".to_string(),
                request_id: None,
                bundle: BundleScope::Named("party".to_string()),
            },
        )
        .await
        .expect("direct broadcast succeeds");
    let mut targets: Vec<&str> = output
        .results
        .iter()
        .map(|result| result.target_session.as_str())
        .collect();
    targets.sort_unstable();
    assert_eq!(targets, vec!["bravo@party"]);
}

/// A relay-wide home carries no bundle scope of its own: the
/// operation passes no bundle parameter (a bare name fused with a
/// GLOBAL home would classify as a relay-wide user and silently
/// become an explicit routing default), so dispatch applies its
/// parameter-required rule instead.
#[tokio::test]
async fn direct_send_broadcast_global_home_is_refused() {
    let fixture = operation_fixture("all");
    authorize_global_operator(&fixture, "op@GLOBAL");
    let tool = send_tool(&fixture);
    let context = verified_context(&fixture.runtime, "op@GLOBAL");
    let error = tool
        .invoke(
            &context,
            SendInput {
                targets: Vec::new(),
                broadcast: true,
                message: "hi".to_string(),
                request_id: None,
                bundle: BundleScope::Home,
            },
        )
        .await
        .expect_err("global-home broadcast must fail");
    assert_eq!(error.code, "validation_invalid_arguments");
    assert!(
        error
            .message
            .contains("explicit operation-bundle parameter"),
        "parameter-required refusal, not a silent default: {error:?}"
    );
}

/// A bare target under a relay-wide home stays unqualified: no GLOBAL
/// default is constructed, so the dispatch layer's own unqualified
/// rejection governs.
#[tokio::test]
async fn direct_send_bare_target_global_home_is_refused() {
    let fixture = operation_fixture("all");
    authorize_global_operator(&fixture, "op@GLOBAL");
    let tool = send_tool(&fixture);
    let context = verified_context(&fixture.runtime, "op@GLOBAL");
    let error = tool
        .invoke(&context, send_input(&["bravo"], "hi"))
        .await
        .expect_err("global-home bare target must fail");
    assert_eq!(error.code, "validation_unqualified_target");
}

/// Without a policy binding even a verified relay-wide context has no
/// resolved controls: provisioning is not authorization.
#[tokio::test]
async fn direct_send_unbound_global_has_no_controls() {
    let fixture = operation_fixture("all");
    let tool = send_tool(&fixture);
    let context = verified_context(&fixture.runtime, "op@GLOBAL");
    let error = tool
        .invoke(
            &context,
            SendInput {
                targets: vec!["carol@other".to_string()],
                broadcast: false,
                message: "hi".to_string(),
                request_id: None,
                bundle: BundleScope::Named("other".to_string()),
            },
        )
        .await
        .expect_err("unbound global must fail");
    assert_eq!(error.code, "validation_unknown_sender");
}

/// A positive control with the same authorized relay-wide context: a
/// Named-scope broadcast fans out to every scope member, since a
/// GLOBAL sender is no home member and nothing is excluded.
#[tokio::test]
async fn direct_send_global_operator_broadcasts_named_scope() {
    let fixture = operation_fixture("all");
    authorize_global_operator(&fixture, "op@GLOBAL");
    let tool = send_tool(&fixture);
    let context = verified_context(&fixture.runtime, "op@GLOBAL");
    let output = tool
        .invoke(
            &context,
            SendInput {
                targets: Vec::new(),
                broadcast: true,
                message: "hi".to_string(),
                request_id: None,
                bundle: BundleScope::Named("party".to_string()),
            },
        )
        .await
        .expect("authorized global broadcast succeeds");
    let mut targets: Vec<&str> = output
        .results
        .iter()
        .map(|result| result.target_session.as_str())
        .collect();
    targets.sort_unstable();
    assert_eq!(targets, vec!["alpha@party", "bravo@party"]);
}

/// Input shape validation rejects before anything touches the relay,
/// with the exact codes the MCP handler produces today.
#[tokio::test]
async fn direct_send_validates_input_shape() {
    let fixture = operation_fixture("all");
    let tool = send_tool(&fixture);
    let context = verified_context(&fixture.runtime, "alpha@party");
    let cases: Vec<(SendInput, &str)> = vec![
        (
            SendInput {
                message: "   ".to_string(),
                ..send_input(&["bravo"], "   ")
            },
            "validation_invalid_arguments",
        ),
        (
            SendInput {
                broadcast: true,
                ..send_input(&["bravo"], "hi")
            },
            "validation_conflicting_targets",
        ),
        (send_input(&[], "hi"), "validation_empty_targets"),
        (
            SendInput {
                bundle: BundleScope::All,
                ..send_input(&["bravo"], "hi")
            },
            "validation_invalid_arguments",
        ),
    ];
    for (input, code) in cases {
        let error = tool
            .invoke(&context, input)
            .await
            .expect_err("shape violation must fail");
        assert_eq!(error.code, code, "wrong code: {error:?}");
    }
}

/// A relay denial surfaces as the canonical error with code and
/// details unchanged; the operation synthesizes no authorization of
/// its own.
#[tokio::test]
async fn direct_send_surfaces_relay_denial_unchanged() {
    let fixture = operation_fixture("home");
    let tool = send_tool(&fixture);
    let context = verified_context(&fixture.runtime, "alpha@party");
    let error = tool
        .invoke(&context, send_input(&["carol@other"], "hi"))
        .await
        .expect_err("cross-bundle send must be denied");
    assert_eq!(error.code, "authorization_forbidden");
    assert!(
        error.details.is_some(),
        "denial details pass through: {error:?}"
    );
}

/// A rotated credential denies a previously minted context: binding
/// epoch revalidation runs on the operation path, not just the
/// socket path.
#[tokio::test]
async fn direct_send_rejects_stale_context_after_rotation() {
    let fixture = operation_fixture("all");
    let tool = send_tool(&fixture);
    let context = verified_context(&fixture.runtime, "alpha@party");
    rotate_principal_credential(
        &fixture.runtime,
        "alpha@party",
        agentmux::relay::CredentialDestination::Response,
    )
    .expect("rotate mid-flight");
    let error = tool
        .invoke(&context, send_input(&["bravo"], "hi"))
        .await
        .expect_err("rotated context must fail");
    assert_eq!(error.code, "validation_stale_verified_context");
}

/// A context minted by another runtime instance is foreign here.
#[tokio::test]
async fn direct_send_rejects_foreign_instance_context() {
    let first = operation_fixture("all");
    let second = operation_fixture("all");
    let tool = send_tool(&second);
    let context = verified_context(&first.runtime, "alpha@party");
    let error = tool
        .invoke(&context, send_input(&["bravo"], "hi"))
        .await
        .expect_err("foreign context must fail");
    assert_eq!(error.code, "validation_foreign_relay_instance");
}

/// Attribution comes only from the verified principal: the input
/// carries no sender field to abuse, and the outcome names the
/// verified context.
#[tokio::test]
async fn direct_send_attributes_requester_from_context() {
    let fixture = operation_fixture("all");
    let tool = send_tool(&fixture);
    let context = verified_context(&fixture.runtime, "alpha@party");
    let output = tool
        .invoke(&context, send_input(&["bravo"], "hi"))
        .await
        .expect("direct send succeeds");
    assert_eq!(output.requester_session, "alpha@party");
}

/// The unassociated-server failure never enters the canonical
/// operation error set: it is raised by MCP association before
/// delegation, which a direct call has no path to.
#[tokio::test]
async fn direct_send_never_reports_unassociated_server() {
    let open = operation_fixture("all");
    let open_tool = send_tool(&open);
    let open_context = verified_context(&open.runtime, "alpha@party");
    let strict = operation_fixture("home");
    let strict_tool = send_tool(&strict);
    let strict_context = verified_context(&strict.runtime, "alpha@party");
    let mut codes = Vec::new();
    for input in [
        send_input(&["bravo"], "   "),
        SendInput {
            broadcast: true,
            ..send_input(&["bravo"], "hi")
        },
        send_input(&[], "hi"),
        SendInput {
            bundle: BundleScope::All,
            ..send_input(&["bravo"], "hi")
        },
    ] {
        codes.push(
            open_tool
                .invoke(&open_context, input)
                .await
                .expect_err("shape violation must fail")
                .code,
        );
    }
    codes.push(
        strict_tool
            .invoke(&strict_context, send_input(&["carol@other"], "hi"))
            .await
            .expect_err("denial must fail")
            .code,
    );
    assert!(
        !codes
            .iter()
            .any(|code| code == "validation_unassociated_server"),
        "adapter-only failure leaked into operations: {codes:?}"
    );
}

/// Owned blackhole peer for the heartbeat test: accepts one dial,
/// signals once it has read the client's Hello line (proving the
/// measured interval is a withheld handshake, not a failed dial),
/// then holds the connection open without answering. All loops are
/// bounded (accept deadline, read timeout, hold cap, done flag), the
/// accepted descriptor is owned by the helper thread, and shutdown
/// joins it — no leaked descriptors, no detached sleepers, no
/// unbounded waits for the inline mutant to deadlock on.
struct BlackholePeer {
    done: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl BlackholePeer {
    fn spawn(
        socket_path: &std::path::Path,
        entered: tokio::sync::oneshot::Sender<()>,
    ) -> Self {
        use std::os::unix::net::UnixListener;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::{Duration, Instant};

        let listener =
            UnixListener::bind(socket_path).expect("blackhole binds");
        listener.set_nonblocking(true).expect("nonblocking accept");
        let done = Arc::new(AtomicBool::new(false));
        let done_thread = Arc::clone(&done);
        let handle = std::thread::spawn(move || {
            use std::io::Read as _;
            // Accept one dial, bounded: no dial ever means the test
            // fails at its entered-timeout, not here.
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut stream = loop {
                if done_thread.load(Ordering::Relaxed)
                    || Instant::now() > deadline
                {
                    return;
                }
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    },
                    Err(_) => return,
                }
            };
            let _ = stream.set_nonblocking(false);
            if stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .is_err()
            {
                // Without a read timeout the Hello read below could
                // park past every test bound; fail closed instead.
                return;
            }
            // Read the Hello frame line: entered fires only on an
            // observed client Hello, never on a bare connection.
            // Timeouts re-loop against the stop flag rather than
            // ending the wait: silence is not evidence either way.
            let mut line = Vec::new();
            let mut byte = [0u8; 1];
            let hello_seen = loop {
                if done_thread.load(Ordering::Relaxed) {
                    return;
                }
                match stream.read(&mut byte) {
                    Ok(0) => break false,
                    Ok(_) => {
                        line.push(byte[0]);
                        if byte[0] == b'\n' {
                            break true;
                        }
                        if line.len() > 65536 {
                            break false;
                        }
                    },
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            || error.kind()
                                == std::io::ErrorKind::TimedOut =>
                    {
                        continue;
                    },
                    Err(_) => break false,
                }
            };
            if !hello_seen {
                return;
            }
            let _ = entered.send(());
            // Hold without answering: the client's Hello-ack timeout
            // bounds the block. The done flag shortens teardown; the
            // cap bounds abandonment.
            let hold_until = Instant::now() + Duration::from_secs(30);
            while !done_thread.load(Ordering::Relaxed)
                && Instant::now() < hold_until
            {
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        Self {
            done,
            handle: Some(handle),
        }
    }

    fn shutdown(mut self) {
        use std::sync::atomic::Ordering;
        self.done.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            handle.join().expect("blackhole thread joins cleanly");
        }
    }
}

impl Drop for BlackholePeer {
    // Backstop: signals stop and reaps without asserting, so cleanup
    // runs on success, failure, and unwind paths alike without ever
    // double-panicking. The helper's loops are all capped, so it
    // always exits on its own after the flag is set.
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        self.done.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// The executor keeps ticking while dispatch blocks on an
/// unresponsive peer: the synchronous relay spine (including peer
/// forwarding) runs on the blocking pool, never on the async
/// worker. The blackhole accepts the dial and withholds the Hello
/// answer, so the forward parks on the handshake timeout while the
/// current-thread executor proves it is free. Ticks are counted only
/// after the peer signals it read our Hello, so they measure the
/// blocked interval — not scheduling before dispatch or an instant
/// dial failure (which would trip the entered timeout instead).
#[tokio::test]
async fn direct_send_heartbeat_during_blocked_forward() {
    use std::time::{Duration, Instant};

    let fixture = operation_fixture("all");
    let socket_path = fixture._temporary.path().join("blackhole.sock");
    let peers = vec![PeerConfiguration {
        alias: "blackhole".to_string(),
        address: socket_path.to_string_lossy().into_owned(),
        connect_as: "test-relay".to_string(),
    }];
    let wiring = DispatchWiring::mirrored(&fixture.runtime, &peers, true)
        .expect("mirrored wiring installs");
    let tool = SendTool::new(Arc::clone(&fixture.runtime), wiring);
    // The peer credential the prepare step reads per dial.
    let peer_dir = fixture._temporary.path().join("state").join("peers");
    std::fs::create_dir_all(&peer_dir).expect("peer dir");
    std::fs::write(peer_dir.join("blackhole.psk"), "blackhole-psk")
        .expect("peer credential");
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
    // All fallible setup happens before the helper spawns, so a setup
    // failure never strands a running peer thread behind a failed test.
    let context = verified_context(&fixture.runtime, "alpha@party");
    let input = SendInput {
        targets: vec!["victim@foreign!blackhole".to_string()],
        broadcast: false,
        message: "hi".to_string(),
        request_id: None,
        bundle: BundleScope::Home,
    };
    let blackhole = BlackholePeer::spawn(&socket_path, entered_tx);
    let started = Instant::now();
    let invoke = tool.invoke(&context, input);
    tokio::pin!(invoke);
    tokio::pin!(entered_rx);
    // Drive invoke from the start: the dial only happens once the
    // future is polled. Ticks count only after the peer signals it
    // read our Hello, so they measure the blocked interval — an
    // instant dial failure completes invoke with zero ticks and trips
    // the entered assertion instead. An independent absolute
    // deadline bounds the wait no matter which branch stalls.
    let overall_deadline = tokio::time::sleep(Duration::from_secs(30));
    tokio::pin!(overall_deadline);
    let mut entered_at: Option<Instant> = None;
    let mut ticks = 0u32;
    let outcome = loop {
        tokio::select! {
            result = &mut invoke => break Some(result),
            hello = &mut entered_rx, if entered_at.is_none() => {
                match hello {
                    Ok(()) => {
                        entered_at = Some(Instant::now());
                    },
                    Err(_) => break None,
                }
            }
            _ = &mut overall_deadline => break None,
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                if entered_at.is_some() {
                    ticks += 1;
                }
                if ticks >= 400 {
                    // 20 s of blocked ticks without completion: fail
                    // loudly instead of hanging. The inline-dispatch
                    // mutant can never reach this branch — it parks the
                    // executor before any tick, so it trips the count
                    // assert below with 0 ticks after a fast resolve.
                    break None;
                }
            }
        }
    };
    let elapsed = started.elapsed();
    // Cleanup first: the helper never outlives the test unboundedly.
    blackhole.shutdown();
    // entered_at is the inspectable record distinguishing real peer
    // observation (set above, before the outcome) from ticks counted
    // during the wait: a dial failure or a closed-before-read peer
    // trips this gate, not the tick count.
    assert!(
        entered_at.is_some(),
        "peer must observe the Hello: proves the measured interval is a withheld handshake, not a failed dial"
    );
    assert!(
        ticks >= 3,
        "executor must progress while forwarding blocks: {ticks} ticks after handshake entered"
    );
    assert!(
        elapsed < Duration::from_secs(45),
        "blocked forward must resolve within its bounds, not hang: {elapsed:?}"
    );
    let output = outcome
        .expect("invoke completes instead of hanging")
        .expect("blackhole forward resolves");
    assert_eq!(output.results.len(), 1);
    assert_eq!(output.results[0].target_session, "victim@foreign!blackhole");
    assert_eq!(output.results[0].outcome, SendOutcome::PeerUnavailable);
}

/// The public operation boundary names no MCP, rmcp, JSON-RPC, stdio,
/// or JSON schema type in its inputs, outputs, or error. This is an
/// outer-name spot check; the structural guarantee is the signature
/// itself (types live in `operations::inputs/outputs/error` and the
/// public runtime) plus the import list above, which names no MCP,
/// rmcp, stdio, or JSON-schema module.
#[test]
fn operation_surface_names_no_protocol_types() {
    for name in [
        std::any::type_name::<SendInput>(),
        std::any::type_name::<SendOutput>(),
        std::any::type_name::<OperationError>(),
        std::any::type_name::<SendTool>(),
    ] {
        for banned in [
            "rmcp",
            "schemars",
            "JsonSchema",
            "ToolRouter",
            "CallToolResult",
            "ErrorData",
            "::mcp::",
        ] {
            assert!(
                !name.contains(banned),
                "protocol type leaked into operation surface: {name}"
            );
        }
    }
}

/// The operation-rendered payload follows the documented MCP
/// contract assembly: optional fields present only when supplied.
/// This pins the operation side of the 4.2 parity obligation
/// (operation output renders to the documented payload); the
/// handler-delegation half, proving the MCP path renders the same
/// payload through this operation, lands with section 3.1. Until
/// then the goldens below are transcribed from the documented
/// contract, not observed from the adapter.
#[test]
fn send_output_renders_documented_contract() {
    use agentmux::relay::SendResult;
    let output = SendOutput {
        schema_version: "1".to_string(),
        request_id: Some("c-1".to_string()),
        requester_session: "alpha@party".to_string(),
        sender_display_name: Some("Alpha".to_string()),
        authenticated_identity: Some("alpha@party".to_string()),
        on_behalf_of: None,
        results: vec![SendResult {
            target_session: "bravo@party".to_string(),
            message_id: "m-1".to_string(),
            outcome: SendOutcome::Delivered,
            reason_code: None,
            reason: None,
            details: None,
        }],
    };
    let rendered = render_send_contract(&output);
    let expected = serde_json::json!({
        "schema_version": "1",
        "request_id": "c-1",
        "requester_session": "alpha@party",
        "sender_display_name": "Alpha",
        "authenticated_identity": "alpha@party",
        "results": [{
            "target_session": "bravo@party",
            "message_id": "m-1",
            "outcome": "delivered",
        }],
    });
    assert_eq!(rendered, expected);
    let mut bare = output;
    bare.request_id = None;
    bare.sender_display_name = None;
    bare.authenticated_identity = None;
    let rendered = render_send_contract(&bare);
    assert!(rendered.get("request_id").is_none());
    assert!(rendered.get("sender_display_name").is_none());
    assert!(rendered.get("authenticated_identity").is_none());
    assert!(rendered.get("on_behalf_of").is_none());
    assert!(rendered.get("results").is_some());
}

/// Test-local rendering implementing ONLY the documented contract
/// assembly (omit-absent optionals); the operation side must match
/// it exactly for adapter parity to hold.
fn render_send_contract(output: &SendOutput) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert(
        "schema_version".to_string(),
        serde_json::Value::String(output.schema_version.clone()),
    );
    if let Some(request_id) = &output.request_id {
        map.insert(
            "request_id".to_string(),
            serde_json::Value::String(request_id.clone()),
        );
    }
    map.insert(
        "requester_session".to_string(),
        serde_json::Value::String(output.requester_session.clone()),
    );
    if let Some(name) = &output.sender_display_name {
        map.insert(
            "sender_display_name".to_string(),
            serde_json::Value::String(name.clone()),
        );
    }
    if let Some(identity) = &output.authenticated_identity {
        map.insert(
            "authenticated_identity".to_string(),
            serde_json::Value::String(identity.clone()),
        );
    }
    if let Some(delegate) = &output.on_behalf_of {
        map.insert(
            "on_behalf_of".to_string(),
            serde_json::Value::String(delegate.clone()),
        );
    }
    map.insert(
        "results".to_string(),
        serde_json::to_value(&output.results).expect("results serialize"),
    );
    serde_json::Value::Object(map)
}
