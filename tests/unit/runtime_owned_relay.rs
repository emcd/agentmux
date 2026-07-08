//! Coverage for TUI-owned relay teardown.
//!
//! Exercises `SpawnedRelay`'s graceful stop and the spawned-vs-already-running
//! ownership branch that `agentmux tui` relies on, all through the public
//! `runtime::bootstrap` surface. No real relay binary is spawned: a long-lived
//! `sleep` child stands in for the relay process, and a bound `UnixListener`
//! stands in for a reachable relay socket.

use std::cell::RefCell;
use std::os::unix::net::UnixListener;
use std::process::{Child, Command};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use agentmux::runtime::bootstrap::{BootstrapOptions, SpawnedRelay, bootstrap_relay};
use agentmux::runtime::paths::{RelayRuntimePaths, ensure_relay_runtime_directory};
use tempfile::TempDir;

fn spawn_benign_child(seconds: &str) -> Child {
    Command::new("sleep")
        .arg(seconds)
        .spawn()
        .expect("spawn sleep child")
}

fn test_paths() -> (TempDir, RelayRuntimePaths) {
    let temporary = TempDir::new().expect("temporary");
    let paths = RelayRuntimePaths::resolve(temporary.path());
    ensure_relay_runtime_directory(&paths).expect("directory");
    (temporary, paths)
}

#[test]
fn spawned_relay_stop_terminates_live_child_within_grace() {
    // A long-lived child stands in for the relay process. `stop` sends SIGTERM,
    // whose default disposition terminates `sleep`, so stop must return well
    // inside the grace window rather than waiting it out.
    let relay = SpawnedRelay::new(spawn_benign_child("60"));
    let grace = Duration::from_secs(5);
    let started = Instant::now();
    relay.stop(grace);
    let elapsed = started.elapsed();
    assert!(
        elapsed < grace,
        "stop should terminate the child, not wait out the grace window (elapsed {elapsed:?})",
    );
}

#[test]
fn spawned_relay_stop_returns_when_child_already_exited() {
    // The child exits before `stop` runs; `try_wait` observes the exit and stop
    // returns promptly without blocking on the grace window.
    let child = spawn_benign_child("0");
    // Give the child time to exit and become reapable.
    thread::sleep(Duration::from_millis(200));
    let relay = SpawnedRelay::new(child);
    let grace = Duration::from_secs(5);
    let started = Instant::now();
    relay.stop(grace);
    assert!(
        started.elapsed() < grace,
        "stop should return promptly for an already-exited child",
    );
}

#[test]
fn spawned_relay_reports_child_pid() {
    let child = spawn_benign_child("60");
    let expected = child.id();
    let relay = SpawnedRelay::new(child);
    assert_eq!(relay.pid(), expected);
    relay.stop(Duration::from_secs(5));
}

#[test]
fn bootstrap_spawn_yields_owned_relay_for_teardown() {
    // Mirrors `ensure_tui_relay_available`: no relay is reachable, so the spawn
    // closure runs, captures the spawned child, and the true `spawned_relay`
    // flag maps to an owned `SpawnedRelay`.
    let (_temporary, paths) = test_paths();
    let owned: Rc<RefCell<Option<SpawnedRelay>>> = Rc::new(RefCell::new(None));
    let owned_inner = Rc::clone(&owned);
    let listener_handle = Arc::new(Mutex::new(None));
    let listener_handle_inner = Arc::clone(&listener_handle);
    let options = BootstrapOptions {
        auto_start_relay: true,
        startup_timeout: Duration::from_secs(1),
    };
    let report = bootstrap_relay(&paths, options, || {
        // Satisfy the readiness gate the way a real relay would: bind the socket
        // and publish the ready sentinel. Held open briefly past the readiness
        // poll, then dropped.
        let relay_socket = paths.relay_socket.clone();
        let ready_sentinel = paths.relay_ready_sentinel.clone();
        let handle = thread::spawn(move || {
            let listener = UnixListener::bind(&relay_socket).expect("bind");
            std::fs::write(&ready_sentinel, b"").expect("write ready sentinel");
            thread::sleep(Duration::from_millis(250));
            drop(listener);
        });
        *listener_handle_inner.lock().expect("mutex") = Some(handle);
        // Stand-in for the relay process the TUI would own for teardown.
        *owned_inner.borrow_mut() = Some(SpawnedRelay::new(spawn_benign_child("60")));
        Ok(())
    })
    .expect("bootstrap");

    let owned_relay = if report.spawned_relay {
        owned.borrow_mut().take()
    } else {
        None
    };
    assert!(report.spawned_relay);
    let owned_relay = owned_relay.expect("a spawned relay is owned for teardown");
    owned_relay.stop(Duration::from_secs(5));

    if let Some(handle) = listener_handle.lock().expect("mutex").take() {
        handle.join().expect("listener thread");
    }
}

#[test]
fn bootstrap_readiness_failure_after_spawn_leaves_child_for_cleanup() {
    // Mirrors the error path in `ensure_tui_relay_available`: the spawn closure
    // launches the relay child, but readiness never arrives (the closure never
    // binds the socket nor writes the ready sentinel), so `bootstrap_relay`
    // returns a startup-timeout error *after* the child is live. The captured
    // child must remain owned so the caller can stop it — dropping it would
    // detach, not kill, orphaning the relay this change exists to own.
    let (_temporary, paths) = test_paths();
    let owned: Rc<RefCell<Option<SpawnedRelay>>> = Rc::new(RefCell::new(None));
    let owned_inner = Rc::clone(&owned);
    let options = BootstrapOptions {
        auto_start_relay: true,
        startup_timeout: Duration::from_millis(200),
    };
    let outcome = bootstrap_relay(&paths, options, || {
        // Launch the stand-in relay but never satisfy the readiness gate.
        *owned_inner.borrow_mut() = Some(SpawnedRelay::new(spawn_benign_child("60")));
        Ok(())
    });

    assert!(
        outcome.is_err(),
        "readiness must time out when the relay never becomes ready",
    );
    // The caller still owns the spawned child on the error path and can tear it
    // down within grace; without ownership the relay would be orphaned.
    let owned_relay = owned
        .borrow_mut()
        .take()
        .expect("spawned child stays owned for error-path cleanup");
    let grace = Duration::from_secs(5);
    let started = Instant::now();
    owned_relay.stop(grace);
    assert!(
        started.elapsed() < grace,
        "stop should terminate the orphan-candidate child within grace",
    );
}

#[test]
fn bootstrap_reuse_of_running_relay_yields_no_ownership() {
    // A relay is already reachable: the spawn closure must not run, the
    // `spawned_relay` flag is false, and nothing is owned for teardown.
    let (_temporary, paths) = test_paths();
    let _listener = UnixListener::bind(&paths.relay_socket).expect("bind existing relay");
    let owned: Rc<RefCell<Option<SpawnedRelay>>> = Rc::new(RefCell::new(None));
    let owned_inner = Rc::clone(&owned);
    let report = bootstrap_relay(&paths, BootstrapOptions::default(), || {
        // Would-be capture; must never run while a relay is already reachable.
        *owned_inner.borrow_mut() = Some(SpawnedRelay::new(spawn_benign_child("60")));
        Ok(())
    })
    .expect("bootstrap");

    let owned_relay = if report.spawned_relay {
        owned.borrow_mut().take()
    } else {
        None
    };
    assert!(!report.spawned_relay);
    assert!(
        owned_relay.is_none(),
        "an already-running relay must not be owned for teardown",
    );
}
