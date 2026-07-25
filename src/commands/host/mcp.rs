use std::env;

use serde_json::json;

use crate::{
    configuration::load_bundle_configuration,
    mcp::McpConfiguration,
    runtime::{
        association::{
            McpAssociationCli, McpAssociationEnvironment, WorkspaceContext,
            load_local_mcp_overrides, resolve_association, resolve_sender_session,
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
    let association_cli = McpAssociationCli {
        bundle_name: arguments.bundle_name.clone(),
        session_name: arguments.session_name.clone(),
    };
    // Roots resolve before the association file is read, because that file now
    // lives under the configuration root rather than selecting it.
    let roots = shared::resolve_roots(&arguments.runtime, &workspace)?;
    ensure_starter_configuration_layout(&roots)?;
    let local_overrides = load_local_mcp_overrides(&roots.configuration_root)?;
    let environment = McpAssociationEnvironment::from_process_environment();
    let candidates = resolve_association(
        &association_cli,
        &environment,
        local_overrides.as_ref(),
        arguments.default_bundle.as_deref(),
    );

    let mut associated_bundle_name = None::<String>;
    let mut sender_session = None::<String>;
    let mut startup_association_reason = None::<String>;

    // An unresolved association is recorded rather than fatal. Nothing is
    // guessed to fill the gap: the previous filesystem inference produced an
    // answer that was plausible and wrong, which is how a session whose worktree
    // sat outside its bundle's tree bound to the wrong bundle.
    match candidates.bundle_name.as_deref() {
        None => {
            startup_association_reason = Some(
                "no bundle resolved from --bundle, injected environment, overlay, or \
                 --default-bundle"
                    .to_string(),
            );
        }
        Some(bundle_name) => {
            match load_bundle_configuration(&roots.configuration_root, bundle_name)
                .map_err(shared::map_bundle_load_error)
            {
                Ok(bundle) => {
                    // Session falls back to matching the working directory against
                    // declared member directories. That is declarative rather than
                    // inferential: the bundle file already states where each member
                    // lives.
                    match resolve_sender_session(
                        &bundle,
                        candidates.session_name.as_deref().unwrap_or_default(),
                        &current_directory,
                    ) {
                        Ok(session_name) => {
                            associated_bundle_name = Some(bundle_name.to_string());
                            sender_session = Some(session_name);
                        }
                        Err(source) => startup_association_reason = Some(source.to_string()),
                    }
                }
                Err(source) => startup_association_reason = Some(source.to_string()),
            }
        }
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
