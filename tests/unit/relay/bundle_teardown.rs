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
