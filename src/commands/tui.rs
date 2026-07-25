use std::{
    cell::RefCell,
    env,
    path::PathBuf,
    process::{Child, Command, Stdio},
    rc::Rc,
    time::Duration,
};

use crate::{
    configuration::load_bundle_group_memberships,
    runtime::{
        association::WorkspaceContext,
        bootstrap::{BootstrapOptions, SpawnedRelay, bootstrap_relay, resolve_relay_program},
        error::RuntimeError,
        paths::{RelayRuntimePaths, RuntimeRoots},
        starter::ensure_starter_configuration_layout,
        tui_session::resolve_tui_launch_identity,
    },
};

use super::{RuntimeArguments, TuiArguments, shared};

pub(super) fn run_agentmux_tui(arguments: &[String]) -> Result<(), RuntimeError> {
    if arguments
        .iter()
        .any(|value| value == "--help" || value == "-h")
    {
        print_tui_help();
        return Ok(());
    }

    let parsed = parse_tui_arguments(arguments)?;
    let current_directory = env::current_dir()
        .map_err(|source| RuntimeError::io("resolve current working directory", source))?;
    let workspace = WorkspaceContext::discover(&current_directory)?;
    let roots = shared::resolve_roots(&parsed.runtime, &workspace, None)?;
    ensure_starter_configuration_layout(&roots)?;
    // The interactive TUI does not require a default bundle to launch: a fresh
    // install ships no `default-bundle` (and the example bundle is empty), so an
    // eager bundle load here would crash startup (issues/tui/11, issues/runtime/3).
    // Resolve available bundles first and seed the browsing context from the
    // first one when no bundle is configured; the operator picks from there.
    let available_bundles = load_bundle_group_memberships(&roots.configuration_root)
        .map_err(shared::map_bundle_load_error)?
        .into_iter()
        .map(|membership| membership.bundle_name)
        .collect::<Vec<_>>();
    let resolved_session = resolve_tui_launch_identity(
        &roots.configuration_root,
        &workspace.workspace_root,
        parsed.bundle_name.as_deref(),
        parsed.session_selector.as_deref(),
        available_bundles.first().map(String::as_str),
    )?;
    let relay_paths = RelayRuntimePaths::resolve(&roots.state_root);
    let owned_relay = ensure_tui_relay_available(&roots, &relay_paths)?;
    let run_result = crate::tui::run(crate::tui::TuiLaunchOptions {
        namespace: resolved_session.namespace,
        sender_session: resolved_session.session_id,
        relay_socket: relay_paths.relay_socket,
        look_lines: parsed.lines,
        available_bundles,
    });
    // Stop the relay this TUI auto-spawned, regardless of how the TUI exited
    // (quit key, terminal signal, or a run error). A relay that was already
    // running when the TUI started is not owned here and is left untouched.
    if let Some(relay) = owned_relay {
        relay.stop(OWNED_RELAY_STOP_GRACE);
    }
    run_result
}

// The relay force-exits within its own shutdown watchdog grace (5s) of a
// termination signal; wait slightly longer so graceful teardown — pruning the
// tmux sessions it owns and reaping the tmux server — completes before the TUI
// process exits and returns the shell to the operator.
const OWNED_RELAY_STOP_GRACE: Duration = Duration::from_millis(6_000);

fn parse_tui_arguments(arguments: &[String]) -> Result<TuiArguments, RuntimeError> {
    let mut parsed = TuiArguments {
        bundle_name: None,
        session_selector: None,
        lines: None,
        runtime: RuntimeArguments::default(),
    };
    let mut index = 0usize;
    while index < arguments.len() {
        if shared::parse_runtime_flag(arguments, &mut index, &mut parsed.runtime)? {
            index += 1;
            continue;
        }
        match arguments[index].as_str() {
            "--bundle" | "--bundle-name" => {
                parsed.bundle_name = Some(shared::take_value(arguments, &mut index, "--bundle")?);
            }
            "--as-session" => {
                parsed.session_selector =
                    Some(shared::take_value(arguments, &mut index, "--as-session")?);
            }
            "--lines" => {
                let value = shared::take_value(arguments, &mut index, "--lines")?;
                parsed.lines = Some(shared::parse_look_lines(value.as_str())?);
            }
            unknown => {
                return Err(RuntimeError::InvalidArgument {
                    argument: unknown.to_string(),
                    message: "unknown argument".to_string(),
                });
            }
        }
        index += 1;
    }
    Ok(parsed)
}

pub(super) fn print_tui_help() {
    println!(
        "Usage: agentmux tui [--bundle NAME] [--as-session NAME] [--lines N] [--configuration-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH] [--discover-local-configuration]"
    );
}

// Returns the relay this TUI invocation auto-spawned, or `None` when a relay
// was already reachable at startup (systemd, a peer client, or a prior
// invocation). Ownership is gated on `BootstrapReport.spawned_relay`, which is
// true only for the single contender that actually performed the spawn: under
// the bootstrap spawn lock, other contenders merely wait for readiness, never
// run the spawn closure, and therefore never own the relay for teardown.
fn ensure_tui_relay_available(
    roots: &RuntimeRoots,
    paths: &RelayRuntimePaths,
) -> Result<Option<SpawnedRelay>, RuntimeError> {
    let relay_program = resolve_relay_program()?;
    let configuration_root = roots.configuration_root.clone();
    let state_root = roots.state_root.clone();
    let inscriptions_root = roots.inscriptions_root.clone();
    let relay_command = relay_program.clone();
    // The spawn closure runs on the calling thread, so a single-threaded cell
    // is sufficient to hand the spawned child back out of `bootstrap_relay`.
    let spawned: Rc<RefCell<Option<SpawnedRelay>>> = Rc::new(RefCell::new(None));
    let spawned_inner = Rc::clone(&spawned);
    let outcome = bootstrap_relay(paths, BootstrapOptions::default(), move || {
        let child = spawn_relay_host_for_tui(
            relay_command.clone(),
            configuration_root.clone(),
            state_root.clone(),
            inscriptions_root.clone(),
        )?;
        *spawned_inner.borrow_mut() = Some(SpawnedRelay::new(child));
        Ok(())
    });
    match outcome {
        Ok(report) if report.spawned_relay => Ok(spawned.borrow_mut().take()),
        Ok(_) => Ok(None),
        Err(error) => {
            // Bootstrap can fail *after* the spawn closure already launched the
            // relay — most notably a readiness timeout in `wait_for_relay_ready`
            // once the child is live. Such a child is captured here but never
            // reaches the ownership handoff to `run_agentmux_tui`, and dropping a
            // `std::process::Child` detaches rather than kills it. Tear down the
            // relay we own on this error path so a failed `agentmux tui` startup
            // does not leave an orphaned relay — and the tmux sessions it owns —
            // running behind the returned error.
            if let Some(relay) = spawned.borrow_mut().take() {
                relay.stop(OWNED_RELAY_STOP_GRACE);
            }
            Err(error)
        }
    }
}

fn spawn_relay_host_for_tui(
    relay_program: PathBuf,
    configuration_root: PathBuf,
    state_root: PathBuf,
    inscriptions_root: PathBuf,
) -> Result<Child, RuntimeError> {
    let mut command = Command::new(&relay_program);
    command
        .arg("host")
        .arg("relay")
        .arg("--configuration-directory")
        .arg(configuration_root)
        .arg("--state-directory")
        .arg(state_root)
        .arg("--inscriptions-directory")
        .arg(inscriptions_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    isolate_process_group(&mut command);
    command
        .spawn()
        .map_err(|source| RuntimeError::RelaySpawnFailure {
            command: relay_program,
            source,
        })
}

#[cfg(unix)]
fn isolate_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    // Put the auto-spawned relay in its own process group so incidental
    // terminal signals — SIGINT on Ctrl-C, SIGHUP on window close — do not reach
    // it through the TUI's process group. The TUI is then the sole author of the
    // spawned relay's shutdown, sending an explicit signal on exit regardless of
    // how the TUI itself was torn down.
    command.process_group(0);
}

#[cfg(not(unix))]
fn isolate_process_group(_command: &mut Command) {}
