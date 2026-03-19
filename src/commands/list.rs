use std::env;

use serde_json::json;

use crate::{
    configuration::load_bundle_configuration,
    relay::{RelayRequest, RelayResponse, request_relay},
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

use super::{ListArguments, shared};

pub(super) fn run_agentmux_list(arguments: &[String]) -> Result<(), RuntimeError> {
    if arguments
        .iter()
        .any(|value| value == "--help" || value == "-h")
    {
        print_list_help();
        return Ok(());
    }

    let parsed = parse_list_arguments(arguments)?;
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
    let sender_session = Some(resolve_sender_session(
        &bundle,
        &association.session_name,
        &current_directory,
    )?);
    let paths = BundleRuntimePaths::resolve(&roots.state_root, &association.bundle_name)?;
    let response = request_relay(&paths.relay_socket, &RelayRequest::List { sender_session })
        .map_err(|source| shared::map_relay_request_failure(&paths.relay_socket, source))?;
    let payload = match response {
        RelayResponse::List {
            schema_version,
            bundle_name,
            recipients,
        } => json!({
            "schema_version": schema_version,
            "bundle_name": bundle_name,
            "recipients": recipients,
        }),
        RelayResponse::Error { error } => return Err(shared::map_relay_error(error)),
        other => {
            return Err(RuntimeError::validation(
                "internal_unexpected_failure",
                format!("relay returned unexpected response variant: {other:?}"),
            ));
        }
    };

    if parsed.output_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).map_err(|source| {
                RuntimeError::io("encode list response json", std::io::Error::other(source))
            })?
        );
    } else {
        println!(
            "bundle={}",
            payload["bundle_name"].as_str().unwrap_or_default()
        );
        if let Some(recipients) = payload["recipients"].as_array() {
            for recipient in recipients {
                let session = recipient["session_name"].as_str().unwrap_or_default();
                if let Some(display_name) = recipient["display_name"].as_str() {
                    println!("{session}\t{display_name}");
                } else {
                    println!("{session}");
                }
            }
        }
    }

    Ok(())
}

fn parse_list_arguments(arguments: &[String]) -> Result<ListArguments, RuntimeError> {
    let mut parsed = ListArguments::default();
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
            "--json" => parsed.output_json = true,
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

pub(super) fn print_list_help() {
    println!(
        "Usage: agentmux list [--bundle NAME] [--sender NAME] [--json] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}
