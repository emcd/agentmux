use std::env;

use crate::{
    configuration::load_bundle_configuration,
    runtime::{
        association::WorkspaceContext, error::RuntimeError, paths::BundleRuntimePaths,
        starter::ensure_starter_configuration_layout, tui_session::resolve_tui_session_identity,
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
