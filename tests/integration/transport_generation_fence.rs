//! What the generation fence is allowed to destroy, driven against a live tmux
//! server.
//!
//! The fence's third step is destructive by design, and the boundary of what it
//! may destroy is not something a controllable generation can be asked about:
//! `unit/delivery_fence.rs` covers the protocol's ordering, windows, and
//! verdicts, but only a real tmux server can answer whether it survived one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use agentmux::configuration::{BundleMember, TargetConfiguration, TmuxTargetConfiguration};
use agentmux::envelope::{AddressIdentity, PromptBatchSettings};
use agentmux::relay::{FenceResolution, FenceVerdict, acknowledge_fence};
use agentmux::runtime::paths::tmux_socket_path_for_runtime_directory;
use agentmux::tmux::TmuxTransport;
use agentmux::transports::{
    ChoiceMade, DeliveryEnvelope, DeliveryMessage, PackingUnitId, PartitionError, PartitionSink,
    StartupContext, SubmissionEvidence, Transport,
};
use tempfile::TempDir;

use crate::support::relay_delivery::{
    TmuxServerGuard, capture_pane, spawn_session, tmux_available, tmux_command,
    wait_for_pane_contains,
};

/// Accepts every declaration, which is what a real relay does for members it has
/// admitted. The transport's partition reporting is covered elsewhere; here it
/// only has to stay on its production path.
struct AcceptingSink;

impl PartitionSink for AcceptingSink {
    fn declare(&self, _member_ids: &[&str]) -> Result<PackingUnitId, PartitionError> {
        Ok(PackingUnitId::mint())
    }

    fn record(&self, _unit: PackingUnitId, _evidence: SubmissionEvidence) {}
}

fn tmux_member(session: &str) -> BundleMember {
    BundleMember {
        id: session.to_string(),
        name: None,
        working_directory: None,
        target: TargetConfiguration::Tmux(TmuxTargetConfiguration {
            start_command: "sh -lc 'exec sleep 45'".to_string(),
            prompt_readiness: None,
        }),
        coder_session_id: None,
        policy_id: None,
        environment: Vec::new(),
    }
}

fn envelope(body: &str) -> DeliveryEnvelope {
    DeliveryEnvelope {
        message_id: "m-fence".to_string(),
        message: DeliveryMessage {
            body: body.to_string(),
            created_at: "2026-08-12T00:00:00Z".to_string(),
            namespace: "party".to_string(),
            sender: AddressIdentity {
                session_name: "alice@party".to_string(),
                display_name: None,
            },
            target: AddressIdentity {
                session_name: "alpha@party".to_string(),
                display_name: None,
            },
            cc: Vec::new(),
            authenticated_identity: None,
            on_behalf_of: None,
        },
        append_enter: false,
        choice_decider_sessions: Vec::new(),
        quiet_window: Duration::from_millis(50),
        is_receipt: false,
    }
}

/// Forced termination reaches this generation's tmux clients and stops there.
///
/// The tmux server is not owned by the generation being fenced — it holds the
/// operator's sessions, including targets no delivery is in flight against — so
/// terminating it to fence one delivery would destroy exactly the work the fence
/// exists to protect. The reachable mistake is not malice but convenience:
/// `kill-server` is the one tmux call guaranteed to stop every client of a
/// generation at once, and it would make the second observation succeed every
/// time.
///
/// So the generation is given something real to lose. It writes to its pane
/// before the fence begins, which puts actual tmux clients behind both its
/// threads, and a bystander session shares the server without being any part of
/// it.
#[test]
fn fencing_a_tmux_generation_leaves_the_server_and_its_sessions_running() {
    if !tmux_available() {
        eprintln!("skipping tmux generation-fence test because tmux is unavailable");
        return;
    }

    let temporary = TempDir::new().expect("temporary directory");
    let runtime_directory = temporary.path().to_path_buf();
    let socket = tmux_socket_path_for_runtime_directory(&runtime_directory);

    spawn_session(&socket, "alpha", "exec sleep 45");
    spawn_session(&socket, "bystander", "exec sleep 45");
    let _server = TmuxServerGuard::new(socket.clone());

    let mut transport = TmuxTransport::new(
        PromptBatchSettings::default(),
        None,
        Arc::new(AcceptingSink) as Arc<dyn PartitionSink>,
    );
    transport
        .startup(StartupContext {
            namespace: "party".to_string(),
            runtime_directory,
            target_member: tmux_member("alpha"),
            choose: Arc::new(|_| ChoiceMade::Cancelled {
                decided_by: String::new(),
                reason_code: "not_applicable".to_string(),
                reason: None,
            }),
        })
        .expect("tmux transport startup");

    // Write first, so the generation being fenced is one that has actually driven
    // tmux rather than one that only ever held idle threads.
    Transport::mailw(&mut transport, envelope("fence-marker"))
        .blocking_recv()
        .expect("mailw outcome future resolves");
    wait_for_pane_contains(
        &socket,
        "alpha",
        "fence-marker",
        Duration::from_millis(5_000),
    );

    let outcome = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build multi-thread runtime")
        .block_on(acknowledge_fence(
            &mut transport,
            Duration::from_millis(200),
        ));

    assert_eq!(outcome.verdict, FenceVerdict::Positive);
    // Not incidental to the test — it is its precondition. The delivery thread
    // parks on its channel, where the cooperative flag cannot reach it, so only
    // the destructive step returns it. A `Cooperative` resolution here would mean
    // that step never ran, and every assertion below would be vacuous.
    assert_eq!(
        outcome.resolution,
        FenceResolution::Forced,
        "the destructive step must have run for the assertions below to mean anything",
    );

    let sessions = tmux_command(&socket, &["list-sessions", "-F", "#{session_name}"]);
    assert!(
        sessions.status.success(),
        "the tmux server must survive the fence: {}",
        String::from_utf8_lossy(&sessions.stderr),
    );
    let listed = String::from_utf8_lossy(&sessions.stdout);
    assert!(
        listed.lines().any(|name| name == "alpha"),
        "the fenced generation's own target session must survive: {listed:?}",
    );
    assert!(
        listed.lines().any(|name| name == "bystander"),
        "a session the fence has no business touching must survive: {listed:?}",
    );

    // The pane keeps what the fenced generation wrote to it. A server that was
    // killed and a session that was recreated would both list as present, so the
    // surviving scrollback is what separates survival from replacement.
    assert!(
        capture_pane(&socket, "alpha", "-40").contains("fence-marker"),
        "the target pane must be the same one the generation wrote to",
    );

    // The fence's own verdict is the transport's; re-reading it after the tmux
    // assertions guards against a cessation observation that only holds while the
    // server is being torn down.
    let started = Instant::now();
    while !agentmux::transports::GenerationFence::generation_ceased(&transport) {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a positively fenced generation must stay ceased",
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
