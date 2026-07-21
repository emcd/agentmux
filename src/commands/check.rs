//! `agentmux check configuration` — pre-flight bundle configuration validation.
//!
//! Loads `coders.toml`, the bundle `.toml` file(s), `policies.toml`,
//! `relay.toml`, and the `users.toml` policy mappings through the exact path the
//! relay uses at startup ([`preflight_bundle_configuration`]). The goal is an
//! earlier, clearer failure than discovering a typo only when the relay refuses
//! to start: it exits non-zero on the first invalid bundle and reports the
//! offending file path plus field-level detail. The check is read-only — it
//! never scaffolds or mutates configuration.

use std::{env, fs};

use crate::{
    configuration::{ConfigurationError, bundles_configuration_directory, load_ui_configuration},
    relay::{RelayError, load_relay_runtime_configuration, preflight_bundle_configuration},
    runtime::{association::WorkspaceContext, error::RuntimeError},
};

use super::{CheckArguments, shared};

const BUNDLE_FILE_SUFFIX: &str = ".toml";

pub(super) fn run_agentmux_check(arguments: &[String]) -> Result<(), RuntimeError> {
    if arguments
        .iter()
        .any(|value| value == "--help" || value == "-h")
    {
        print_check_help();
        return Ok(());
    }

    let parsed = parse_check_arguments(arguments)?;
    let current_directory = env::current_dir()
        .map_err(|source| RuntimeError::io("resolve current working directory", source))?;
    let workspace = WorkspaceContext::discover(&current_directory)?;
    let roots = shared::resolve_roots(&parsed.runtime, &workspace, None)?;

    // Validate relay-level configuration before bundle discovery, so a malformed,
    // unknown-field, wrong-type, or invalid-peer relay.toml is reported even when
    // the config root has no bundles — matching relay startup, which rejects the
    // same artifact up front. The shared loader keeps check and startup in step.
    load_relay_runtime_configuration(&roots.configuration_root, None, None)
        .map_err(preflight_error_to_runtime)?;

    // Validate ui.toml alongside relay.toml at the config-root level: a
    // malformed UI-surface config should fail pre-flight with its path and
    // field detail, matching how the TUI/CLI reject it when loading surface
    // defaults. An absent or valid ui.toml is a no-op.
    load_ui_configuration(&roots.configuration_root).map_err(configuration_error_to_runtime)?;

    let bundle_names = match parsed.bundle_id.as_deref() {
        Some(bundle_id) => vec![bundle_id.to_string()],
        None => discover_bundle_names(&roots.configuration_root)?,
    };
    if bundle_names.is_empty() {
        return Err(RuntimeError::validation(
            "validation_no_bundles",
            format!(
                "no bundle configurations found under {}",
                bundles_configuration_directory(&roots.configuration_root).display()
            ),
        ));
    }

    // Fail-fast: validate bundles in deterministic order and surface the first
    // invalid one with full detail. No partial-load or graceful degradation.
    for bundle_name in &bundle_names {
        preflight_bundle_configuration(&roots.configuration_root, bundle_name)
            .map_err(preflight_error_to_runtime)?;
        println!("ok: {bundle_name}");
    }
    println!(
        "checked {} bundle configuration(s): all valid",
        bundle_names.len()
    );
    Ok(())
}

fn parse_check_arguments(arguments: &[String]) -> Result<CheckArguments, RuntimeError> {
    let Some(subcommand) = arguments.first().map(String::as_str) else {
        return Err(RuntimeError::validation(
            "validation_invalid_params",
            "missing check subcommand; expected 'configuration'".to_string(),
        ));
    };
    if subcommand != "configuration" {
        return Err(RuntimeError::InvalidArgument {
            argument: subcommand.to_string(),
            message: "unknown check subcommand".to_string(),
        });
    }

    let mut parsed = CheckArguments::default();
    let mut index = 1usize;
    while index < arguments.len() {
        if shared::parse_runtime_flag(arguments, &mut index, &mut parsed.runtime)? {
            index += 1;
            continue;
        }
        match arguments[index].as_str() {
            value if !value.starts_with('-') => {
                if parsed.bundle_id.is_some() {
                    return Err(RuntimeError::InvalidArgument {
                        argument: value.to_string(),
                        message: "unexpected second positional bundle id".to_string(),
                    });
                }
                parsed.bundle_id = Some(value.to_string());
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

/// Enumerates bundle ids from `<config-root>/bundles/*.toml`. Returns an empty
/// list (not an error) when the directory is absent, mirroring the relay's own
/// discovery so an unconfigured root reports "no bundles" rather than failing on
/// a missing directory.
fn discover_bundle_names(
    configuration_root: &std::path::Path,
) -> Result<Vec<String>, RuntimeError> {
    let bundles_directory = bundles_configuration_directory(configuration_root);
    if !bundles_directory.exists() {
        return Ok(Vec::new());
    }
    let mut bundle_names = fs::read_dir(&bundles_directory)
        .map_err(|source| {
            RuntimeError::io(
                format!("read bundle directory {}", bundles_directory.display()),
                source,
            )
        })?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.path().file_name().map(ToOwned::to_owned))
        .filter_map(|name| name.to_str().map(ToOwned::to_owned))
        .filter_map(|name| name.strip_suffix(BUNDLE_FILE_SUFFIX).map(ToOwned::to_owned))
        .collect::<Vec<_>>();
    bundle_names.sort_unstable();
    Ok(bundle_names)
}

/// Maps a relay pre-flight error onto a runtime error while preserving the
/// structured `details` (file path, offending field, policy id, control, …).
/// The shared `map_relay_error` drops `details`, which would gut the whole point
/// of the check — field-level diagnostics — so this command renders them inline.
/// Maps a configuration load error onto a runtime validation error, preserving
/// the offending file path and message so the pre-flight report stays
/// field-level. Used for config-root artifacts (like `ui.toml`) that load
/// through the configuration module rather than the relay pre-flight path.
fn configuration_error_to_runtime(error: ConfigurationError) -> RuntimeError {
    match error {
        ConfigurationError::InvalidConfiguration { path, message } => RuntimeError::validation(
            "validation_invalid_arguments",
            format!("invalid configuration {}: {}", path.display(), message),
        ),
        other => RuntimeError::validation("validation_invalid_arguments", other.to_string()),
    }
}

fn preflight_error_to_runtime(error: RelayError) -> RuntimeError {
    let RelayError {
        code,
        message,
        details,
    } = error;
    let rendered = match details {
        Some(details) => format!("{message} — {details}"),
        None => message,
    };
    RuntimeError::validation(code, rendered)
}

pub(super) fn print_check_help() {
    println!(
        "Usage: agentmux check configuration [<bundle-id>] [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}
