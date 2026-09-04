//! Send operation tests, grouped by the concern each one pins:
//! - [`targeting`]: target resolution, alias rejection, broadcast, async
//!   dispatch.
//! - [`cross_relay`]: bang-path classification, resolution, and rejection.
//! - [`admission`]: what is refused before anything is queued — payload
//!   ceilings, unimplemented session types, configured quotas.
//! - [`queue_reporting`]: the undelivered aggregate and its warnings.
//! - [`bundle_stop`]: how a bundle teardown resolves the members it held.
//! - [`delivery_records`]: batch, partition, and terminal-record identity.
//! - [`policy_snapshot`]: when authorization policy is judged, and what a
//!   change to it may reach.
//! - [`unreachable`]: how an unreachable target resolves, and when it does
//!   not.
//!
//! Helpers shared across more than one of those clusters live in this hub.
//! Each is used by at least two sibling modules, which is why it is here
//! rather than beside its caller: the inscription readers span nearly every
//! cluster, and the stateful fake tmux is what both [`queue_reporting`] and
//! [`bundle_stop`] use to build a queue holding an authorized member and
//! waiting ones at the same time.
//!
//! Fixture writers general to the whole `relay` surface (`write_bundle`,
//! `write_tui_configuration`, the `dispatch_request` adapter, the dwell
//! configurers) stay in the parent hub and reach here through `use super::*`.

use super::*;

mod admission;
mod bundle_stop;
mod cross_relay;
mod delivery_records;
mod policy_snapshot;
mod queue_reporting;
mod targeting;
mod unreachable;

/// Counts `relay.send.async.queued` inscriptions naming bravo. The `queued`
/// inscription is written synchronously before the send response returns, so its
/// absence after a refused request is decidable at that point.
fn count_bravo_queued_inscriptions(path: &std::path::Path) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| {
            line.contains("\"event\":\"relay.send.async.queued\"")
                && line.contains("\"target_session\":\"bravo\"")
        })
        .count()
}

/// Fake tmux with two switchable behaviours, addressed through sidecar files
/// rather than arguments so a test can change what the target reports without
/// restarting anything.
///
/// Substituted with `replace` rather than `format!`: the body is almost entirely
/// `${...}` expansions, and doubling every brace for the formatter would bury
/// the shell this fixture is actually made of.
const STATEFUL_FAKE_TMUX: &str = r##"#!/usr/bin/env bash
set -euo pipefail

BUSY_FILE="@BUSY_FILE@"
PASTED_FILE="@PASTED_FILE@"

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
    if [[ -f "${BUSY_FILE}" ]]; then
      printf "agent is working\n"
    else
      printf "READY-FOR-HANDOVER\n"
    fi
    ;;
  load-buffer)
    cat - > /dev/null
    ;;
  paste-buffer)
    printf "1\n" > "${PASTED_FILE}"
    sleep 60
    ;;
  *)
    :
    ;;
esac
"##;

/// Writes the fake tmux this fixture drives, and returns nothing: every knob it
/// has is a file path the caller already holds.
///
/// Three behaviours, each load-bearing:
///
/// - **`capture-pane` reports a prompt until `busy_file` exists.** That is the
///   readiness axis, and flipping it is what leaves later entries queued and
///   undeclared. It is used rather than the activity marker because readiness is
///   read from a cached observation the transport refreshes on its own clock: an
///   advancing marker would only suppress a write when two executor reads
///   happened to straddle an observer poll, which is a race, while an unready
///   pane suppresses every read after the flip.
/// - **`paste-buffer` never returns.** The executor declares one entry and then
///   parks inside its write, so that entry stays declared for as long as the test
///   needs. The sleep is bounded rather than infinite so the orphan it leaves
///   reaps itself; nothing in the test outlives it.
/// - **`display-message` answers a fixed pane and a constant activity marker.**
///   A constant can never advance, so the activity axis never suppresses a write
///   and the readiness flip above is the only thing that does.
fn write_stateful_fake_tmux(
    script_path: &std::path::Path,
    busy_file: &std::path::Path,
    pasted_file: &std::path::Path,
) {
    use std::os::unix::fs::PermissionsExt;

    let body = STATEFUL_FAKE_TMUX
        .replace(
            "@BUSY_FILE@",
            busy_file.to_str().expect("busy path is utf-8"),
        )
        .replace(
            "@PASTED_FILE@",
            pasted_file.to_str().expect("pasted path is utf-8"),
        );
    std::fs::write(script_path, body).expect("write stateful fake tmux");
    std::fs::set_permissions(script_path, std::fs::Permissions::from_mode(0o755))
        .expect("set stateful fake tmux executable");
}

/// Republishes the bundle's coder with a prompt-readiness template.
///
/// Without one the transport reports ready whenever the pane can be captured at
/// all, so the fake tmux above would have no way to say "reachable, but not at a
/// prompt" — the state that separates a held member from an unreachable one.
fn write_prompt_readiness_coders(configuration_roots: &ConfigurationRoots) {
    std::fs::write(
        configuration_roots.base_layer().join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "shell"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
prompt-regex = '^READY-FOR-HANDOVER$'
"#,
    )
    .expect("write prompt-readiness coders file");
}

/// Polls for the first inscription line for `event`. The terminal record is
/// written by the delivery worker task rather than on the request path, so it is
/// not present when the send response returns.
fn await_inscription(path: &std::path::Path, event: &str) -> String {
    await_inscription_within(path, event, std::time::Duration::from_secs(5))
}

/// `await_inscription` with an explicit bound, for the one scenario whose
/// completion is gated on a transport's own wait rather than on the relay.
fn await_inscription_within(
    path: &std::path::Path,
    event: &str,
    bound: std::time::Duration,
) -> String {
    let deadline = std::time::Instant::now() + bound;
    loop {
        if let Some(line) = read_inscriptions(path, event).into_iter().next() {
            return line;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no {event} inscription within {bound:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Counts inscription lines for exactly `event`. The aggregate and warning event
/// names share a prefix, so matching on the closing quote keeps the aggregate
/// count from absorbing warnings.
fn count_inscriptions(path: &std::path::Path, event: &str) -> usize {
    read_inscriptions(path, event).len()
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
