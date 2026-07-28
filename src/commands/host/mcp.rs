use std::{env, path::PathBuf};

use serde_json::json;

use crate::{
    configuration::{ConfigurationRoots, load_bundle_configuration},
    mcp::{McpConfiguration, McpReadiness, McpStartupFault},
    runtime::{
        association::{
            AssociationSource, McpAssociationCli, McpAssociationEnvironment,
            load_local_mcp_overrides, resolve_association, resolve_sender_session,
        },
        error::RuntimeError,
        inscriptions::{
            configure_process_inscriptions, emit_inscription, mcp_inscriptions_path,
            mcp_unassociated_inscriptions_path,
        },
        paths::{BundleRuntimePaths, RuntimeRoots},
        starter::ensure_starter_configuration_layout,
    },
};

use crate::commands::{McpHostArguments, shared};

/// A resolved operational context.
struct PreparedRuntime {
    roots: RuntimeRoots,
    associated_bundle_paths: Option<BundleRuntimePaths>,
    sender_session: Option<String>,
    diagnostics: StartupDiagnostics,
}

/// What startup reports about how it arrived at its association.
#[derive(Default)]
struct StartupDiagnostics {
    /// Why the association is absent, when it is. Distinct from a startup
    /// fault: a relay-wide server carrying no bundle is legitimate.
    association_reason: Option<String>,
    /// The tier each identity came from, so a resolution that silently fell
    /// below the tier an operator configured is visible without reproducing it.
    bundle_source: Option<AssociationSource>,
    session_source: Option<AssociationSource>,
    /// A supplied identity which did not resolve. Retained with its original
    /// cause rather than flattened into a generic unassociated server, because
    /// an operator naming something unresolvable has a mistake to repair, not a
    /// configuration to accept.
    startup_fault: Option<McpStartupFault>,
}

/// Runs the MCP stdio service.
///
/// Takes the argument parse *result* rather than parsed arguments: once
/// `host mcp` is identifiable, even an argument fault is retained rather than
/// fatal. A failed parse contributes no values at all -- the whole result is
/// discarded -- so nothing partially parsed can leak into the running server.
pub(super) async fn run_mcp_host(
    arguments: Result<McpHostArguments, RuntimeError>,
) -> Result<(), RuntimeError> {
    // Past this point the process serves the protocol whatever it finds. Failing
    // startup would erase the tool inventory negotiated at `initialize` rather
    // than degrading it, leaving agents calling tools their context says exist,
    // and would bury the cause in a log no agent reads. Only an inability to
    // serve the protocol itself remains fatal.
    let prepared = arguments
        .map_err(to_startup_fault)
        .and_then(|arguments| prepare_runtime(&arguments));
    let configuration = match prepared {
        Ok(mut prepared) => {
            let inscriptions_path = match (
                prepared.associated_bundle_paths.as_ref(),
                prepared.sender_session.as_deref(),
            ) {
                (Some(bundle_paths), Some(session_name)) => mcp_inscriptions_path(
                    &prepared.roots.inscriptions_root,
                    bundle_paths.bundle_name.as_str(),
                    session_name,
                ),
                _ => mcp_unassociated_inscriptions_path(&prepared.roots.inscriptions_root),
            };
            // Inscriptions are diagnostics, not the protocol, so failing to open
            // them must not terminate the process. It is still a startup fault
            // and is retained as one: a fault reported only on stderr reaches
            // nobody, and this one can be a foreign-owned path, where an agent
            // running fully functional with no audit trail is precisely what
            // must not pass quietly. Stderr as well, never stdout, which carries
            // the protocol -- the sink that would otherwise record this is the
            // thing that just failed.
            if let Err(source) = configure_process_inscriptions(&inscriptions_path) {
                eprintln!("failed to configure MCP inscriptions: {source}");
                // An earlier fault keeps precedence: it names the condition that
                // came first, and this one would not have been reached without it.
                prepared
                    .diagnostics
                    .startup_fault
                    .get_or_insert_with(|| to_startup_fault(source));
            }
            let readiness = match prepared.diagnostics.startup_fault.clone() {
                Some(fault) => McpReadiness::Unavailable(fault),
                None => McpReadiness::Ready,
            };
            let configuration = McpConfiguration {
                configuration_roots: Some(prepared.roots.configuration_roots),
                state_root: prepared.roots.state_root,
                associated_bundle_paths: prepared.associated_bundle_paths,
                sender_session: prepared.sender_session,
                readiness,
            };
            emit_startup_inscription(&configuration, &prepared.diagnostics);
            configuration
        }
        Err(fault) => {
            // Faulted before any root resolved, so there is nowhere to inscribe.
            // A fault found *after* roots resolve takes the branch above, which
            // keeps its inscriptions. The fault still reaches an operator
            // through every tool call, which is the channel that gets it
            // repaired.
            let configuration = McpConfiguration {
                configuration_roots: None,
                state_root: PathBuf::new(),
                associated_bundle_paths: None,
                sender_session: None,
                readiness: McpReadiness::Unavailable(fault.clone()),
            };
            emit_startup_inscription(
                &configuration,
                &StartupDiagnostics {
                    startup_fault: Some(fault),
                    ..StartupDiagnostics::default()
                },
            );
            configuration
        }
    };

    crate::mcp::run(configuration)
        .await
        .map_err(|source| RuntimeError::io("run MCP stdio service", std::io::Error::other(source)))
}

/// Builds the operational context, or reports the fault which prevented one.
fn prepare_runtime(arguments: &McpHostArguments) -> Result<PreparedRuntime, McpStartupFault> {
    let current_directory = env::current_dir().map_err(|source| McpStartupFault {
        code: "runtime_startup_failed".to_string(),
        message: format!("cannot resolve current working directory: {source}"),
    })?;
    let roots =
        shared::resolve_roots(&arguments.runtime, &current_directory).map_err(to_startup_fault)?;
    ensure_starter_configuration_layout(&roots).map_err(to_startup_fault)?;
    let local_overrides =
        load_local_mcp_overrides(&roots.configuration_roots).map_err(to_startup_fault)?;

    let candidates = resolve_association(
        &McpAssociationCli {
            bundle_name: arguments.bundle_name.clone(),
            session_name: arguments.session_name.clone(),
        },
        &McpAssociationEnvironment::from_process_environment(),
        local_overrides.as_ref(),
        arguments.default_bundle.as_deref(),
    );

    let bundle_source = candidates.bundle.as_ref().map(|candidate| candidate.source);
    let unassociated = |roots, reason: String| PreparedRuntime {
        roots,
        associated_bundle_paths: None,
        sender_session: None,
        diagnostics: StartupDiagnostics {
            association_reason: Some(reason),
            bundle_source,
            ..StartupDiagnostics::default()
        },
    };
    let retained = |roots, source: RuntimeError| PreparedRuntime {
        roots,
        associated_bundle_paths: None,
        sender_session: None,
        diagnostics: StartupDiagnostics {
            bundle_source,
            startup_fault: Some(to_startup_fault(source)),
            ..StartupDiagnostics::default()
        },
    };

    let Some(bundle_candidate) = candidates.bundle.clone() else {
        return Ok(unassociated(
            roots,
            "no bundle resolved from --bundle, injected environment, association file, or \
             --default-bundle"
                .to_string(),
        ));
    };

    // The bundle was named by some tier, so failing to load it is a mistake to
    // repair rather than a configuration to accept.
    let bundle =
        match load_bundle_configuration(&roots.configuration_roots, &bundle_candidate.value)
            .map_err(shared::map_bundle_load_error)
        {
            Ok(bundle) => bundle,
            Err(source) => return Ok(retained(roots, source)),
        };

    // With no candidate, the session falls to matching the working directory
    // against the member directories the bundle file already declares. A
    // candidate that *was* supplied is validated and nothing more, so a typo
    // cannot authenticate as whichever member owns the current directory.
    let session_candidate = candidates.session.clone();
    let sender_session = match resolve_sender_session(
        &bundle,
        session_candidate
            .as_ref()
            .map(|candidate| candidate.value.as_str()),
        &current_directory,
    ) {
        Ok(Some(sender_session)) => sender_session,
        Ok(None) => {
            return Ok(unassociated(
                roots,
                format!(
                    "no session resolved from --session-name, injected environment, or \
                     association file, and working directory '{}' matched no session directory \
                     declared by bundle '{}'",
                    current_directory.display(),
                    bundle_candidate.value
                ),
            ));
        }
        Err(source) => return Ok(retained(roots, source)),
    };
    let session_source = session_candidate
        .map_or(AssociationSource::WorkingDirectory, |candidate| {
            candidate.source
        });

    let associated_bundle_paths =
        match BundleRuntimePaths::resolve(&roots.state_root, &bundle_candidate.value) {
            Ok(paths) => paths,
            Err(source) => return Ok(retained(roots, source)),
        };

    Ok(PreparedRuntime {
        roots,
        associated_bundle_paths: Some(associated_bundle_paths),
        sender_session: Some(sender_session),
        diagnostics: StartupDiagnostics {
            association_reason: None,
            bundle_source,
            session_source: Some(session_source),
            startup_fault: None,
        },
    })
}

/// Maps a runtime failure onto a retained startup fault, preserving its code so
/// a caller can tell an absent configuration layer from a malformed file.
fn to_startup_fault(source: RuntimeError) -> McpStartupFault {
    let code = match &source {
        RuntimeError::Validation { code, .. } => code.clone(),
        RuntimeError::InvalidArgument { .. } => "validation_invalid_arguments".to_string(),
        RuntimeError::SecurityForeignOwned { .. } => "runtime_security_foreign_owned".to_string(),
        _ => "runtime_startup_failed".to_string(),
    };
    McpStartupFault {
        code,
        message: source.to_string(),
    }
}

fn emit_startup_inscription(configuration: &McpConfiguration, diagnostics: &StartupDiagnostics) {
    let startup_fault = diagnostics.startup_fault.as_ref();
    emit_inscription(
        "mcp.startup",
        &json!({
            "association_status": if configuration.sender_session.is_some() {
                "associated"
            } else {
                "unassociated"
            },
            "association_reason": diagnostics.association_reason,
            "bundle_source": diagnostics.bundle_source.map(AssociationSource::as_str),
            "session_source": diagnostics.session_source.map(AssociationSource::as_str),
            "startup_fault_code": startup_fault.map(|fault| fault.code.clone()),
            "startup_fault_message": startup_fault.map(|fault| fault.message.clone()),
            "bundle_name": configuration
                .associated_bundle_paths
                .as_ref()
                .map(|paths| paths.bundle_name.clone()),
            "session_name": configuration.sender_session.clone(),
            "configuration_layers": configuration
                .configuration_roots
                .as_ref()
                .map(ConfigurationRoots::layers),
            "state_root": configuration.state_root,
        }),
    );
}
