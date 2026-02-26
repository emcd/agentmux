use std::{env, path::PathBuf, time::Duration};

use tmuxmux::{
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
        configuration_root: None,
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
        let _child = spawn_relay_process(&relay_program, &paths)?;
        Ok(())
    })?;

    let configuration = McpConfiguration {
        bundle_paths: paths,
        sender_session: arguments.sender_session,
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
        "Usage: tmuxmux-mcp [--bundle NAME] [--state-directory PATH] \
         [--repository-root PATH] [--sender-session NAME] \
         [--startup-timeout-ms N] [--no-auto-start-relay]"
    );
}

fn anyhow_to_io(source: anyhow::Error) -> std::io::Error {
    std::io::Error::other(source.to_string())
}
