use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;

use super::super::*;

/// Asserts the relay armed its bundle watcher before publishing readiness.
/// `relay.bundle.watch.started` is inscribed before the ready sentinel, so
/// after `wait_for_relay_ready` its absence means the watch could not be
/// created (for example inotify instance exhaustion) and the relay is serving
/// without reconciliation — every watcher signal the test waits on afterwards
/// would starve for the full budget with no explanation.
pub(super) fn assert_bundle_watch_started(inscriptions_root: &Path) {
    let log = fs::read_to_string(inscriptions_root.join("relay.log")).unwrap_or_default();
    let started = log
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|entry| entry["event"] == "relay.bundle.watch.started");
    assert!(
        started,
        "relay published readiness without arming the bundle watcher; relay inscriptions: {log}"
    );
}

/// Unwraps a watcher-driven signal, panicking with the relay inscriptions log
/// so a starved wait reports how far the watcher actually got (event observed?
/// unload/reload inscribed?) instead of a bare expect message.
pub(super) fn expect_watcher_signal<T>(
    signal: Option<T>,
    what: &str,
    inscriptions_root: &Path,
) -> T {
    signal.unwrap_or_else(|| {
        panic!(
            "{what} not observed within {WATCHER_SIGNAL_WAIT_BUDGET:?}; relay inscriptions: {}",
            fs::read_to_string(inscriptions_root.join("relay.log")).unwrap_or_default()
        )
    })
}

/// Polls the relay inscriptions log until an entry satisfies `matches`,
/// returning `true` on a match or `false` once the deadline passes. Used to
/// observe a watcher outcome that emits no stream frame (so it cannot be awaited
/// on a keepalive connection).
pub(super) fn poll_inscriptions(
    inscriptions_root: &Path,
    matches: impl Fn(&Value) -> bool,
) -> bool {
    let log = inscriptions_root.join("relay.log");
    let deadline = Instant::now() + WATCHER_SIGNAL_WAIT_BUDGET;
    loop {
        if let Ok(contents) = fs::read_to_string(&log) {
            let found = contents
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .any(|entry| matches(&entry));
            if found {
                return true;
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Polls for an entry with `event` naming `bundle_name`.
pub(super) fn poll_inscription_event(
    inscriptions_root: &Path,
    event: &str,
    bundle_name: &str,
) -> bool {
    poll_inscriptions(inscriptions_root, |entry| {
        entry["event"] == event && entry["details"]["bundle_name"] == bundle_name
    })
}

/// Polls for an entry with `event`, whatever it names.
///
/// The bundle-scoped variant cannot serve a reconciliation suppressed by an
/// unreadable layer: enumeration never got far enough to know which bundles were
/// involved, so the inscription names the layer rather than a bundle.
pub(super) fn poll_inscription_event_kind(inscriptions_root: &Path, event: &str) -> bool {
    poll_inscriptions(inscriptions_root, |entry| entry["event"] == event)
}
