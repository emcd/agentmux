//! Bundle-file watcher behavior: load new bundle, load held non-autostart bundle,
//! unload removed bundle, retain the catalog when a layer becomes unreadable,
//! reload modified bundle, preserve down-intent across edit, hold non-autostart
//! bundle on edit, `--no-watch` reconcile-disable, and `watch-bundles = false`
//! reconcile-disable.

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use agentmux::runtime::paths::RelayRuntimePaths;
use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;
use crate::support::permissions::{deny_directory_access, report_permission_fixture_skip};

/// Negative-assertion budget for `--no-watch` and `watch-bundles = false`
/// tests: how long to wait after writing a runtime bundle before asserting
/// the relay still reports it as unknown. A watcher (if one existed)
/// would have had this long to reconcile. The watcher polls on the
/// bundle-watcher's internal interval (well under 1 second even on
/// slow CI); 1 second is generous margin to ensure a missing watcher
/// is correctly distinguished from a slow watcher.
const WATCHER_RECONCILE_BUDGET: Duration = Duration::from_secs(1);

/// Opens a persistent stream connection, sends one Hello, and returns the live
/// stream, a buffered reader (with a read timeout so a missing frame fails the
/// test rather than hanging), and the first server frame. Held open so a
/// later watcher-driven eviction frame can be observed on the same connection.
fn relay_hello_keepalive(
    state_root: &Path,
    principal_id: &str,
    identity_token: &str,
) -> (UnixStream, BufReader<UnixStream>, Value) {
    let socket = RelayRuntimePaths::resolve(state_root).relay_socket;
    let mut stream = UnixStream::connect(&socket).expect("connect relay socket");
    let reader_stream = stream.try_clone().expect("clone relay stream");
    reader_stream
        .set_read_timeout(Some(WATCHER_SIGNAL_WAIT_BUDGET))
        .expect("set relay read timeout");
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
    let mut reader = BufReader::new(reader_stream);
    let frame = read_next_frame(&mut reader).expect("hello frame");
    (stream, reader, frame)
}

/// Reads the next newline-delimited frame, or `None` on EOF or read timeout.
fn read_next_frame(reader: &mut BufReader<UnixStream>) -> Option<Value> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => None,
        Ok(_) => Some(serde_json::from_str(line.trim_end()).expect("decode relay frame")),
        Err(_) => None,
    }
}

/// Asserts the relay armed its bundle watcher before publishing readiness.
/// `relay.bundle.watch.started` is inscribed before the ready sentinel, so
/// after `wait_for_relay_ready` its absence means the watch could not be
/// created (for example inotify instance exhaustion) and the relay is serving
/// without reconciliation — every watcher signal the test waits on afterwards
/// would starve for the full budget with no explanation.
fn assert_bundle_watch_started(inscriptions_root: &Path) {
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
fn expect_watcher_signal<T>(signal: Option<T>, what: &str, inscriptions_root: &Path) -> T {
    signal.unwrap_or_else(|| {
        panic!(
            "{what} not observed within {WATCHER_SIGNAL_WAIT_BUDGET:?}; relay inscriptions: {}",
            fs::read_to_string(inscriptions_root.join("relay.log")).unwrap_or_default()
        )
    })
}

/// Repeatedly opens a fresh Hello connection until `accept` matches the returned
/// frame or the deadline passes. Used to observe the watcher's eventual catalog
/// state (a newly loaded bundle accepting Hello, or an unloaded one rejecting).
fn poll_hello_first_frame(
    state_root: &Path,
    principal_id: &str,
    identity_token: &str,
    accept: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + WATCHER_SIGNAL_WAIT_BUDGET;
    loop {
        let frame = relay_hello_first_frame(state_root, principal_id, identity_token);
        if accept(&frame) || Instant::now() >= deadline {
            return frame;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Polls the relay inscriptions log until an entry satisfies `matches`,
/// returning `true` on a match or `false` once the deadline passes. Used to
/// observe a watcher outcome that emits no stream frame (so it cannot be awaited
/// on a keepalive connection).
fn poll_inscriptions(inscriptions_root: &Path, matches: impl Fn(&Value) -> bool) -> bool {
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
fn poll_inscription_event(inscriptions_root: &Path, event: &str, bundle_name: &str) -> bool {
    poll_inscriptions(inscriptions_root, |entry| {
        entry["event"] == event && entry["details"]["bundle_name"] == bundle_name
    })
}

/// Polls for an entry with `event`, whatever it names.
///
/// The bundle-scoped variant cannot serve a reconciliation suppressed by an
/// unreadable layer: enumeration never got far enough to know which bundles were
/// involved, so the inscription names the layer rather than a bundle.
fn poll_inscription_event_kind(inscriptions_root: &Path, event: &str) -> bool {
    poll_inscriptions(inscriptions_root, |entry| entry["event"] == event)
}

// A bundle TOML file added at runtime is picked up by the watcher: the relay
// loads and starts the bundle without a restart, and a new connection to that
// bundle succeeds where it was previously rejected as an unknown bundle.
#[test]
fn host_relay_watcher_loads_new_bundle_file_at_runtime() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a"],
        Some(true),
    );
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .args([
                "host",
                "relay",
                "--configuration-directory",
                &config_root.to_string_lossy(),
                "--state-directory",
                &state_root.to_string_lossy(),
                "--inscriptions-directory",
                &inscriptions_root.to_string_lossy(),
            ])
            .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay"),
    );
    wait_for_relay_ready(&state_root, "alpha");
    assert_bundle_watch_started(&inscriptions_root);

    // The bravo bundle does not exist yet: Hello is rejected as unknown.
    let before = relay_hello_first_frame(&state_root, "b@bravo", "socket-trust");
    // Add the bravo bundle file at runtime; the watcher should load it.
    write_bundle_configuration_with_options(
        &config_root,
        "bravo",
        Some(&["dev"]),
        &["b"],
        Some(true),
    );
    let added = poll_hello_first_frame(&state_root, "b@bravo", "socket-trust", |frame| {
        frame["frame"] == "hello_ack"
    });

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert_eq!(
        before["frame"], "response",
        "expected error frame: {before:?}"
    );
    assert_eq!(
        before["response"]["error"]["code"],
        "validation_unknown_bundle"
    );
    assert_eq!(
        added["frame"],
        "hello_ack",
        "expected hello_ack after add: {added:?}; relay inscriptions: {}",
        fs::read_to_string(inscriptions_root.join("relay.log")).unwrap_or_default()
    );
    assert_eq!(added["principal_id"], "b@bravo");
}

// A bundle TOML file added at runtime with `autostart = false` is loaded held:
// the relay learns it (its members register as offline shells, so Hello is
// accepted) but does not start it, mirroring the boot-time process-only path.
#[test]
fn host_relay_watcher_loads_non_autostart_bundle_held_without_starting() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a"],
        Some(true),
    );
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .args([
                "host",
                "relay",
                "--configuration-directory",
                &config_root.to_string_lossy(),
                "--state-directory",
                &state_root.to_string_lossy(),
                "--inscriptions-directory",
                &inscriptions_root.to_string_lossy(),
            ])
            .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay"),
    );
    wait_for_relay_ready(&state_root, "alpha");
    assert_bundle_watch_started(&inscriptions_root);

    // Add a non-autostart bravo bundle at runtime; the watcher should register it
    // held rather than start it.
    write_bundle_configuration_with_options(
        &config_root,
        "bravo",
        Some(&["dev"]),
        &["b"],
        Some(false),
    );
    let loaded_held =
        poll_inscription_event(&inscriptions_root, "relay.bundle.loaded_held", "bravo");
    // The held bundle is nonetheless known: Hello to one of its members succeeds.
    let known = poll_hello_first_frame(&state_root, "b@bravo", "socket-trust", |frame| {
        frame["frame"] == "hello_ack"
    });

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert!(
        loaded_held,
        "expected relay.bundle.loaded_held for the non-autostart runtime add; relay inscriptions: {}",
        fs::read_to_string(inscriptions_root.join("relay.log")).unwrap_or_default()
    );
    assert_eq!(
        known["frame"], "hello_ack",
        "held bundle should still be known: {known:?}"
    );
    // It must not have been started: no `relay.bundle.loaded` for bravo.
    let inscriptions = fs::read_to_string(inscriptions_root.join("relay.log"))
        .expect("read relay inscriptions log");
    let started = inscriptions
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|entry| {
            entry["event"] == "relay.bundle.loaded" && entry["details"]["bundle_name"] == "bravo"
        });
    assert!(
        !started,
        "non-autostart bundle must not be started on runtime add: {inscriptions}"
    );
}

// A bundle TOML file removed at runtime unloads the bundle: active sessions
// receive a `runtime_bundle_unloaded` error frame before disconnect, and
// subsequent connection attempts to that bundle are rejected as unknown.
#[test]
fn host_relay_watcher_unloads_removed_bundle_file_at_runtime() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a"],
        Some(true),
    );
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .args([
                "host",
                "relay",
                "--configuration-directory",
                &config_root.to_string_lossy(),
                "--state-directory",
                &state_root.to_string_lossy(),
                "--inscriptions-directory",
                &inscriptions_root.to_string_lossy(),
            ])
            .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay"),
    );
    wait_for_relay_ready(&state_root, "alpha");
    assert_bundle_watch_started(&inscriptions_root);

    let (_stream, mut reader, hello) =
        relay_hello_keepalive(&state_root, "a@alpha", "socket-trust");
    fs::remove_file(config_root.join("bundles").join("alpha.toml")).expect("remove bundle file");
    // The active session's connection receives the typed unload frame.
    let eviction = expect_watcher_signal(
        read_next_frame(&mut reader),
        "unload eviction frame",
        &inscriptions_root,
    );
    // The catalog is updated before the eviction frame is written, so a new
    // Hello to the unloaded bundle is now rejected as unknown.
    let after = relay_hello_first_frame(&state_root, "a@alpha", "socket-trust");

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert_eq!(hello["frame"], "hello_ack", "expected hello_ack: {hello:?}");
    assert_eq!(
        eviction["frame"], "response",
        "expected eviction frame: {eviction:?}"
    );
    assert_eq!(
        eviction["response"]["error"]["code"],
        "runtime_bundle_unloaded"
    );
    assert_eq!(
        after["frame"], "response",
        "expected unknown-bundle frame: {after:?}"
    );
    assert_eq!(
        after["response"]["error"]["code"],
        "validation_unknown_bundle"
    );
}

// A layer that becomes unreadable at runtime holds the last successful
// reconciliation rather than tearing the catalog down. Enumeration is ground
// truth for the unload pass, so a layer that cannot be enumerated reads as every
// bundle in it having been deleted — the relay would evict live sessions and
// unload a running bundle over a permission bit, and the only trace would be an
// ordinary unload. Retention is what makes the difference observable: the bundle
// stays connectable and the suppression is inscribed.
#[test]
fn host_relay_watcher_retains_the_catalog_when_a_layer_becomes_unreadable() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a"],
        Some(true),
    );
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .args([
                "host",
                "relay",
                "--configuration-directory",
                &config_root.to_string_lossy(),
                "--state-directory",
                &state_root.to_string_lossy(),
                "--inscriptions-directory",
                &inscriptions_root.to_string_lossy(),
            ])
            .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay"),
    );
    wait_for_relay_ready(&state_root, "alpha");
    assert_bundle_watch_started(&inscriptions_root);

    // Denying access to the watched bundles directory is itself a filesystem
    // event, so it drives the reconcile pass under test rather than merely
    // arranging the state for one.
    let bundles = config_root.join("bundles");
    let Some(restore) = deny_directory_access(&bundles) else {
        report_permission_fixture_skip(
            "host_relay_watcher_retains_the_catalog_when_a_layer_becomes_unreadable",
        );
        shutdown_relay_if_present(&state_root, "alpha");
        let _ = child.wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT);
        return;
    };

    let suppressed = poll_inscription_event_kind(
        &inscriptions_root,
        "relay.bundle.reconcile_suppressed_unreadable_layer",
    );
    // Admission reads the bundle file, which lives inside the directory just
    // denied, so this connection cannot be admitted either way. Its *error code*
    // is what separates the two worlds: a retained catalog still knows the
    // bundle and fails on the layer it cannot read, while a torn-down one would
    // have forgotten it and answered `validation_unknown_bundle` — the same
    // answer it gives for a bundle the operator deleted.
    let during = relay_hello_first_frame(&state_root, "a@alpha", "socket-trust");
    // Restored before shutdown so the relay's own teardown is not fighting a
    // directory it cannot traverse.
    drop(restore);
    // Readable again, and serving without an intervening reload: the runtime
    // that answers here is the one that was running before the layer went dark.
    let after = poll_hello_first_frame(&state_root, "a@alpha", "socket-trust", |frame| {
        frame["frame"] == "hello_ack"
    });

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    let inscriptions = fs::read_to_string(inscriptions_root.join("relay.log"))
        .expect("read relay inscriptions log");
    assert!(
        suppressed,
        "expected relay.bundle.reconcile_suppressed_unreadable_layer; relay inscriptions: {inscriptions}"
    );
    assert_eq!(
        during["response"]["error"]["code"], "validation_unreadable_configuration_layer",
        "a retained bundle must fail on the unreadable layer rather than be forgotten: \
         {during:?}; relay inscriptions: {inscriptions}"
    );
    assert_eq!(
        after["frame"], "hello_ack",
        "the bundle must still be served once the layer is readable again: {after:?}; \
         relay inscriptions: {inscriptions}"
    );
    let disturbed: Vec<String> = inscriptions
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| entry["details"]["bundle_name"] == "alpha")
        .filter_map(|entry| {
            entry["event"]
                .as_str()
                .filter(|event| {
                    matches!(
                        *event,
                        "relay.bundle.unloaded" | "relay.bundle.reloaded" | "relay.bundle.loaded"
                    )
                })
                .map(str::to_string)
        })
        .collect();
    assert!(
        disturbed.is_empty(),
        "an unreadable layer must disturb neither the catalog nor the runtime, saw {disturbed:?}: \
         {inscriptions}"
    );
}

// A bundle TOML file modified at runtime is treated as a full teardown +
// reload: active sessions receive a `runtime_bundle_reloaded` error frame before
// disconnect, and the relay reloads the bundle (a fresh connection succeeds).
#[test]
fn host_relay_watcher_reloads_modified_bundle_file_at_runtime() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a"],
        Some(true),
    );
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .args([
                "host",
                "relay",
                "--configuration-directory",
                &config_root.to_string_lossy(),
                "--state-directory",
                &state_root.to_string_lossy(),
                "--inscriptions-directory",
                &inscriptions_root.to_string_lossy(),
            ])
            .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay"),
    );
    wait_for_relay_ready(&state_root, "alpha");
    assert_bundle_watch_started(&inscriptions_root);

    let (_stream, mut reader, hello) =
        relay_hello_keepalive(&state_root, "a@alpha", "socket-trust");
    // Modify the alpha bundle file (add a session): the watcher reloads it.
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a", "c"],
        Some(true),
    );
    let eviction = expect_watcher_signal(
        read_next_frame(&mut reader),
        "reload eviction frame",
        &inscriptions_root,
    );
    // After reload the bundle is still loaded: a fresh Hello succeeds.
    let after = poll_hello_first_frame(&state_root, "a@alpha", "socket-trust", |frame| {
        frame["frame"] == "hello_ack"
    });

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert_eq!(hello["frame"], "hello_ack", "expected hello_ack: {hello:?}");
    assert_eq!(
        eviction["frame"], "response",
        "expected eviction frame: {eviction:?}"
    );
    assert_eq!(
        eviction["response"]["error"]["code"],
        "runtime_bundle_reloaded"
    );
    assert_eq!(
        after["frame"],
        "hello_ack",
        "expected hello_ack after reload: {after:?}; relay inscriptions: {}",
        fs::read_to_string(inscriptions_root.join("relay.log")).unwrap_or_default()
    );
    assert_eq!(after["principal_id"], "a@alpha");
}

// A definition appearing in an earlier layer over a byte-identical one in a
// later layer — and later being removed to reveal it again — changes which file
// supplies the identifier without changing a single content byte. Both
// transitions must reload, since the relay now tracks a different file for edits
// and deletions.
#[test]
fn host_relay_watcher_reloads_when_a_byte_identical_earlier_layer_appears_and_is_removed() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a"],
        Some(true),
    );
    // A sibling layer, watched from startup so its bundles directory appearing
    // later is observed. The bundle file itself arrives mid-test.
    let override_layer = temporary.path().join("override");
    let override_bundles = override_layer.join("bundles");
    fs::create_dir_all(&override_bundles).expect("create override bundles");
    let base_bundle = config_root.join("bundles/alpha.toml");
    let override_bundle = override_bundles.join("alpha.toml");
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .args([
                "host",
                "relay",
                "--configuration-directory",
                &override_layer.to_string_lossy(),
                "--configuration-directory",
                &config_root.to_string_lossy(),
                "--state-directory",
                &state_root.to_string_lossy(),
                "--inscriptions-directory",
                &inscriptions_root.to_string_lossy(),
            ])
            .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay"),
    );
    wait_for_relay_ready(&state_root, "alpha");
    assert_bundle_watch_started(&inscriptions_root);

    let (_appearance_stream, mut appearance_reader, appearance_hello) =
        relay_hello_keepalive(&state_root, "a@alpha", "socket-trust");
    fs::copy(&base_bundle, &override_bundle).expect("copy base bundle into the earlier layer");
    let appearance_eviction = expect_watcher_signal(
        read_next_frame(&mut appearance_reader),
        "earlier-layer appearance reload eviction frame",
        &inscriptions_root,
    );
    let after_appearance =
        poll_hello_first_frame(&state_root, "a@alpha", "socket-trust", |frame| {
            frame["frame"] == "hello_ack"
        });

    let (_removal_stream, mut removal_reader, removal_hello) =
        relay_hello_keepalive(&state_root, "a@alpha", "socket-trust");
    fs::remove_file(&override_bundle).expect("remove the earlier-layer bundle");
    let removal_eviction = expect_watcher_signal(
        read_next_frame(&mut removal_reader),
        "earlier-layer removal reload eviction frame",
        &inscriptions_root,
    );
    let after_removal = poll_hello_first_frame(&state_root, "a@alpha", "socket-trust", |frame| {
        frame["frame"] == "hello_ack"
    });

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert_eq!(appearance_hello["frame"], "hello_ack");
    assert_eq!(
        appearance_eviction["response"]["error"]["code"], "runtime_bundle_reloaded",
        "an earlier layer appearing over identical base content must reload: \
         {appearance_eviction:?}"
    );
    assert_eq!(after_appearance["frame"], "hello_ack");
    assert_eq!(removal_hello["frame"], "hello_ack");
    assert_eq!(
        removal_eviction["response"]["error"]["code"], "runtime_bundle_reloaded",
        "removing an earlier layer to reveal identical base content must reload: \
         {removal_eviction:?}"
    );
    assert_eq!(after_removal["frame"], "hello_ack");
}

// A bundle the operator explicitly took down via `down` is not silently brought
// back up when its configuration file is edited: the watcher absorbs the new
// fingerprint and records `relay.bundle.reload_suppressed_downed` instead of
// reloading (and restarting) the runtime.
#[test]
fn host_relay_watcher_preserves_down_intent_across_file_edit() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a"],
        Some(true),
    );
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .args([
                "host",
                "relay",
                "--configuration-directory",
                &config_root.to_string_lossy(),
                "--state-directory",
                &state_root.to_string_lossy(),
                "--inscriptions-directory",
                &inscriptions_root.to_string_lossy(),
            ])
            .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay"),
    );
    wait_for_relay_ready(&state_root, "alpha");
    assert_bundle_watch_started(&inscriptions_root);

    // Operator takes the bundle down. This flows over the live relay socket and
    // records the down intent on the shared catalog the watcher observes.
    let down = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "down",
            "alpha",
            "--configuration-directory",
            &config_root.to_string_lossy(),
            "--state-directory",
            &state_root.to_string_lossy(),
            "--inscriptions-directory",
            &inscriptions_root.to_string_lossy(),
        ])
        .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
        .output()
        .expect("run agentmux down alpha");
    assert!(down.status.success(), "down should succeed: {down:?}");

    // Edit the (downed) bundle file: the watcher must absorb the change without
    // restarting the runtime.
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a", "c"],
        Some(true),
    );
    let suppressed = poll_inscription_event(
        &inscriptions_root,
        "relay.bundle.reload_suppressed_held",
        "alpha",
    );

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert!(
        suppressed,
        "expected relay.bundle.reload_suppressed_held for the downed bundle; relay inscriptions: {}",
        fs::read_to_string(inscriptions_root.join("relay.log")).unwrap_or_default()
    );
    // The suppressed edit must not have reloaded (and thereby restarted) the
    // bundle the operator deliberately took down.
    let inscriptions = fs::read_to_string(inscriptions_root.join("relay.log"))
        .expect("read relay inscriptions log");
    let reloaded = inscriptions
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|entry| {
            entry["event"] == "relay.bundle.reloaded" && entry["details"]["bundle_name"] == "alpha"
        });
    assert!(
        !reloaded,
        "downed bundle must not be reloaded on a file edit: {inscriptions}"
    );
}

// A bundle configured `autostart = false` enters the catalog already held, so a
// configuration edit must not start it: the watcher records
// `relay.bundle.reload_suppressed_held` and does not reload. This is the static
// (declared) counterpart to an explicit `down` — the same hold rule, sourced
// from config rather than a runtime command, with no `down` issued.
#[test]
fn host_relay_watcher_holds_non_autostart_bundle_on_file_edit() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a"],
        Some(false),
    );
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .args([
                "host",
                "relay",
                "--configuration-directory",
                &config_root.to_string_lossy(),
                "--state-directory",
                &state_root.to_string_lossy(),
                "--inscriptions-directory",
                &inscriptions_root.to_string_lossy(),
            ])
            .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay"),
    );
    wait_for_relay_ready(&state_root, "alpha");
    assert_bundle_watch_started(&inscriptions_root);

    // Edit the never-started bundle file: the standing `autostart = false` hold
    // must keep the watcher from bringing it up.
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a", "c"],
        Some(false),
    );
    let suppressed = poll_inscription_event(
        &inscriptions_root,
        "relay.bundle.reload_suppressed_held",
        "alpha",
    );

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert!(
        suppressed,
        "expected relay.bundle.reload_suppressed_held for the non-autostart bundle; relay inscriptions: {}",
        fs::read_to_string(inscriptions_root.join("relay.log")).unwrap_or_default()
    );
    let inscriptions = fs::read_to_string(inscriptions_root.join("relay.log"))
        .expect("read relay inscriptions log");
    let reloaded = inscriptions
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|entry| {
            entry["event"] == "relay.bundle.reloaded" && entry["details"]["bundle_name"] == "alpha"
        });
    assert!(
        !reloaded,
        "non-autostart bundle must not be reloaded on a file edit: {inscriptions}"
    );
}

// With `--no-watch`, a bundle file added at runtime is not reconciled: the relay
// continues to reject connections to the new bundle until a restart.
#[test]
fn host_relay_no_watch_flag_disables_runtime_reconcile() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a"],
        Some(true),
    );
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .args([
                "host",
                "relay",
                "--no-watch",
                "--configuration-directory",
                &config_root.to_string_lossy(),
                "--state-directory",
                &state_root.to_string_lossy(),
                "--inscriptions-directory",
                &inscriptions_root.to_string_lossy(),
            ])
            .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay --no-watch"),
    );
    wait_for_relay_ready(&state_root, "alpha");

    write_bundle_configuration_with_options(
        &config_root,
        "bravo",
        Some(&["dev"]),
        &["b"],
        Some(true),
    );
    // Give a watcher (if one existed) ample time to reconcile, then confirm the
    // bundle is still unknown: no reconciliation happened.
    thread::sleep(WATCHER_RECONCILE_BUDGET);
    let frame = relay_hello_first_frame(&state_root, "b@bravo", "socket-trust");

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert_eq!(
        frame["frame"], "response",
        "expected error frame: {frame:?}"
    );
    assert_eq!(
        frame["response"]["error"]["code"],
        "validation_unknown_bundle"
    );
}

// relay.toml `watch-bundles = false` disables the watcher without the `--no-watch`
// CLI flag: a bundle added at runtime is not reconciled, mirroring the CLI-flag
// behavior but sourced from configuration.
#[test]
fn host_relay_watch_bundles_false_from_relay_toml_disables_reconcile() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(
        &config_root,
        "alpha",
        Some(&["dev"]),
        &["a"],
        Some(true),
    );
    fs::write(config_root.join("relay.toml"), "watch-bundles = false\n").expect("write relay.toml");
    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .args([
                "host",
                "relay",
                "--configuration-directory",
                &config_root.to_string_lossy(),
                "--state-directory",
                &state_root.to_string_lossy(),
                "--inscriptions-directory",
                &inscriptions_root.to_string_lossy(),
            ])
            .env("AGENTMUX_TMUX_COMMAND", &fake_tmux)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay with relay.toml watch-bundles=false"),
    );
    wait_for_relay_ready(&state_root, "alpha");

    write_bundle_configuration_with_options(
        &config_root,
        "bravo",
        Some(&["dev"]),
        &["b"],
        Some(true),
    );
    // Give a watcher (if one existed) ample time to reconcile, then confirm the
    // bundle is still unknown: no reconciliation happened.
    thread::sleep(WATCHER_RECONCILE_BUDGET);
    let frame = relay_hello_first_frame(&state_root, "b@bravo", "socket-trust");

    shutdown_relay_if_present(&state_root, "alpha");
    let output = child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .expect("wait for agentmux host relay");
    assert!(output.status.success(), "command should succeed");

    assert_eq!(
        frame["frame"], "response",
        "expected error frame: {frame:?}"
    );
    assert_eq!(
        frame["response"]["error"]["code"],
        "validation_unknown_bundle"
    );
}
