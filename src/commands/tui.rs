use std::{
    env,
    path::PathBuf,
    process::{Command, Stdio},
};

use crate::{
    configuration::load_bundle_configuration,
    runtime::{
        association::WorkspaceContext,
        bootstrap::{BootstrapOptions, bootstrap_relay, resolve_relay_program},
        error::RuntimeError,
        paths::BundleRuntimePaths,
        paths::RuntimeRoots,
        starter::ensure_starter_configuration_layout,
        tui_session::resolve_tui_session_identity,
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
    ensure_starter_configuration_layout(&roots.configuration_root)?;
    let resolved_session = resolve_tui_session_identity(
        &roots.configuration_root,
        &workspace.workspace_root,
        parsed.bundle_name.as_deref(),
        parsed.session_selector.as_deref(),
    )?;
    load_bundle_configuration(&roots.configuration_root, &resolved_session.bundle_name)
        .map_err(shared::map_bundle_load_error)?;
    let paths = BundleRuntimePaths::resolve(&roots.state_root, &resolved_session.bundle_name)?;
    ensure_tui_relay_available(&roots, &paths)?;
    crate::tui::run(crate::tui::TuiLaunchOptions {
        bundle_name: resolved_session.bundle_name,
        sender_session: resolved_session.session_id,
        relay_socket: paths.relay_socket,
        look_lines: parsed.lines,
    })
}

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
            "--session" => {
                parsed.session_selector =
                    Some(shared::take_value(arguments, &mut index, "--session")?);
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
        "Usage: agentmux tui [--bundle NAME] [--session NAME] [--lines N] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}

fn ensure_tui_relay_available(
    roots: &RuntimeRoots,
    paths: &BundleRuntimePaths,
) -> Result<(), RuntimeError> {
    let relay_program = resolve_relay_program()?;
    let configuration_root = roots.configuration_root.clone();
    let state_root = roots.state_root.clone();
    let inscriptions_root = roots.inscriptions_root.clone();
    let relay_command = relay_program.clone();
    bootstrap_relay(paths, BootstrapOptions::default(), move || {
        spawn_relay_host_for_tui(
            relay_command.clone(),
            configuration_root.clone(),
            state_root.clone(),
            inscriptions_root.clone(),
        )
    })?;
    Ok(())
}

fn spawn_relay_host_for_tui(
    relay_program: PathBuf,
    configuration_root: PathBuf,
    state_root: PathBuf,
    inscriptions_root: PathBuf,
) -> Result<(), RuntimeError> {
    let spawn_result = Command::new(&relay_program)
        .arg("host")
        .arg("relay")
        .arg("--config-directory")
        .arg(configuration_root)
        .arg("--state-directory")
        .arg(state_root)
        .arg("--inscriptions-directory")
        .arg(inscriptions_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match spawn_result {
        Ok(_) => Ok(()),
        Err(source) => Err(RuntimeError::RelaySpawnFailure {
            command: relay_program,
            source,
        }),
    }
}
