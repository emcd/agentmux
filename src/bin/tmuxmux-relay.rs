use std::{env, path::PathBuf};

use tmuxmux::relay::reconcile_bundle;
use tmuxmux::runtime::{
    bootstrap::{acquire_relay_runtime_lock, bind_relay_listener},
    error::RuntimeError,
    paths::{
        BundleRuntimePaths, RuntimeRootOverrides, RuntimeRoots, ensure_bundle_runtime_directory,
    },
};

#[derive(Debug)]
struct RelayArguments {
    bundle_name: String,
    configuration_root: Option<PathBuf>,
    state_root: Option<PathBuf>,
    repository_root: Option<PathBuf>,
}

impl Default for RelayArguments {
    fn default() -> Self {
        Self {
            bundle_name: "default".to_string(),
            configuration_root: None,
            state_root: None,
            repository_root: None,
        }
    }
}

fn main() {
    if let Err(err) = run() {
        eprintln!("tmuxmux-relay: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), RuntimeError> {
    let arguments = parse_arguments(env::args().skip(1).collect())?;
    let current_directory = env::current_dir()
        .map_err(|source| RuntimeError::io("resolve current working directory", source))?;
    let overrides = RuntimeRootOverrides {
        configuration_root: arguments.configuration_root,
        state_root: arguments.state_root,
        repository_root: arguments.repository_root.or(Some(current_directory)),
    };
    let roots = RuntimeRoots::resolve(&overrides)?;
    let paths = BundleRuntimePaths::resolve(&roots.state_root, &arguments.bundle_name)?;
    ensure_bundle_runtime_directory(&paths)?;
    let _runtime_lock = acquire_relay_runtime_lock(&paths)?;
    let report = reconcile_bundle(
        &roots.configuration_root,
        &paths.bundle_name,
        &paths.tmux_socket,
    )
    .map_err(map_reconcile_error)?;
    let listener = bind_relay_listener(&paths)?;
    println!(
        "tmuxmux-relay listening bundle={} socket={} bootstrap={:?} created={} pruned={}",
        paths.bundle_name,
        paths.relay_socket.display(),
        report.bootstrap_session,
        report.created_sessions.len(),
        report.pruned_sessions.len(),
    );
    for incoming in listener.incoming() {
        match incoming {
            Ok(mut stream) => {
                if let Err(source) =
                    tmuxmux::relay::serve_connection(&mut stream, &roots.configuration_root, &paths)
                {
                    eprintln!("tmuxmux-relay: request handling failed: {source}");
                }
            }
            Err(source) if source.kind() == std::io::ErrorKind::Interrupted => {
                continue;
            }
            Err(source) => {
                return Err(RuntimeError::io("accept relay socket connection", source));
            }
        }
    }
    Ok(())
}

fn map_reconcile_error(source: tmuxmux::relay::RelayError) -> RuntimeError {
    if source.code.starts_with("validation_") {
        return RuntimeError::validation(source.code, source.message);
    }
    let message = source.message.clone();
    RuntimeError::io(message, std::io::Error::other(format!("{source:?}")))
}

fn parse_arguments(arguments: Vec<String>) -> Result<RelayArguments, RuntimeError> {
    let mut parsed = RelayArguments::default();
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
        "Usage: tmuxmux-relay [--bundle NAME] [--config-directory PATH] \
         [--state-directory PATH] \
         [--repository-root PATH]"
    );
}
