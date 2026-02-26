use std::{env, path::PathBuf};

use tmuxmux::runtime::{
    bootstrap::{acquire_relay_runtime_lock, bind_relay_listener},
    error::RuntimeError,
    paths::{BundleRuntimePaths, RuntimeRootOverrides, RuntimeRoots},
};

#[derive(Debug)]
struct RelayArguments {
    bundle_name: String,
    state_root: Option<PathBuf>,
    repository_root: Option<PathBuf>,
}

impl Default for RelayArguments {
    fn default() -> Self {
        Self {
            bundle_name: "default".to_string(),
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
    let overrides = RuntimeRootOverrides {
        configuration_root: None,
        state_root: arguments.state_root,
        repository_root: arguments.repository_root,
    };
    let roots = RuntimeRoots::resolve(&overrides)?;
    let paths = BundleRuntimePaths::resolve(&roots.state_root, &arguments.bundle_name)?;
    let _runtime_lock = acquire_relay_runtime_lock(&paths)?;
    let listener = bind_relay_listener(&paths)?;
    println!(
        "tmuxmux-relay listening bundle={} socket={}",
        paths.bundle_name,
        paths.relay_socket.display()
    );
    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => drop(stream),
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

fn parse_arguments(arguments: Vec<String>) -> Result<RelayArguments, RuntimeError> {
    let mut parsed = RelayArguments::default();
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
        "Usage: tmuxmux-relay [--bundle NAME] [--state-directory PATH] \
         [--repository-root PATH]"
    );
}
