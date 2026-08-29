use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
};

use tempfile::TempDir;

use super::super::super::support::process;
use super::super::helpers::*;
use super::helpers::*;

/// Builds a state root deep enough that `<state_root>/bundles/alpha/tmux.sock`
/// overshoots `sun_path`, rather than merely approaching it — a fixture near
/// the boundary passes whether or not the fix is present.
///
/// Gated with its caller: on a non-Linux target the deep-root bring-up is not
/// expected to succeed and the test is absent, which would leave this dead under
/// `clippy --all-targets -D warnings`.
#[cfg(target_os = "linux")]
fn deep_state_root(base: &std::path::Path) -> std::path::PathBuf {
    // The crate's own constant rather than a literal: the limit is 107 on Linux
    // and 103 on Darwin, and a fixture hardcoding one of them would overshoot by
    // a different margin on the other.
    use agentmux::runtime::sockets::UNIX_SOCKET_PATH_MAXIMUM;

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

// Linux only, and that is the behavior rather than a test-environment excuse.
// Shortening the address depends on `/proc/self/fd`, so on Darwin the full path
// is used and a root this deep is genuinely unreachable. The non-Linux
// expectation — a structured refusal naming the limit — is asserted directly
// against `runtime::sockets` in `tests/unit/runtime_sockets.rs`, which is where
// it can be stated without standing up a relay that cannot come up.
#[cfg(target_os = "linux")]
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

    let host_child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
            .expect("spawn agentmux host relay --no-autostart"),
    );
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
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
}

#[test]
fn a_relative_tmux_wrapper_still_resolves_against_the_launch_directory() {
    // Running the client from the socket's directory changed what a relative
    // program path means: the kernel resolves a value containing a separator
    // against the *child's* working directory, so `./fake-tmux.sh` would be
    // looked for under the bundle runtime directory. Every other test here
    // passes an absolute wrapper and cannot see it.
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("named-state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));

    let wrapper_directory = temporary.path().join("wrappers");
    fs::create_dir_all(&wrapper_directory).expect("create wrapper directory");
    let fake_tmux = wrapper_directory.join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    // Relative, with a separator, interpreted against the relay's own working
    // directory — which is the wrapper's parent, not the bundle runtime.
    let relative_wrapper = std::path::PathBuf::from("wrappers/fake-tmux.sh");

    let host_child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .current_dir(temporary.path())
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
            .env("AGENTMUX_TMUX_COMMAND", &relative_wrapper)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay --no-autostart"),
    );
    wait_for_relay_ready(&state_root, "alpha");

    let up = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .current_dir(temporary.path())
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
        .env("AGENTMUX_TMUX_COMMAND", &relative_wrapper)
        .output()
        .expect("run agentmux up");
    assert!(
        up.status.success(),
        "a relative wrapper must still be found; stderr:\n{}",
        String::from_utf8_lossy(&up.stderr)
    );

    let log = fs::read_to_string(fake_tmux_log_path(&fake_tmux))
        .expect("the relative wrapper must have run and recorded its invocations");
    assert!(
        log.contains("new-session"),
        "the wrapper must have been reached for session creation; got:\n{log}"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
}

#[test]
fn a_bare_tmux_command_resolves_through_the_launch_directorys_path() {
    // The companion to the relative-wrapper case, for the other kind of relative
    // reference. A bare name carries no separator and goes through `PATH`, but a
    // `PATH` *entry* may itself be relative, so it moves with the working
    // directory just as a `./wrapper.sh` would. Configuring an absolute wrapper —
    // what every other test here does — cannot see it.
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("named-state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));

    let wrapper_directory = temporary.path().join("wrappers");
    fs::create_dir_all(&wrapper_directory).expect("create wrapper directory");
    let fake_tmux = wrapper_directory.join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    // A relative first entry, resolved against the relay's own working directory.
    // The rest of the inherited `PATH` is kept because the wrapper's `env`
    // shebang needs it.
    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let search_path = format!("wrappers:{inherited_path}");

    let host_child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .current_dir(temporary.path())
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
            .env("AGENTMUX_TMUX_COMMAND", "fake-tmux.sh")
            .env("PATH", &search_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay --no-autostart"),
    );
    wait_for_relay_ready(&state_root, "alpha");

    let up = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .current_dir(temporary.path())
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
        .env("AGENTMUX_TMUX_COMMAND", "fake-tmux.sh")
        .env("PATH", &search_path)
        .output()
        .expect("run agentmux up");
    assert!(
        up.status.success(),
        "a bare command on a relative PATH entry must still be found; stderr:\n{}",
        String::from_utf8_lossy(&up.stderr)
    );

    let log = fs::read_to_string(fake_tmux_log_path(&fake_tmux))
        .expect("the wrapper on the relative PATH entry must have run and recorded");
    assert!(
        log.contains("new-session"),
        "the wrapper must have been reached for session creation; got:\n{log}"
    );

    // The other half, and the reason the fix resolves the program rather than
    // normalizing the environment: the client's `PATH` must reach it untouched.
    // A tmux client hands its environment to a server it starts and thence to
    // every pane, and a pane is started with `-c <member directory>`, so
    // rewriting a relative entry here would silently repoint coder lookups from
    // the member's directory to the relay's. The fake tmux stands where the
    // client stands, so what it recorded is what a pane would inherit.
    let observed_path = fs::read_to_string(fake_tmux_search_path_file(&fake_tmux))
        .expect("the wrapper must have recorded the PATH it inherited");
    assert_eq!(
        observed_path.trim(),
        search_path,
        "the client's PATH must arrive exactly as inherited, relative entry included"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
}

#[test]
fn a_bare_tmux_command_keeps_execvp_search_order_across_relative_entries() {
    // The lookup for a bare name belongs to execvp, and its search is more than
    // "first file of that name": a non-executable candidate is passed over for a
    // later entry. This drives two relative entries where the first shadows the
    // second by name only, so a hand-rolled lookup that stopped at the first
    // match would run nothing.
    //
    // It does not reproduce the sharper case of a file carrying an execute bit
    // the effective user cannot use — that needs a file owned by another
    // principal, which an unprivileged fixture cannot create. An execute-only
    // script does not stand in for it either: exec of a shebang script succeeds
    // and the interpreter fails afterwards, so the search is never resumed. What
    // rules that case out is not testing it but declining to reimplement the
    // search at all.
    let temporary = TempDir::new().expect("temporary");
    let config_root = temporary.path().join("config");
    let state_root = temporary.path().join("named-state");
    let inscriptions_root = temporary.path().join("inscriptions");
    fs::create_dir_all(&config_root).expect("create config root");
    fs::create_dir_all(&state_root).expect("create state root");
    fs::create_dir_all(&inscriptions_root).expect("create inscriptions root");
    write_bundle_configuration_with_options(&config_root, "alpha", None, &["a"], Some(false));

    let shadowed_directory = temporary.path().join("wrappers-shadowed");
    let wrapper_directory = temporary.path().join("wrappers");
    fs::create_dir_all(&shadowed_directory).expect("create shadowed directory");
    fs::create_dir_all(&wrapper_directory).expect("create wrapper directory");
    // Same name, earlier on PATH, and not executable at all.
    let shadowed = shadowed_directory.join("fake-tmux.sh");
    fs::write(&shadowed, "#!/usr/bin/env bash\nexit 3\n").expect("write shadowed wrapper");
    fs::set_permissions(&shadowed, fs::Permissions::from_mode(0o644))
        .expect("make shadowed wrapper non-executable");
    let fake_tmux = wrapper_directory.join("fake-tmux.sh");
    write_fake_tmux_script(&fake_tmux);

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let search_path = format!("wrappers-shadowed:wrappers:{inherited_path}");

    let host_child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .current_dir(temporary.path())
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
            .env("AGENTMUX_TMUX_COMMAND", "fake-tmux.sh")
            .env("PATH", &search_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agentmux host relay --no-autostart"),
    );
    wait_for_relay_ready(&state_root, "alpha");

    let up = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .current_dir(temporary.path())
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
        .env("AGENTMUX_TMUX_COMMAND", "fake-tmux.sh")
        .env("PATH", &search_path)
        .output()
        .expect("run agentmux up");
    assert!(
        up.status.success(),
        "the executable later entry must be reached; stderr:\n{}",
        String::from_utf8_lossy(&up.stderr)
    );

    let log = fs::read_to_string(fake_tmux_log_path(&fake_tmux))
        .expect("the executable wrapper must have run and recorded");
    assert!(
        log.contains("new-session"),
        "the wrapper on the later PATH entry must have been reached; got:\n{log}"
    );
    assert!(
        !fake_tmux_log_path(&shadowed).exists(),
        "the shadowing non-executable entry must not have run"
    );

    shutdown_relay_if_present(&state_root, "alpha");
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
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

    let host_child = process::RelayChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_agentmux"))
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
            .expect("spawn agentmux host relay --no-autostart"),
    );
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
    host_child
        .wait_with_output(process::HARNESS_CHILD_WAIT_DEFAULT)
        .ok();
}
