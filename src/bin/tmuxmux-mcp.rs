use std::{env, path::PathBuf};

use tmuxmux::{
    configuration::{ConfigurationError, load_bundle_configuration},
    mcp::McpConfiguration,
    runtime::{
        association::{
            McpAssociationCli, load_local_mcp_overrides, resolve_association,
            validate_sender_session,
        },
        error::RuntimeError,
        paths::{BundleRuntimePaths, RuntimeRootOverrides, RuntimeRoots},
    },
};

#[derive(Debug, Default)]
struct McpArguments {
    bundle_name: Option<String>,
    configuration_root: Option<PathBuf>,
    state_root: Option<PathBuf>,
    repository_root: Option<PathBuf>,
    session_name: Option<String>,
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("tmuxmux-mcp: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), RuntimeError> {
    let arguments = parse_arguments(env::args().skip(1).collect())?;
    let current_directory = env::current_dir()
        .map_err(|source| RuntimeError::io("resolve current working directory", source))?;
    let workspace = tmuxmux::runtime::association::WorkspaceContext::discover(&current_directory)?;
    let local_overrides = load_local_mcp_overrides(&workspace.workspace_root)?;
    let configuration_root = arguments.configuration_root.clone().or_else(|| {
        local_overrides
            .as_ref()
            .and_then(|overrides| overrides.config_root.clone())
    });
    let association = resolve_association(
        &McpAssociationCli {
            bundle_name: arguments.bundle_name.clone(),
            session_name: arguments.session_name.clone(),
        },
        local_overrides.as_ref(),
        &workspace,
    )?;
    let overrides = RuntimeRootOverrides {
        configuration_root,
        state_root: arguments.state_root,
        repository_root: arguments
            .repository_root
            .or_else(|| Some(workspace.workspace_root.clone())),
    };
    let roots = RuntimeRoots::resolve(&overrides)?;
    let bundle = load_bundle_configuration(&roots.configuration_root, &association.bundle_name)
        .map_err(map_bundle_load_error)?;
    let session_name = validate_sender_session(&bundle, &association.session_name)?;
    let paths = BundleRuntimePaths::resolve(&roots.state_root, &association.bundle_name)?;
    let configuration = McpConfiguration {
        bundle_paths: paths,
        sender_session: Some(session_name),
    };
    tmuxmux::mcp::run(configuration)
        .await
        .map_err(|source| RuntimeError::io("run MCP stdio service", anyhow_to_io(source)))
}

fn parse_arguments(arguments: Vec<String>) -> Result<McpArguments, RuntimeError> {
    let mut parsed = McpArguments::default();
    let mut index = 0usize;

    while index < arguments.len() {
        match arguments[index].as_str() {
            "--bundle-name" => {
                let value = take_value(&arguments, &mut index, "--bundle-name")?;
                parsed.bundle_name = Some(value);
            }
            "--config-directory" => {
                let value = take_value(&arguments, &mut index, "--config-directory")?;
                parsed.configuration_root = Some(PathBuf::from(value));
            }
            "--state-directory" => {
                let value = take_value(&arguments, &mut index, "--state-directory")?;
                parsed.state_root = Some(PathBuf::from(value));
            }
            "--repository-root" => {
                let value = take_value(&arguments, &mut index, "--repository-root")?;
                parsed.repository_root = Some(PathBuf::from(value));
            }
            "--session-name" => {
                let value = take_value(&arguments, &mut index, "--session-name")?;
                parsed.session_name = Some(value);
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
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

fn take_value(arguments: &[String], index: &mut usize, flag: &str) -> Result<String, RuntimeError> {
    *index += 1;
    let Some(value) = arguments.get(*index) else {
        return Err(RuntimeError::InvalidArgument {
            argument: flag.to_string(),
            message: "missing value".to_string(),
        });
    };
    Ok(value.to_string())
}

fn print_help() {
    println!(
        "Usage: tmuxmux-mcp [--bundle-name NAME] [--config-directory PATH] \
         [--state-directory PATH] \
         [--repository-root PATH] [--session-name NAME]"
    );
}

fn anyhow_to_io(source: anyhow::Error) -> std::io::Error {
    std::io::Error::other(source.to_string())
}

fn map_bundle_load_error(source: ConfigurationError) -> RuntimeError {
    match source {
        ConfigurationError::UnknownBundle { bundle_name, .. } => RuntimeError::validation(
            "validation_unknown_bundle",
            format!("bundle '{}' is not configured", bundle_name),
        ),
        ConfigurationError::AmbiguousSender { .. } => RuntimeError::validation(
            "validation_unknown_sender",
            "sender association is ambiguous".to_string(),
        ),
        ConfigurationError::InvalidConfiguration { path, message } => RuntimeError::validation(
            "validation_invalid_arguments",
            format!(
                "invalid bundle configuration {}: {}",
                path.display(),
                message
            ),
        ),
        ConfigurationError::Io { context, source } => RuntimeError::io(context, source),
    }
}
