//! `agentmux check configuration` — pre-flight bundle configuration validation.
//!
//! Loads `coders.toml`, the bundle `.toml` file(s), `policies.toml`,
//! `relay.toml`, and the `users.toml` policy mappings through the exact path the
//! relay uses at startup ([`preflight_bundle_configuration`]). The goal is an
//! earlier, clearer failure than discovering a typo only when the relay refuses
//! to start: it exits non-zero on the first invalid bundle and reports the
//! offending file path plus field-level detail. The check is read-only — it
//! never scaffolds or mutates configuration.

use std::env;

use crate::{
    configuration::{
        ConfigurationError, ConfigurationRoots, bundle_directory_layers,
        effective_bundle_definitions, load_ui_configuration,
    },
    relay::{RelayError, load_relay_runtime_configuration, preflight_bundle_configuration},
    runtime::{error::RuntimeError, starter::validate_supplied_configuration_layers},
};

use super::{CheckArguments, shared};

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
    let roots = shared::resolve_roots(&parsed.runtime, &current_directory)?;

    // A supplied layer that does not exist is reported here rather than absorbed.
    // Every other command reaches this check through
    // `ensure_starter_configuration_layout`; pre-flight cannot, because that path
    // scaffolds and this command is read-only. Without it a typo'd override layer
    // validates clean from the layers below it — the silent demotion the closed
    // list exists to prevent, and precisely the misconfiguration an operator runs
    // pre-flight to catch.
    validate_supplied_configuration_layers(&roots)?;

    // Validate relay-level configuration before bundle discovery, so a malformed,
    // unknown-field, wrong-type, or invalid-peer relay.toml is reported even when
    // the config root has no bundles — matching relay startup, which rejects the
    // same artifact up front. The shared loader keeps check and startup in step.
    load_relay_runtime_configuration(&roots.configuration_roots, None, None)
        .map_err(preflight_error_to_runtime)?;

    // Validate ui.toml alongside relay.toml at the config-root level: a
    // malformed UI-surface config should fail pre-flight with its path and
    // field detail, matching how the TUI/CLI reject it when loading surface
    // defaults. An absent or valid ui.toml is a no-op.
    load_ui_configuration(&roots.configuration_roots).map_err(configuration_error_to_runtime)?;

    let bundle_names = match parsed.bundle_id.as_deref() {
        Some(bundle_id) => vec![bundle_id.to_string()],
        None => discover_bundle_names(&roots.configuration_roots),
    };
    if bundle_names.is_empty() {
        return Err(RuntimeError::validation(
            "validation_no_bundles",
            // Every searched directory, not one: with an arbitrary layer list,
            // naming a single directory would misreport where the search looked
            // and send an operator to fix the wrong layer.
            format!(
                "no bundle configurations found under {}",
                bundle_directory_layers(&roots.configuration_roots)
                    .iter()
                    .map(|directory| directory.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }

    // Fail-fast: validate bundles in deterministic order and surface the first
    // invalid one with full detail. No partial-load or graceful degradation.
    for bundle_name in &bundle_names {
        preflight_bundle_configuration(&roots.configuration_roots, bundle_name)
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

/// Enumerates bundle ids from the effective bundle set, which unions every
/// layer's bundles directory with earlier entries shadowing later ones.
///
/// Returns an empty list (not an error) when no directory is present,
/// mirroring the relay's own discovery so an unconfigured root reports "no
/// bundles" rather than failing on a missing directory. Validating the union is
/// what makes this command validate what the relay would actually load.
fn discover_bundle_names(configuration_roots: &ConfigurationRoots) -> Vec<String> {
    effective_bundle_definitions(configuration_roots)
        .into_keys()
        .collect()
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
        "Usage: agentmux check configuration [<bundle-id>] [--configuration-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}
