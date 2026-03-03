use std::{env, path::PathBuf, time::Duration};

use tmuxmux::{
    configuration::{
        ConfigurationError, infer_sender_from_working_directory, load_bundle_configuration,
    },
    mcp::McpConfiguration,
    runtime::{
        bootstrap::{
            BootstrapOptions, bootstrap_relay, resolve_relay_program, spawn_relay_process,
        },
        error::RuntimeError,
        paths::{BundleRuntimePaths, RuntimeRootOverrides, RuntimeRoots},
    },
};

#[derive(Debug)]
struct McpArguments {
    bundle_name: String,
    auto_start_relay: bool,
    startup_timeout_ms: u64,
    configuration_root: Option<PathBuf>,
    state_root: Option<PathBuf>,
    repository_root: Option<PathBuf>,
    sender_session: Option<String>,
}

impl Default for McpArguments {
    fn default() -> Self {
        Self {
            bundle_name: "default".to_string(),
            auto_start_relay: true,
            startup_timeout_ms: 10_000,
            configuration_root: None,
            state_root: None,
            repository_root: None,
            sender_session: None,
        }
    }
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
    let overrides = RuntimeRootOverrides {
        configuration_root: arguments.configuration_root,
        state_root: arguments.state_root,
        repository_root: arguments.repository_root,
    };
    let roots = RuntimeRoots::resolve(&overrides)?;
    let paths = BundleRuntimePaths::resolve(&roots.state_root, &arguments.bundle_name)?;
    let relay_program = resolve_relay_program()?;
    let options = BootstrapOptions {
        auto_start_relay: arguments.auto_start_relay,
        startup_timeout: Duration::from_millis(arguments.startup_timeout_ms),
    };
    let _ = bootstrap_relay(&paths, options, || {
        let _child = spawn_relay_process(&relay_program, &paths, &roots.configuration_root)?;
        Ok(())
    })?;

    let sender_session =
        resolve_sender_session(arguments.sender_session, &roots.configuration_root, &paths)?;
    let configuration = McpConfiguration {
        bundle_paths: paths,
        sender_session,
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
            "--bundle" => {
                parsed.bundle_name = take_value(&arguments, &mut index, "--bundle")?;
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
            "--sender-session" => {
                let value = take_value(&arguments, &mut index, "--sender-session")?;
                parsed.sender_session = Some(value);
            }
            "--startup-timeout-ms" => {
                let value = take_value(&arguments, &mut index, "--startup-timeout-ms")?;
                parsed.startup_timeout_ms =
                    value
                        .parse::<u64>()
                        .map_err(|_| RuntimeError::InvalidArgument {
                            argument: "--startup-timeout-ms".to_string(),
                            message: "must be an unsigned integer".to_string(),
                        })?;
            }
            "--no-auto-start-relay" => parsed.auto_start_relay = false,
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
        "Usage: tmuxmux-mcp [--bundle NAME] [--config-directory PATH] \
         [--state-directory PATH] \
         [--repository-root PATH] [--sender-session NAME] \
         [--startup-timeout-ms N] [--no-auto-start-relay]"
    );
}

fn resolve_sender_session(
    explicit_sender: Option<String>,
    configuration_root: &std::path::Path,
    bundle_paths: &BundleRuntimePaths,
) -> Result<Option<String>, RuntimeError> {
    if explicit_sender.is_some() {
        return Ok(explicit_sender);
    }
    let bundle = match load_bundle_configuration(configuration_root, &bundle_paths.bundle_name) {
        Ok(value) => value,
        Err(ConfigurationError::UnknownBundle { .. }) => return Ok(None),
        Err(ConfigurationError::AmbiguousSender { .. }) => return Ok(None),
        Err(source) => {
            return Err(RuntimeError::io(
                "load bundle configuration for sender resolution",
                anyhow_to_io(anyhow::Error::from(source)),
            ));
        }
    };
    infer_sender_from_working_directory(
        &bundle,
        &env::current_dir().map_err(|source| {
            RuntimeError::io(
                "resolve current working directory for sender resolution",
                source,
            )
        })?,
    )
    .map_err(|source| {
        RuntimeError::io(
            "infer sender session",
            anyhow_to_io(anyhow::Error::from(source)),
        )
    })
}

fn anyhow_to_io(source: anyhow::Error) -> std::io::Error {
    std::io::Error::other(source.to_string())
}
