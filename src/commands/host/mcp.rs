use std::env;

use serde_json::json;

use crate::{
    configuration::load_bundle_configuration,
    mcp::McpConfiguration,
    runtime::{
        association::{
            McpAssociationCli, WorkspaceContext, load_local_mcp_overrides, resolve_association,
            resolve_sender_session,
        },
        error::RuntimeError,
        inscriptions::{
            configure_process_inscriptions, emit_inscription, mcp_inscriptions_path,
            mcp_unassociated_inscriptions_path,
        },
        paths::BundleRuntimePaths,
        starter::ensure_starter_configuration_layout,
    },
};

use crate::commands::{McpHostArguments, shared};

pub(super) async fn run_mcp_host(arguments: McpHostArguments) -> Result<(), RuntimeError> {
    let current_directory = env::current_dir()
        .map_err(|source| RuntimeError::io("resolve current working directory", source))?;
    let workspace = WorkspaceContext::discover(&current_directory)?;
    let local_overrides = load_local_mcp_overrides(&workspace.workspace_root)?;
    let association_cli = McpAssociationCli {
        bundle_name: arguments.bundle_name.clone(),
        session_name: arguments.session_name.clone(),
    };
    let association_is_explicit = association_cli.bundle_name.is_some()
        || association_cli.session_name.is_some()
        || local_overrides.as_ref().is_some_and(|overrides| {
            overrides.bundle_name.is_some() || overrides.session_name.is_some()
        });
    let roots = shared::resolve_roots(&arguments.runtime, &workspace, local_overrides.as_ref())?;
    ensure_starter_configuration_layout(&roots)?;
    let mut associated_bundle_name = None::<String>;
    let mut sender_session = None::<String>;
    let mut startup_association_reason = None::<String>;

    let association = resolve_association(&association_cli, local_overrides.as_ref(), &workspace);
    match association {
        Ok(association) => {
            associated_bundle_name = Some(association.bundle_name.clone());
            let loaded_bundle =
                load_bundle_configuration(&roots.configuration_root, &association.bundle_name)
                    .map_err(shared::map_bundle_load_error);
            match loaded_bundle {
                Ok(bundle) => {
                    let resolved_sender = resolve_sender_session(
                        &bundle,
                        &association.session_name,
                        &current_directory,
                    );
                    match resolved_sender {
                        Ok(session_name) => sender_session = Some(session_name),
                        Err(source)
                            if should_start_mcp_unassociated(association_is_explicit, &source) =>
                        {
                            startup_association_reason = Some(source.to_string());
                            associated_bundle_name = None;
                        }
                        Err(source) => return Err(source),
                    }
                }
                Err(source) if should_start_mcp_unassociated(association_is_explicit, &source) => {
                    startup_association_reason = Some(source.to_string());
                    associated_bundle_name = None;
                }
                Err(source) => return Err(source),
            }
        }
        Err(source) if should_start_mcp_unassociated(association_is_explicit, &source) => {
            startup_association_reason = Some(source.to_string());
        }
        Err(source) => return Err(source),
    }

    let associated_bundle_paths = associated_bundle_name
        .as_deref()
        .map(|bundle_name| BundleRuntimePaths::resolve(&roots.state_root, bundle_name))
        .transpose()?;
    let inscriptions_path = if let Some(bundle_paths) = associated_bundle_paths.as_ref() {
        let session_name = sender_session
            .as_deref()
            .expect("associated startup must include sender session");
        mcp_inscriptions_path(
            &roots.inscriptions_root,
            bundle_paths.bundle_name.as_str(),
            session_name,
        )
    } else {
        mcp_unassociated_inscriptions_path(&roots.inscriptions_root)
    };
    configure_process_inscriptions(&inscriptions_path)?;
    emit_inscription(
        "mcp.startup",
        &json!({
            "association_status": if sender_session.is_some() { "associated" } else { "unassociated" },
            "association_reason": startup_association_reason,
            "bundle_name": associated_bundle_name,
            "session_name": sender_session.clone(),
            "runtime_bundle_name": associated_bundle_paths.as_ref().map(|paths| paths.bundle_name.clone()),
            "configuration_root": roots.configuration_root,
            "state_root": roots.state_root,
            "inscriptions_root": roots.inscriptions_root,
        }),
    );
    crate::mcp::run(McpConfiguration {
        configuration_root: roots.configuration_root,
        state_root: roots.state_root,
        associated_bundle_paths,
        sender_session,
    })
    .await
    .map_err(|source| RuntimeError::io("run MCP stdio service", std::io::Error::other(source)))
}

fn should_start_mcp_unassociated(association_is_explicit: bool, source: &RuntimeError) -> bool {
    if association_is_explicit {
        return false;
    }
    matches!(
        source,
        RuntimeError::Validation { code, .. }
            if code == "validation_unknown_bundle" || code == "validation_unknown_sender"
    )
}
