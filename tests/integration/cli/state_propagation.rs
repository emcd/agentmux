//! What a spawned member actually receives from the relay that started it.
//!
//! These drive `agentmux up` against a fake tmux and read the argument vector
//! tmux was handed. Asserting on the recorded invocation rather than on
//! configuration is the point: the defect these guard against is a child
//! resolving somewhere the relay never bound, which is only visible at the
//! spawn.

use std::{
    fs,
    process::{Command, Stdio},
};

use tempfile::TempDir;

use super::super::support::process;
use super::helpers::*;

/// A state root the member declares for itself, which the relay must overwrite.
const MEMBER_DECLARED_STATE_ROOT: &str = "/nowhere/member-declared";

/// Appends a session-level `AGENTMUX_STATE_DIRECTORY` to a written bundle file,
/// so the spawn has an operator-declared value to contend with.
fn declare_member_state_directory(config_root: &std::path::Path, bundle_name: &str, value: &str) {
    let path = config_root
        .join("bundles")
        .join(format!("{bundle_name}.toml"));
    let mut bundle = fs::read_to_string(&path).expect("read bundle configuration");
    bundle.push_str(&format!(
        "\n[[sessions.environment]]\nname = \"AGENTMUX_STATE_DIRECTORY\"\nvalue = \"{value}\"\n"
    ));
    fs::write(&path, bundle).expect("write bundle configuration");
}

/// Returns the single recorded `new-session` invocation.
fn recorded_new_session(log: &str) -> &str {
    let mut lines = log
        .lines()
        .filter(|line| line.contains("new-session"))
        .collect::<Vec<_>>();
    assert_eq!(
        lines.len(),
        1,
        "expected exactly one new-session invocation, got:\n{log}"
    );
    lines.pop().expect("one new-session line")
}

#[test]
fn a_spawned_member_receives_the_relays_state_root_over_its_own_declaration() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("named-state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));
    declare_member_state_directory(&config_root, "alpha", MEMBER_DECLARED_STATE_ROOT);

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let host_child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--no-autostart",
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
        .expect("spawn agentmux host relay --no-autostart");
    wait_for_relay_ready(&state_root, "alpha");

    let up = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "up",
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
        .expect("run agentmux up");
    assert!(up.status.success(), "up should succeed");

    let log = fs::read_to_string(fake_tmux_log_path(&fake_tmux)).expect("read fake tmux log");
    let new_session = recorded_new_session(&log);

    assert!(
        new_session.contains(&format!(
            "-e AGENTMUX_STATE_DIRECTORY={}",
            state_root.display()
        )),
        "the spawn must carry the relay's state root; got:\n{new_session}"
    );
    assert!(
        !new_session.contains(MEMBER_DECLARED_STATE_ROOT),
        "a member-declared state root must be overwritten, not preserved; got:\n{new_session}"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    process::wait_with_output_bounded(host_child, process::HARNESS_CHILD_WAIT_DEFAULT).ok();
}

/// Builds a state root deep enough that `<state_root>/bundles/alpha/tmux.sock`
/// overshoots `sun_path`, rather than merely approaching it — a fixture near
/// the boundary passes whether or not the fix is present.
fn deep_state_root(base: &std::path::Path) -> std::path::PathBuf {
    /// Usable bytes in `sockaddr_un.sun_path` on Linux.
    const UNIX_SOCKET_PATH_MAXIMUM: usize = 107;
    /// Clears the limit by a wide margin instead of sitting on it.
    const OVERSHOOT: usize = 60;

    let mut root = base.to_path_buf();
    while root.join("bundles/alpha/tmux.sock").as_os_str().len()
        <= UNIX_SOCKET_PATH_MAXIMUM + OVERSHOOT
    {
        root = root.join("deeply-nested-state-directory");
    }
    assert!(
        root.join("relay.sock").as_os_str().len() > UNIX_SOCKET_PATH_MAXIMUM,
        "the fixture must overshoot the limit for the relay socket too"
    );
    root
}

#[test]
fn a_relay_comes_up_under_a_state_root_longer_than_sun_path() {
    // Normalizing the state root to an absolute path removed the relative-path
    // escape hatch deep hierarchies relied on, so binding has to stop scaling
    // with depth. Driving a real relay bring-up is the only way to see that:
    // the relay binds its socket, publishes the ready sentinel, and answers a
    // client that connects to the same long path.
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = deep_state_root(temporary.path());
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let host_child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--no-autostart",
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
        .expect("spawn agentmux host relay --no-autostart");
    wait_for_relay_ready(&state_root, "alpha");

    // Reaching the relay is the assertion: `up` is a client that connects to
    // the same over-long socket path the relay bound.
    let up = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "up",
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
        .expect("run agentmux up");
    assert!(
        up.status.success(),
        "up should reach a relay under a deep state root; stderr:\n{}",
        String::from_utf8_lossy(&up.stderr)
    );

    let log = fs::read_to_string(fake_tmux_log_path(&fake_tmux)).expect("read fake tmux log");
    let new_session = recorded_new_session(&log);
    assert!(
        new_session.starts_with("-S tmux.sock "),
        "tmux must still be addressed by the bare socket name; got:\n{new_session}"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    process::wait_with_output_bounded(host_child, process::HARNESS_CHILD_WAIT_DEFAULT).ok();
}

#[test]
fn tmux_is_addressed_relative_to_its_own_socket_directory() {
    // The socket address must not scale with state-root depth: tmux binds the
    // `-S` path itself, and `<state_root>/bundles/<bundle>/tmux.sock` is the
    // longest path this project constructs. Asserting on the recorded
    // invocation is what makes this a claim about what tmux received rather
    // than about an intermediate value.
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("named-state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));

    let fake_tmux = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let host_child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "host",
            "relay",
            "--no-autostart",
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
        .expect("spawn agentmux host relay --no-autostart");
    wait_for_relay_ready(&state_root, "alpha");

    let up = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args([
            "up",
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
        .expect("run agentmux up");
    assert!(up.status.success(), "up should succeed");

    let log = fs::read_to_string(fake_tmux_log_path(&fake_tmux)).expect("read fake tmux log");
    for invocation in log.lines().filter(|line| !line.trim().is_empty()) {
        assert!(
            invocation.starts_with("-S tmux.sock "),
            "every tmux invocation must address the bare socket name; got:\n{invocation}"
        );
    }

    // And the start directory stays explicit, because an omitted `-c` would
    // take the client's working directory — now the bundle runtime directory
    // rather than wherever the relay was launched.
    let new_session = recorded_new_session(&log);
    assert!(
        new_session.contains("-c /tmp"),
        "new-session must pass the member's declared directory; got:\n{new_session}"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    process::wait_with_output_bounded(host_child, process::HARNESS_CHILD_WAIT_DEFAULT).ok();
}
