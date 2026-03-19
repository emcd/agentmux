use std::env;

use crate::{
    configuration::load_bundle_configuration,
    runtime::{
        association::{
            McpAssociationCli, WorkspaceContext, load_local_mcp_overrides, resolve_association,
            resolve_sender_session,
        },
        error::RuntimeError,
        paths::BundleRuntimePaths,
        starter::ensure_starter_configuration_layout,
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
    let local_overrides = load_local_mcp_overrides(&workspace.workspace_root)?;
    let association = resolve_association(
        &McpAssociationCli {
            bundle_name: parsed.bundle_name.clone(),
            session_name: parsed.sender_session.clone(),
        },
        local_overrides.as_ref(),
        &workspace,
    )?;
    let roots = shared::resolve_roots(&parsed.runtime, &workspace, local_overrides.as_ref())?;
    ensure_starter_configuration_layout(&roots.configuration_root)?;
    let bundle = load_bundle_configuration(&roots.configuration_root, &association.bundle_name)
        .map_err(shared::map_bundle_load_error)?;
    let sender_session =
        resolve_sender_session(&bundle, &association.session_name, &current_directory)?;
    let paths = BundleRuntimePaths::resolve(&roots.state_root, &association.bundle_name)?;
    crate::tui::run(crate::tui::TuiLaunchOptions {
        bundle_name: association.bundle_name,
        sender_session,
        relay_socket: paths.relay_socket,
        look_lines: parsed.lines,
    })
}

fn parse_tui_arguments(arguments: &[String]) -> Result<TuiArguments, RuntimeError> {
    let mut parsed = TuiArguments {
        bundle_name: None,
        sender_session: None,
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
            "--sender" | "--session-name" => {
                parsed.sender_session =
                    Some(shared::take_value(arguments, &mut index, "--sender")?);
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
        "Usage: agentmux tui [--bundle NAME] [--sender NAME] [--lines N] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}
