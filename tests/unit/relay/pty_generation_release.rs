//! A generation issued for a transport that then fails to build is still given
//! back.
//!
//! `build_generation` issues a consumer generation, records it on the worker's
//! registry entry, and only then builds the transport. The recording has to
//! happen on the near side of that build: the reap riding an unregister names
//! whatever the entry carries, so an entry that only learned of a generation
//! once its transport existed could never release one whose transport did not.
//! The target would then be held by a generation no worker owns, and every later
//! send to it would fail its own construction.
//!
//! Pty is the only transport these can be written against. tmux, UI and ACP all
//! have infallible `startup` — an ACP bootstrap fails later, supervised, and not
//! during construction — so none of them can be made to fail after a generation
//! is issued. Pty spawns the configured `initial-command` as a real child, which
//! makes a missing program a deterministic construction failure with no timing in
//! it: the file is either there or it is not.

use super::{dispatch_request, write_tui_configuration};
use agentmux::configuration::ConfigurationRoots;
use agentmux::relay::RelayRequest;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// The child a successful Pty build spawns.
///
/// It reports its own start by touching a marker, so a test can wait for a build
/// to have actually happened rather than sleeping, and then holds the pty open
/// without reading it.
const AGENT_SCRIPT: &str = "#!/bin/sh\nprintf 1 > \"$AGENT_MARKER\"\nexec sleep 45\n";

/// The same child, but it leaves its terminal unread for a moment before
/// draining it.
///
/// A Pty write goes to the pty master, so it completes as fast as the child
/// reads. A child that reads nothing at first lets the terminal's input buffer
/// fill and the write block, which is what holds a declaration open past the
/// submission bound and arms the watchdog; the drain that follows lets the write
/// finish, so the executor acknowledges and stops cooperatively and the fence
/// reaches a positive verdict instead of a fail-stop.
const SLOW_DRAIN_AGENT_SCRIPT: &str =
    "#!/bin/sh\nprintf 1 > \"$AGENT_MARKER\"\nsleep 1\nexec cat > /dev/null\n";

fn write_script(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, body).expect("write agent script");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("set agent script executable");
}

fn write_agent_script(path: &std::path::Path) {
    write_script(path, AGENT_SCRIPT);
}

/// Waits for `path` to exist, so a test can act on a child having started rather
/// than on a duration.
fn await_path(path: &std::path::Path, within: Duration, what: &str) {
    let deadline = Instant::now() + within;
    while !path.exists() {
        assert!(Instant::now() < deadline, "{what} within {within:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A bundle whose target is a Pty member running `initial_command`.
///
/// No prompt readiness is configured, so `prompt_satisfied` holds unconditionally
/// and the target is ready as soon as its worker exists. That is deliberate:
/// these tests are about a generation being released, and a readiness gate would
/// only add a second reason for a member not to be delivered.
fn write_pty_bundle(
    temporary: &TempDir,
    name: &str,
    initial_command: &std::path::Path,
    marker: &std::path::Path,
) -> ConfigurationRoots {
    let root = temporary.path().join("config");
    let bundles = root.join("bundles");
    std::fs::create_dir_all(&bundles).expect("create bundles directory");
    std::fs::write(
        root.join("coders.toml"),
        format!(
            r#"
format-version = 1

[[coders]]
id = "agent"

[coders.pty]
initial-command = "{command}"
resume-command = "{command}"
"#,
            command = initial_command.display()
        ),
    )
    .expect("write pty coders file");
    std::fs::write(
        root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "home"
look = "self"
send = "home"
"#,
    )
    .expect("write policies file");
    std::fs::write(
        bundles.join(format!("{name}.toml")),
        format!(
            r#"
format-version = 1

[[sessions]]
id = "alpha"
name = "Alpha"
directory = "/tmp"
coder = "agent"

[[sessions]]
id = "bravo"
name = "Bravo"
directory = "/tmp"
coder = "agent"

[[sessions.environment]]
name = "AGENT_MARKER"
value = "{marker}"
"#,
            marker = marker.display()
        ),
    )
    .expect("write pty bundle file");
    ConfigurationRoots::single(root)
}

fn read_inscriptions(path: &std::path::Path, event: &str) -> Vec<String> {
    let needle = format!("\"event\":\"{event}\"");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains(needle.as_str()))
        .map(str::to_string)
        .collect()
}

/// Waits for one more `relay.send.async.completed` than `settled_before`, and
/// returns it parsed.
fn await_completion_after(
    inscriptions: &std::path::Path,
    settled_before: usize,
    within: Duration,
) -> serde_json::Value {
    let deadline = Instant::now() + within;
    loop {
        let completions = read_inscriptions(inscriptions, "relay.send.async.completed");
        if completions.len() > settled_before {
            return serde_json::from_str(completions[settled_before].as_str())
                .expect("completion is json");
        }
        assert!(
            Instant::now() < deadline,
            "no completion beyond {settled_before} within {within:?}: {completions:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// A target whose transport failed to build can still be served once the cause
/// is gone.
///
/// This is the case that pins *where* the generation is recorded, which neither
/// teardown test can reach: both of those build their transports successfully, so
/// recording after the build would satisfy them while still leaking every
/// generation whose transport failed. Here the build fails on the near side of
/// that recording, so a worker that had not yet noted what it holds unregisters
/// naming nothing, the ledger refuses the reap, and the target is retired for the
/// life of the process — from one missing program, recovered by nothing.
#[test]
fn a_target_whose_transport_failed_to_build_can_still_be_served() {
    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    let agent = temporary.path().join("agent.sh");
    let marker = temporary.path().join("agent-started");
    // Deliberately not written yet: the first build must fail, and a missing
    // program is the one construction failure with no timing in it.
    let config_root = write_pty_bundle(&temporary, "party", &agent, &marker);
    write_tui_configuration(&config_root, "default");
    let tmux_socket = temporary.path().join("tmux.sock");

    let send = |message: &str| {
        dispatch_request(
            RelayRequest::Send {
                request_id: None,
                requester_session: "alpha".to_string(),
                message: message.to_string(),
                targets: vec!["bravo@party".to_string()],
                broadcast: false,
                on_behalf_of: None,
            },
            &config_root,
            "party",
            &tmux_socket,
        )
        .expect("send response");
    };

    send("first");
    let first = await_completion_after(&inscriptions, 0, Duration::from_secs(15));

    // The fixture's premise: the first send failed because the transport could
    // not be built, which is the only failure that leaves a generation issued
    // with no transport behind it. Any other spelling here means the test stopped
    // exercising the path it was written for.
    assert_eq!(
        first["details"]["outcome"], "failed",
        "the first send must fail: its program does not exist: {first}"
    );
    assert!(
        first["details"]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("spawn"),
        "the first send must fail in the Pty spawn, not somewhere else: {first}"
    );

    // The cause removed. Nothing else about the target has changed, so a send
    // that still fails now is failing on the generation the last one left behind.
    write_agent_script(&agent);

    send("second");
    let second = await_completion_after(&inscriptions, 1, Duration::from_secs(15));
    assert_eq!(
        second["details"]["outcome"], "delivered",
        "a failed construction must give the target back, so the next send can \
         build a transport and deliver: {second}"
    );
}

/// A replacement generation whose transport fails to build gives the target back
/// too.
///
/// This is the recovery path the relay deliberately spells as recoverable. A
/// positive fence verdict establishes that the outgoing generation ceased, so the
/// worker rebuilds in place; when that rebuild fails it unregisters rather than
/// fail-stopping, precisely so a later send can elect a fresh worker. The
/// generation the failed rebuild issued has to come back with it, or the
/// recoverable spelling recovers nothing — the target is retired exactly as hard
/// as a fail-stop, but reported with a code that tells an operator to retry.
///
/// Every step is provoked through the shipped machinery: the watchdog arms
/// because a real write is still outstanding, the verdict is positive because the
/// executor really does finish and stop, and the rebuild fails because the
/// program really is gone.
#[test]
fn a_replacement_whose_transport_failed_to_build_gives_the_target_back() {
    use agentmux::relay::{DeliveryConfiguration, configure_delivery};

    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    let agent = temporary.path().join("agent.sh");
    let marker = temporary.path().join("agent-started");
    write_script(&agent, SLOW_DRAIN_AGENT_SCRIPT);

    // The smallest submission bound an operator can configure, against a write
    // held open for about a second, inside a fence window of five. Both margins
    // are hundreds of milliseconds against seconds.
    configure_delivery(DeliveryConfiguration {
        submission_timeout_ms: 500,
        fence_observation_timeout_ms: 5_000,
        unreachable_dwell_ms: 600_000,
        ..Default::default()
    });

    let config_root = write_pty_bundle(&temporary, "party", &agent, &marker);
    write_tui_configuration(&config_root, "default");
    let tmux_socket = temporary.path().join("tmux.sock");

    let send = |message: String| {
        dispatch_request(
            RelayRequest::Send {
                request_id: None,
                requester_session: "alpha".to_string(),
                message,
                targets: vec!["bravo@party".to_string()],
                broadcast: false,
                on_behalf_of: None,
            },
            &config_root,
            "party",
            &tmux_socket,
        )
        .expect("send response");
    };

    // Many short lines rather than one long one: a terminal in canonical mode
    // bounds a single line, and a message that exceeded that bound would stall on
    // the line discipline rather than on the child not reading — which is a
    // different stall, and not one the watchdog should be provoked by.
    let bulk = "0123456789abcdefghijklmnopqrstuvwxyz\n".repeat(3_000);
    send(bulk);

    await_path(
        &marker,
        Duration::from_secs(15),
        "the first Pty child never started, so no generation was ever built",
    );

    // The rebuild's program removed while the first generation is still writing.
    // The marker goes with it, so its absence below is evidence that no second
    // child was spawned rather than a leftover from the first.
    std::fs::remove_file(&agent).expect("remove agent script");
    std::fs::remove_file(&marker).expect("remove start marker");

    let deadline = Instant::now() + Duration::from_secs(30);
    let verdict = loop {
        let verdicts = read_inscriptions(&inscriptions, "relay.delivery.fence.verdict");
        if let Some(first) = verdicts.first() {
            break serde_json::from_str::<serde_json::Value>(first.as_str())
                .expect("verdict is json");
        }
        assert!(
            Instant::now() < deadline,
            "the submission watchdog never fenced the generation"
        );
        std::thread::sleep(Duration::from_millis(25));
    };

    // The fixture's premise, in two parts. The watchdog is what fenced this, and
    // the verdict is positive — a negative one fail-stops the target instead,
    // which is the path this test exists to distinguish itself from.
    assert_eq!(
        verdict["details"]["trigger"], "submission_timeout",
        "the fixture must provoke the execution watchdog: {verdict}"
    );
    assert_eq!(
        verdict["details"]["verdict"], "positive",
        "a replacement is attempted only behind a positive verdict: {verdict}"
    );

    // And the premise this test adds to the tmux one: the replacement's build
    // actually failed. A rebuild that succeeded would have spawned a child and
    // put the marker back.
    std::thread::sleep(Duration::from_millis(750));
    assert!(
        !marker.exists(),
        "the replacement build must fail for this to test the failed-rebuild path"
    );

    // The first send's own completion, waited for rather than assumed to have
    // landed already. Its member was acknowledged when the slow write finished,
    // which is before the verdict above in practice — but nothing *orders* that
    // inscription against the verdict, and a count taken while it was still in
    // flight would leave the wait below free to return the first send's
    // completion and report it as the second's. The recovery would then be
    // asserted by a delivery that happened before the failure it is supposed to
    // have recovered from.
    let first = await_completion_after(&inscriptions, 0, Duration::from_secs(15));
    assert_eq!(
        first["details"]["outcome"], "delivered",
        "the outgoing generation delivered its member before the fence stopped it: {first}"
    );
    let settled_before = read_inscriptions(&inscriptions, "relay.send.async.completed").len();
    assert_eq!(
        settled_before, 1,
        "exactly one send has resolved so far, so the next completion can only be \
         the recovery send's"
    );

    // The cause removed, exactly as in the first test. What must happen now is
    // the recovery the unregister-rather-than-fail-stop spelling promises.
    write_agent_script(&agent);
    send("second".to_string());
    let second = await_completion_after(&inscriptions, settled_before, Duration::from_secs(15));
    assert_eq!(
        second["details"]["outcome"], "delivered",
        "a replacement that could not be built must still give the target back, or \
         the recoverable teardown recovers nothing: {second}"
    );
    await_path(
        &marker,
        Duration::from_secs(5),
        "no fresh Pty child was spawned, so nothing was re-elected",
    );
}
