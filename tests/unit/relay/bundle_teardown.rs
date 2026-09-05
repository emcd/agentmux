//! Tearing a bundle's runtime down ends the delivery workers it owns.
//!
//! Every teardown path — `down`, a watcher reload, a watcher unload — reaches a
//! bundle's runtime through `shutdown_bundle_runtime`, which used to prune tmux
//! sessions and nothing else. A worker survived all three, and on a reload the
//! surviving worker was handed back to the restarted bundle, so an edited
//! configuration could report success while the old generation kept serving.
//!
//! These tests drive a tmux target rather than an ACP one. The stop is a property
//! of the worker registry and is transport-agnostic — an ACP worker is reached by
//! the same key — and a tmux fixture costs a shell script where an ACP fixture
//! costs an agent process. What is *not* covered here is the ACP-specific
//! consequence of the worker ending, namely its child process being reaped.

use super::{dispatch_request, write_bundle, write_tui_configuration};
use agentmux::relay::RelayRequest;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// A fake tmux that lets a send run to completion.
///
/// Deliberately unlike the stateful fixture in `send.rs`: `paste-buffer` returns
/// immediately and the pane is always at a prompt, so the member delivers and the
/// worker settles idle. The teardown under test is then the only thing that ends
/// it, rather than competing with a write still in flight.
///
/// The catch-all arm answers every lifecycle query — `list-sessions` among them —
/// with no output, which reads as "this socket owns nothing". `kill-server` is
/// answered separately, the way a real tmux answers it when no server is running:
/// a non-zero exit carrying "no server running", which the lifecycle code reads
/// as nothing-to-reap rather than as a reap. Answering it with a bare success
/// instead would set `killed_tmux_server`, and the reporting test below would
/// then pass on a tmux effect it is meant to prove absent.
const IDLE_FAKE_TMUX: &str = r##"#!/usr/bin/env bash
set -euo pipefail

args=("$@")
if [[ "${#args[@]}" -ge 2 && "${args[0]}" == "-S" ]]; then
  args=("${args[@]:2}")
fi
if [[ "${#args[@]}" -eq 0 ]]; then
  exit 1
fi

case "${args[0]}" in
  display-message)
    case "${args[4]-}" in
      '#{pane_id}')
        printf "%%1\n"
        ;;
      '#{window_activity}')
        printf "1\n"
        ;;
      *)
        printf "\n"
        ;;
    esac
    ;;
  capture-pane)
    printf "ready\n"
    ;;
  load-buffer)
    cat - > /dev/null
    ;;
  kill-server)
    printf "no server running on %s\n" "${TMUX_SOCKET-socket}" >&2
    exit 1
    ;;
  *)
    :
    ;;
esac
"##;

/// The idle fake, but `paste-buffer` takes long enough that the relay's
/// submission watchdog arms while the write is still outstanding.
///
/// That is the only production route to a *replacement* generation: the watchdog
/// fires on a declaration nobody has acknowledged yet, fences the generation, and
/// builds a replacement behind a positive verdict. The sleep is sized so the
/// write is still outstanding when the watchdog arms and finishes well inside the
/// fence's observation window — the executor then acknowledges its unit, sees the
/// cooperative stop, and exits, which is what makes the verdict positive rather
/// than a fail-stop. Both margins are hundreds of milliseconds against seconds,
/// so this is not a race dressed as a fixture.
const SLOW_PASTE_FAKE_TMUX: &str = r##"#!/usr/bin/env bash
set -euo pipefail

args=("$@")
if [[ "${#args[@]}" -ge 2 && "${args[0]}" == "-S" ]]; then
  args=("${args[@]:2}")
fi
if [[ "${#args[@]}" -eq 0 ]]; then
  exit 1
fi

case "${args[0]}" in
  display-message)
    case "${args[4]-}" in
      '#{pane_id}')
        printf "%%1\n"
        ;;
      '#{window_activity}')
        printf "1\n"
        ;;
      *)
        printf "\n"
        ;;
    esac
    ;;
  capture-pane)
    printf "ready\n"
    ;;
  load-buffer)
    cat - > /dev/null
    ;;
  paste-buffer)
    sleep 1
    ;;
  kill-server)
    printf "no server running on %s\n" "${TMUX_SOCKET-socket}" >&2
    exit 1
    ;;
  *)
    :
    ;;
esac
"##;

fn write_slow_paste_fake_tmux(script_path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(script_path, SLOW_PASTE_FAKE_TMUX).expect("write slow-paste fake tmux");
    std::fs::set_permissions(script_path, std::fs::Permissions::from_mode(0o755))
        .expect("set slow-paste fake tmux executable");
}

fn write_idle_fake_tmux(script_path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(script_path, IDLE_FAKE_TMUX).expect("write idle fake tmux");
    std::fs::set_permissions(script_path, std::fs::Permissions::from_mode(0o755))
        .expect("set idle fake tmux executable");
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

/// Waits for `event` to appear, failing with what the log did hold.
fn await_inscription(path: &std::path::Path, event: &str, within: Duration) {
    let deadline = Instant::now() + within;
    while read_inscriptions(path, event).is_empty() {
        assert!(
            Instant::now() < deadline,
            "{event} never appeared within {within:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Tearing a bundle down stops the delivery workers it owns.
///
/// The worker is established rather than assumed: the send is waited on until it
/// reports delivered, which cannot happen without a registered worker having
/// carried it. `signalled_worker_count` then says how many workers the teardown
/// asked to stop, so a stop path that reached nothing would report zero here;
/// `unstopped_worker_count` separately says whether they were observed to leave,
/// so a worker that never acted on the request cannot hide behind the first
/// count. Both are asserted, because either alone can be satisfied by a
/// half-working stop.
#[test]
fn tearing_a_bundle_down_stops_the_workers_it_owns() {
    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    let fake_tmux = temporary.path().join("fake-tmux");
    write_idle_fake_tmux(&fake_tmux);
    // SAFETY: nextest runs each test in its own process, and this runs before the
    // first dispatch spawns anything that could read the environment.
    unsafe { std::env::set_var("AGENTMUX_TMUX_COMMAND", &fake_tmux) };

    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration(&config_root, "default");
    let tmux_socket = temporary.path().join("tmux.sock");
    let runtime_directory = temporary.path();

    dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("send response");

    // A delivered member proves a worker was elected and carried it, so the
    // teardown below is acting on a registry that demonstrably holds one.
    await_inscription(
        &inscriptions,
        "relay.send.async.completed",
        Duration::from_secs(10),
    );

    // Tearing down a *different* bundle must leave this one alone. The registry is
    // process-wide and one relay hosts many bundles, so a stop that matched too
    // loosely would take another bundle's workers down with it — a far worse
    // failure than the leak this change fixes.
    let other =
        agentmux::relay::shutdown_bundle_runtime("elsewhere", runtime_directory, &tmux_socket)
            .expect("unrelated bundle teardown");
    assert_eq!(
        other.signalled_worker_count, 0,
        "tearing down another bundle must not stop this bundle's workers"
    );

    let report = agentmux::relay::shutdown_bundle_runtime("party", runtime_directory, &tmux_socket)
        .expect("bundle teardown");

    assert_eq!(
        report.signalled_worker_count, 1,
        "the bundle's one delivery worker must be stopped by its teardown"
    );
    assert_eq!(
        report.unstopped_worker_count, 0,
        "no worker may still be registered once the teardown reports"
    );

    // The ending is named for what actually happened. A bundle stop reported as a
    // relay shutdown would tell an operator the relay was exiting while it is
    // still serving every other bundle.
    let verdicts = read_inscriptions(&inscriptions, "relay.delivery.fence.verdict");
    assert_eq!(
        verdicts.len(),
        1,
        "the teardown ends exactly one generation: {verdicts:?}"
    );
    let record: serde_json::Value =
        serde_json::from_str(verdicts[0].as_str()).expect("verdict is json");
    assert_eq!(
        record["details"]["trigger"], "bundle_stop",
        "the fence verdict names the bundle stop, not a relay shutdown: {record}"
    );
}

/// A teardown that stopped a worker but pruned no tmux session still reports the
/// bundle as having been hosted.
///
/// This is the shape an ACP-only bundle has: nothing to prune, no server to reap,
/// and a live worker per member. Deriving `changed` from tmux effects alone
/// reported `skipped` / `already_unhosted` for it — telling the operator nothing
/// had been running while its agents were still alive.
#[test]
fn a_teardown_with_no_tmux_effects_still_reports_the_bundle_hosted() {
    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    let fake_tmux = temporary.path().join("fake-tmux");
    write_idle_fake_tmux(&fake_tmux);
    // SAFETY: as above — one process per test, set before anything reads it.
    unsafe { std::env::set_var("AGENTMUX_TMUX_COMMAND", &fake_tmux) };

    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration(&config_root, "default");
    let tmux_socket = temporary.path().join("tmux.sock");
    let runtime_directory = temporary.path();

    dispatch_request(
        RelayRequest::Send {
            request_id: None,
            requester_session: "alpha".to_string(),
            message: "hello".to_string(),
            targets: vec!["bravo@party".to_string()],
            broadcast: false,
            on_behalf_of: None,
        },
        &config_root,
        "party",
        &tmux_socket,
    )
    .expect("send response");
    await_inscription(
        &inscriptions,
        "relay.send.async.completed",
        Duration::from_secs(10),
    );

    let report = agentmux::relay::shutdown_bundle_runtime("party", runtime_directory, &tmux_socket)
        .expect("bundle teardown");

    // The fixture's own premise: this teardown had no tmux work to do, so the
    // reporting below rests on the worker half alone.
    assert!(
        report.pruned_sessions.is_empty(),
        "the fixture owns no tmux sessions: {:?}",
        report.pruned_sessions
    );
    assert!(
        !report.killed_tmux_server,
        "the fixture reaps no tmux server"
    );
    assert_eq!(
        report.signalled_worker_count, 1,
        "the teardown's only effect is the worker it stopped"
    );

    // The derivation `down` reports on. Reading only the two tmux fields here
    // yields `false`, which is precisely the misreport.
    let changed = !report.pruned_sessions.is_empty()
        || report.killed_tmux_server
        || report.signalled_worker_count > 0;
    assert!(
        changed,
        "a teardown that stopped a worker changed the bundle's hosting state"
    );
}

/// A torn-down target can be served again, which is the half of the teardown the
/// tests above do not reach.
///
/// Stopping a worker is only half of giving a target up: the other half is the
/// reap that rides the unregister, which releases the target's consumer
/// generation so a later send can claim it. Those two were wired at different
/// times and only the first was covered, so a target could be stopped correctly
/// and then be unelectable for the rest of the process — every later send to it
/// failing worker construction, with the sender told
/// `internal_unexpected_failure` and no record naming the real cause.
///
/// The assertion is deliberately the second delivery rather than any ledger
/// state. What an operator loses when this regresses is the ability to `down` and
/// `up` a bundle and have it keep working, and a test that read the generation
/// out of the ledger would pass on a release that still left nothing able to
/// deliver.
#[test]
fn a_torn_down_target_can_be_served_again() {
    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    let fake_tmux = temporary.path().join("fake-tmux");
    write_idle_fake_tmux(&fake_tmux);
    // SAFETY: as above — one process per test, set before anything reads it.
    unsafe { std::env::set_var("AGENTMUX_TMUX_COMMAND", &fake_tmux) };

    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration(&config_root, "default");
    let tmux_socket = temporary.path().join("tmux.sock");
    let runtime_directory = temporary.path();

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
    await_inscription(
        &inscriptions,
        "relay.send.async.completed",
        Duration::from_secs(10),
    );

    let report = agentmux::relay::shutdown_bundle_runtime("party", runtime_directory, &tmux_socket)
        .expect("bundle teardown");
    // The premise: a worker really was stopped, so the second send below is
    // electing a fresh one rather than reusing a survivor. Without this the test
    // would still pass against a teardown that stopped nothing at all.
    assert_eq!(
        report.signalled_worker_count, 1,
        "the first send's worker must have been stopped for this to test re-election"
    );

    send("second");
    let deadline = Instant::now() + Duration::from_secs(10);
    while read_inscriptions(&inscriptions, "relay.send.async.completed").len() < 2 {
        assert!(
            Instant::now() < deadline,
            "the second send never resolved, so the torn-down target was never re-elected: {:?}",
            read_inscriptions(&inscriptions, "relay.send.async.completed")
        );
        std::thread::sleep(Duration::from_millis(25));
    }

    let completions = read_inscriptions(&inscriptions, "relay.send.async.completed");
    let second: serde_json::Value =
        serde_json::from_str(completions[1].as_str()).expect("completion is json");
    assert_eq!(
        second["details"]["outcome"], "delivered",
        "the second send must be delivered by a freshly elected worker, not refused \
         by a target the teardown failed to give up: {second}"
    );
}

/// A target that replaced its delivery generation can still be given up, and so
/// can still be served again afterwards.
///
/// The replacement is the second place a generation is issued, and it issues a
/// *new* identifier while the worker's registry entry still names the old one.
/// A fix that recorded the identifier only on the worker's first claim would
/// leave that entry naming a generation the ledger no longer holds, so the reap
/// riding the teardown would be refused exactly as it was before the fix — and
/// every test that never provoked a replacement would still pass.
///
/// Driven through the real watchdog rather than by calling the ledger: the write
/// is slow enough that `submission-timeout-ms` elapses with the unit still
/// declared, which arms the watchdog, fences the generation, and builds the
/// replacement behind a positive verdict. The verdict is asserted before the
/// teardown, because a fixture that stopped provoking the replacement would
/// otherwise quietly degrade into a second copy of the test above.
#[test]
fn a_target_that_replaced_its_generation_can_still_be_served_again() {
    use agentmux::relay::{DeliveryConfiguration, configure_delivery};

    let temporary = TempDir::new().expect("temporary");
    let inscriptions = temporary.path().join("inscriptions.log");
    let _ = agentmux::runtime::inscriptions::configure_process_inscriptions(&inscriptions);

    let fake_tmux = temporary.path().join("fake-tmux");
    write_slow_paste_fake_tmux(&fake_tmux);
    // SAFETY: as above — one process per test, set before anything reads it.
    unsafe { std::env::set_var("AGENTMUX_TMUX_COMMAND", &fake_tmux) };

    // The watchdog must arm while the write is still outstanding, and the fence
    // must then outlast that write so the executor can acknowledge and stop. A
    // fence window shorter than the paste would report cessation it never
    // observed and fail-stop the target instead of replacing its generation.
    //
    // The submission bound is the smallest an operator can actually configure.
    // `configure_delivery` takes the resolved struct and so validates nothing,
    // which makes it easy to write a fixture around a value `relay.toml` would
    // reject — and a fence provoked only by an impossible setting would prove
    // nothing about the shipped one.
    configure_delivery(DeliveryConfiguration {
        submission_timeout_ms: 500,
        fence_observation_timeout_ms: 5_000,
        unreachable_dwell_ms: 600_000,
        ..Default::default()
    });

    let config_root = write_bundle(&temporary, "party");
    write_tui_configuration(&config_root, "default");
    let tmux_socket = temporary.path().join("tmux.sock");
    let runtime_directory = temporary.path();

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
    await_inscription(
        &inscriptions,
        "relay.delivery.fence.verdict",
        Duration::from_secs(20),
    );

    // The fixture's premise, asserted rather than assumed: the watchdog is what
    // fenced this generation, and the verdict was positive, so a replacement was
    // built. A negative verdict would fail-stop the target instead, which is a
    // different path and would make the re-election below meaningless.
    let verdicts = read_inscriptions(&inscriptions, "relay.delivery.fence.verdict");
    let verdict: serde_json::Value =
        serde_json::from_str(verdicts[0].as_str()).expect("verdict is json");
    assert_eq!(
        verdict["details"]["trigger"], "submission_timeout",
        "the fixture must provoke the execution watchdog: {verdict}"
    );
    assert_eq!(
        verdict["details"]["verdict"], "positive",
        "a replacement is built only behind a positive verdict: {verdict}"
    );

    let report = agentmux::relay::shutdown_bundle_runtime("party", runtime_directory, &tmux_socket)
        .expect("bundle teardown");
    assert_eq!(
        report.signalled_worker_count, 1,
        "the replacement's worker must have been stopped for this to test re-election"
    );

    // The first send is waited out before anything is counted. It is delivered by
    // the outgoing generation, ahead of the fence that stops it — but nothing
    // orders that inscription against the verdict and the teardown above, so a
    // count taken while it was still in flight would let the wait below return
    // the first send's completion and report it as the second's. The recovery
    // would then be asserted by a delivery that preceded the teardown it is
    // supposed to have survived.
    // Its outcome is asserted too, not merely its arrival. The premise this whole
    // fixture rests on is that the outgoing generation's slow write *succeeded*
    // before the watchdog fenced it; a first member that resolved `failed` or
    // `submission_unknown` would mean the fence cut a write that never landed,
    // and the replacement below would be recovering from a different event than
    // the one this test claims to exercise.
    let first_deadline = Instant::now() + Duration::from_secs(20);
    let first = loop {
        let completions = read_inscriptions(&inscriptions, "relay.send.async.completed");
        if let Some(only) = completions.first() {
            break serde_json::from_str::<serde_json::Value>(only.as_str())
                .expect("completion is json");
        }
        assert!(
            first_deadline > Instant::now(),
            "the first send never resolved, so nothing below can be attributed"
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(
        first["details"]["outcome"], "delivered",
        "the outgoing generation must have completed its write before the fence \
         stopped it: {first}"
    );
    let settled_before = read_inscriptions(&inscriptions, "relay.send.async.completed").len();
    assert_eq!(
        settled_before, 1,
        "exactly one send has resolved so far, so the next completion can only be \
         the re-elected worker's"
    );

    send("second");
    let deadline = Instant::now() + Duration::from_secs(20);
    let second = loop {
        let completions = read_inscriptions(&inscriptions, "relay.send.async.completed");
        if completions.len() > settled_before {
            break serde_json::from_str::<serde_json::Value>(completions[settled_before].as_str())
                .expect("completion is json");
        }
        assert!(
            Instant::now() < deadline,
            "the second send never resolved after a generation replacement: {completions:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(
        second["details"]["outcome"], "delivered",
        "the target a replaced generation left behind was never given up, so the \
         second send could not be served: {second}"
    );
}
