use std::env;

use serde_json::{Value, json};

use crate::{
    configuration::{
        BundleConfiguration, TargetConfiguration, load_bundle_configuration,
        load_bundle_group_memberships,
    },
    relay::{
        ListedBundle, ListedBundleState, ListedSession, ListedSessionTransport, RelayRequest,
        RelayResponse, request_relay,
    },
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
    let roots = shared::resolve_roots(&parsed.runtime, &workspace, local_overrides.as_ref())?;
    ensure_starter_configuration_layout(&roots.configuration_root)?;
    let association = resolve_association(
        &McpAssociationCli {
            bundle_name: parsed.bundle_name.clone(),
            session_name: None,
        },
        local_overrides.as_ref(),
        &workspace,
    )?;
    let home_bundle_name = association.bundle_name.clone();
    let sender_candidate = association.session_name.clone();

    let payload = if parsed.all_bundles {
        let memberships = load_bundle_group_memberships(&roots.configuration_root)
            .map_err(shared::map_bundle_load_error)?;
        let mut bundle_names = memberships
            .into_iter()
            .map(|membership| membership.bundle_name)
            .collect::<Vec<_>>();
        bundle_names.sort_unstable();
        bundle_names.dedup();

        let mut schema_version = "1".to_string();
        let mut bundles = Vec::<ListedBundle>::new();
        for bundle_name in bundle_names {
            let listed = request_listed_bundle(
                &roots,
                &bundle_name,
                sender_candidate.as_str(),
                home_bundle_name.as_str(),
                &current_directory,
            )?;
            schema_version = listed.schema_version;
            bundles.push(listed.bundle);
        }
        json!({
            "schema_version": schema_version,
            "bundles": bundles,
        })
    } else {
        let listed = request_listed_bundle(
            &roots,
            &association.bundle_name,
            sender_candidate.as_str(),
            home_bundle_name.as_str(),
            &current_directory,
        )?;
        json!({
            "schema_version": listed.schema_version,
            "bundle": listed.bundle,
        })
    };

    if parsed.output_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).map_err(|source| {
                RuntimeError::io("encode list response json", std::io::Error::other(source))
            })?
        );
    } else {
        if let Some(bundle) = payload.get("bundle").and_then(Value::as_object) {
            print_human_bundle(bundle);
        } else if let Some(bundles) = payload.get("bundles").and_then(Value::as_array) {
            for (index, bundle) in bundles.iter().enumerate() {
                if index > 0 {
                    println!();
                }
                if let Some(bundle) = bundle.as_object() {
                    print_human_bundle(bundle);
                }
            }
        }
    }

    Ok(())
}

fn parse_list_arguments(arguments: &[String]) -> Result<ListArguments, RuntimeError> {
    let Some(subcommand) = arguments.first().map(String::as_str) else {
        return Err(RuntimeError::validation(
            "validation_invalid_params",
            "missing list subcommand; expected 'sessions'".to_string(),
        ));
    };
    if subcommand != "sessions" {
        return Err(RuntimeError::InvalidArgument {
            argument: subcommand.to_string(),
            message: "unknown list subcommand".to_string(),
        });
    }

    let mut parsed = ListArguments::default();
    let mut index = 1usize;
    while index < arguments.len() {
        if shared::parse_runtime_flag(arguments, &mut index, &mut parsed.runtime)? {
            index += 1;
            continue;
        }
        match arguments[index].as_str() {
            "--bundle" | "--bundle-name" => {
                parsed.bundle_name = Some(shared::take_value(arguments, &mut index, "--bundle")?);
            }
            "--all" => parsed.all_bundles = true,
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
    if parsed.bundle_name.is_some() && parsed.all_bundles {
        return Err(RuntimeError::validation(
            "validation_invalid_params",
            "list sessions accepts either --bundle or --all, not both".to_string(),
        ));
    }
    Ok(parsed)
}

pub(super) fn print_list_help() {
    println!(
        "Usage: agentmux list sessions [--bundle NAME|--all] [--json] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}

#[derive(Clone, Debug)]
struct ListedBundleResult {
    schema_version: String,
    bundle: ListedBundle,
}

fn request_listed_bundle(
    roots: &crate::runtime::paths::RuntimeRoots,
    bundle_name: &str,
    sender_candidate: &str,
    home_bundle_name: &str,
    current_directory: &std::path::Path,
) -> Result<ListedBundleResult, RuntimeError> {
    let bundle = load_bundle_configuration(&roots.configuration_root, bundle_name)
        .map_err(shared::map_bundle_load_error)?;
    let sender_session = resolve_sender_session(&bundle, sender_candidate, current_directory)?;
    let paths = BundleRuntimePaths::resolve(&roots.state_root, bundle_name)?;
    let response = request_relay(
        &paths.relay_socket,
        &RelayRequest::List {
            sender_session: Some(sender_session),
        },
    );
    let response = match response {
        Ok(response) => response,
        Err(source) => {
            let error = shared::map_relay_request_failure(&paths.relay_socket, source);
            if can_use_home_fallback(&error, bundle_name, home_bundle_name) {
                return Ok(ListedBundleResult {
                    schema_version: "1".to_string(),
                    bundle: synthesize_unreachable_bundle(&bundle, &paths),
                });
            }
            return Err(error);
        }
    };
    match response {
        RelayResponse::List {
            schema_version,
            bundle,
        } => Ok(ListedBundleResult {
            schema_version,
            bundle,
        }),
        RelayResponse::Error { error } => Err(shared::map_relay_error(error)),
        other => Err(RuntimeError::validation(
            "internal_unexpected_failure",
            format!("relay returned unexpected response variant: {other:?}"),
        )),
    }
}

fn can_use_home_fallback(
    error: &RuntimeError,
    requested_bundle_name: &str,
    home_bundle_name: &str,
) -> bool {
    if requested_bundle_name != home_bundle_name {
        return false;
    }
    matches!(
        error,
        RuntimeError::Validation { code, .. } if code == "relay_unavailable"
    )
}

fn synthesize_unreachable_bundle(
    bundle: &BundleConfiguration,
    paths: &BundleRuntimePaths,
) -> ListedBundle {
    let (state_reason_code, state_reason) = if paths.relay_socket.exists() {
        (
            Some("relay_unavailable".to_string()),
            Some(format!(
                "relay socket exists but list request failed at {}",
                paths.relay_socket.display()
            )),
        )
    } else {
        (
            Some("not_started".to_string()),
            Some(format!(
                "relay socket is absent at {}",
                paths.relay_socket.display()
            )),
        )
    };
    ListedBundle {
        id: bundle.bundle_name.clone(),
        state: ListedBundleState::Down,
        state_reason_code,
        state_reason,
        sessions: bundle
            .members
            .iter()
            .map(|member| ListedSession {
                id: member.id.clone(),
                name: member.name.clone(),
                transport: match &member.target {
                    TargetConfiguration::Tmux(_) => ListedSessionTransport::Tmux,
                    TargetConfiguration::Acp(_) => ListedSessionTransport::Acp,
                },
            })
            .collect::<Vec<_>>(),
    }
}

fn print_human_bundle(bundle: &serde_json::Map<String, Value>) {
    let bundle_id = bundle
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let state = bundle
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut header = format!("bundle={bundle_id} state={state}");
    if let Some(reason_code) = bundle.get("state_reason_code").and_then(Value::as_str) {
        header.push_str(format!(" reason_code={reason_code}").as_str());
    }
    if let Some(reason) = bundle.get("state_reason").and_then(Value::as_str) {
        header.push_str(format!(" reason={reason}").as_str());
    }
    println!("{header}");

    if let Some(sessions) = bundle.get("sessions").and_then(Value::as_array) {
        for session in sessions {
            let id = session
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let transport = session
                .get("transport")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if let Some(name) = session.get("name").and_then(Value::as_str) {
                println!("{id}\t{name}\t{transport}");
            } else {
                println!("{id}\t{transport}");
            }
        }
    }
}
