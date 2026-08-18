//! `agentmux check configuration` — pre-flight bundle configuration validation.
//!
//! Loads `coders.toml`, the bundle `.toml` file(s), `policies.toml`,
//! `relay.toml`, and the `users.toml` policy mappings through the exact path the
//! relay uses at startup ([`preflight_bundle_configuration`]). The goal is an
//! earlier, clearer failure than discovering a typo only when the relay refuses
//! to start: it exits non-zero on the first invalid bundle and reports the
//! offending file path plus field-level detail. The check is read-only — it
//! never scaffolds or mutates configuration.

use std::{
    collections::BTreeMap,
    env,
    io::{self, Write},
    path::{Path, PathBuf},
};

use crate::{
    configuration::{
        BUNDLE_EXTENSION, BUNDLES_DIRECTORY, ConfigurationError, bundle_directory_layers,
        effective_bundle_definitions, load_ui_configuration, supplied_root_configuration_sources,
    },
    relay::{RelayError, load_relay_runtime_configuration, preflight_bundle_configuration},
    runtime::{error::RuntimeError, starter::validate_supplied_configuration_layers},
};

use super::{CheckArguments, shared};

/// Runs the pre-flight check, flushing standard output ahead of any failure.
///
/// The flush is redundant against the standard library as it stands: standard
/// output is line-buffered whether or not it is a terminal, so each reported
/// line reaches the pipe at its newline and the two streams stay in order on
/// their own. It is here because that is an implementation choice rather than a
/// guarantee. A standard output buffered by destination — the C discipline, and
/// a change the standard library has been asked for more than once — would let
/// the caller's failure report overtake the source report explaining it in a
/// captured transcript, which is what an operator pastes into a bug report.
/// Flushing at the single exit costs nothing, states the ordering contract where
/// a reader can find it, and covers failure paths added later.
pub(super) fn run_agentmux_check(arguments: &[String]) -> Result<(), RuntimeError> {
    let outcome = check_configuration(arguments);
    if outcome.is_err() {
        let _ = io::stdout().flush();
    }
    outcome
}

fn check_configuration(arguments: &[String]) -> Result<(), RuntimeError> {
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
    let roots = shared::resolve_roots(&parsed.runtime)?;

    // Findings that do not stop the run. This command is what an operator
    // reaches for *because* something is wrong, so a layer it cannot read is the
    // condition it exists to report rather than a reason to stop reporting.
    // Collected here, rendered as they are found, and answered for at the end.
    let mut findings: Vec<String> = Vec::new();

    // Discovery is a lookup, not validation, so it runs ahead of everything and
    // serves the source report and the validation loop from one enumeration. An
    // absent bundles directory yields no entries rather than an error, mirroring
    // the relay's own discovery.
    let bundle_definitions = match effective_bundle_definitions(&roots.configuration_roots) {
        Ok(definitions) => definitions,
        Err(error) => {
            findings.push(error.to_string());
            BTreeMap::new()
        }
    };
    let bundle_names: Vec<String> = match parsed.bundle_id.as_deref() {
        Some(bundle_id) => vec![bundle_id.to_string()],
        None => bundle_definitions.keys().cloned().collect(),
    };

    // Ahead of every validation, including the layer-existence check below. A
    // missing override layer is the case where the report is worth most: it
    // shows every artifact resolving from the layers beneath, which is the
    // diagnosis the operator needs, and the error that follows names the layer
    // responsible. Withholding it there would blank the report on one of the few
    // runs that motivate having one.
    // Resolved whether or not the report will be printed, so that an unreadable
    // layer is a finding on the same terms in quiet mode. Tying the lookup to
    // the flag would make `--quiet` change which policy the condition gets, not
    // just how much is written.
    let supplied_sources = match supplied_root_configuration_sources(&roots.configuration_roots) {
        Ok(sources) => sources,
        Err(error) => {
            findings.push(error.to_string());
            Vec::new()
        }
    };
    if !parsed.quiet {
        report_configuration_sources(
            &supplied_sources,
            &bundle_definitions,
            &bundle_names,
            &current_directory,
        );
    }

    // Rendered before the checks that follow, so the operator sees the layer
    // problem ahead of anything it caused rather than after.
    for finding in &findings {
        println!("finding: {finding}");
    }

    // A supplied layer that does not exist is reported rather than absorbed.
    // Every other command reaches this check through
    // `ensure_starter_configuration_layout`; pre-flight cannot, because that path
    // scaffolds and this command is read-only. Without it a typo'd override layer
    // validates clean from the layers below it — the silent demotion the closed
    // list exists to prevent, and precisely the misconfiguration an operator runs
    // pre-flight to catch.
    validate_supplied_configuration_layers(&roots)?;

    // Validate relay-level configuration before bundle validation, so a
    // malformed, unknown-field, wrong-type, or invalid-peer relay.toml is
    // reported even when the config root has no bundles — matching relay startup,
    // which rejects the same artifact up front. The shared loader keeps check and
    // startup in step.
    load_relay_runtime_configuration(&roots.configuration_roots, None, None)
        .map_err(preflight_error_to_runtime)?;

    // Validate ui.toml alongside relay.toml at the config-root level: a
    // malformed UI-surface config should fail pre-flight with its path and
    // field detail, matching how the TUI/CLI reject it when loading surface
    // defaults. An absent or valid ui.toml is a no-op.
    load_ui_configuration(&roots.configuration_roots).map_err(configuration_error_to_runtime)?;

    // Suppressed when a layer could not be read: enumeration returned nothing
    // because it could not look, and this error would name the wrong problem
    // and abort ahead of the finding that explains it. The findings answer for
    // the run instead.
    if bundle_names.is_empty() && findings.is_empty() {
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
        if !parsed.quiet {
            println!("ok: {bundle_name}");
        }
    }
    // Answered for only after every other check has had its say. Reaching here
    // with findings means the bundles that could be validated are valid and the
    // configuration as a whole still is not, which is a failure — reporting it
    // as success would leave the operator with a clean run and a deployment
    // resolving from layers they did not intend.
    if !findings.is_empty() {
        return Err(RuntimeError::validation(
            "validation_unreadable_configuration_layer",
            findings.join("; "),
        ));
    }
    if !parsed.quiet {
        println!(
            "checked {} bundle configuration(s): all valid",
            bundle_names.len()
        );
    }
    Ok(())
}

/// Writes the physical file supplying each artifact this run resolves.
///
/// Runs before validation because validation is fail-fast: interleaving the two
/// would truncate the report at the first invalid artifact, on precisely the run
/// where the whole picture is most wanted. The lookup that produces the sources
/// can itself fault on an unreadable layer; that is recorded as a finding by the
/// caller and does not stop the report, so what was resolvable is still shown.
///
/// Only artifacts a layer actually supplies are reported. Bundles are scoped to
/// the set being validated, so a run naming one bundle reports that bundle
/// rather than every discoverable one.
fn report_configuration_sources(
    supplied_sources: &[(&'static str, PathBuf)],
    bundle_definitions: &BTreeMap<String, PathBuf>,
    bundle_names: &[String],
    current_directory: &Path,
) {
    for (name, path) in supplied_sources {
        println!(
            "source {name}: {}",
            physical_path(current_directory, path).display()
        );
    }
    for bundle_name in bundle_names {
        // A name with no definition is a bundle the operator asked for and no
        // layer supplies; validation reports that, and inventing a source line
        // for it here would contradict the report's own contract.
        let Some(path) = bundle_definitions.get(bundle_name) else {
            continue;
        };
        println!(
            "source {BUNDLES_DIRECTORY}/{bundle_name}.{BUNDLE_EXTENSION}: {}",
            physical_path(current_directory, path).display()
        );
    }
}

/// Renders a resolved path in absolute form.
///
/// A layer list may carry relative layers, which resolve against the working
/// directory. A relative path in the report is ambiguous the moment the report
/// leaves the shell that produced it — which is the normal case, since the
/// report exists to be read elsewhere.
fn physical_path(current_directory: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        current_directory.join(path)
    }
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
            "-q" | "--quiet" => {
                parsed.quiet = true;
            }
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
        // The structured code is the only axis carrying this condition: every
        // failure exits `1`, so folding the layer fault into the generic
        // argument code would make it indistinguishable from a malformed file
        // at the one surface that reports on the distinction. Every other
        // mapper across the runtime, relay, and MCP boundaries preserves it.
        layer @ ConfigurationError::UnreadableConfigurationLayer { .. } => {
            RuntimeError::validation(
                "validation_unreadable_configuration_layer",
                layer.to_string(),
            )
        }
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
        "Usage: agentmux check configuration [<bundle-id>] [-q|--quiet] [--configuration-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH]"
    );
    println!(
        "  Reports the physical file supplying each resolved artifact, so a shadowed copy is distinguishable from the copy in effect."
    );
    println!("  -q, --quiet  Suppress success output, leaving the exit code and any failure.");
}
