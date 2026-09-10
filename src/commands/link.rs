//! `link peer`: coordinate peer credential provisioning across two relays.
//!
//! The coordinator drives issuance on one relay and installation on the
//! other, carrying the PSK in process memory only. It never renders,
//! logs, or persists the secret: step summaries name principals and paths,
//! and unexpected relay responses surface as generic failures without
//! payloads (a debug dump could carry the issued PSK).

use serde_json::json;

use crate::{
    relay::{RelayRequest, RelayResponse, peer_scope::parse_peer_scope, request_relay},
    runtime::{
        error::RuntimeError,
        paths::{RelayRuntimePaths, RuntimeRootOverrides, RuntimeRoots},
        starter::ensure_starter_configuration_layout,
        tui_session::resolve_tui_session_identity,
    },
};

use super::{LinkPeerArguments, LinkPeerMode, LinkPeerPairedArguments, shared};

/// A relay endpoint resolved from an explicit state root: configuration,
/// socket, and operator identity for one side of the link.
struct LinkEndpoint {
    socket: std::path::PathBuf,
    namespace: String,
    session_id: String,
}

pub(super) fn run_agentmux_link(arguments: &[String]) -> Result<(), RuntimeError> {
    if arguments
        .iter()
        .any(|value| value == "--help" || value == "-h")
    {
        print_link_help();
        return Ok(());
    }

    let parsed = parse_link_arguments(arguments)?;
    let issuer = resolve_link_endpoint(
        &parsed.issuer_state_root,
        &parsed.issuer_configuration_layers,
        parsed.inscriptions_root.clone(),
        parsed.bundle_name.as_deref(),
        parsed.session_selector.as_deref(),
    )?;
    let destination = resolve_link_endpoint(
        &parsed.destination_state_root,
        &parsed.destination_configuration_layers,
        parsed.inscriptions_root.clone(),
        parsed.bundle_name.as_deref(),
        parsed.session_selector.as_deref(),
    )?;

    let mut lines = Vec::new();
    let summary = match parsed.mode {
        LinkPeerMode::OneWay => link_one_way(&parsed, &issuer, &destination, &mut lines)?,
        LinkPeerMode::Paired => link_paired(&parsed, &issuer, &destination, &mut lines)?,
    };
    if parsed.output_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&summary).map_err(|source| {
                RuntimeError::io(
                    "encode link peer response json",
                    std::io::Error::other(source),
                )
            })?
        );
    } else {
        for line in &lines {
            println!("{line}");
        }
    }
    Ok(())
}

/// Resolves one link endpoint: roots, socket, and operator identity. Both
/// relays must be live; neither needs the other's `[[peers]]` entry yet.
fn resolve_link_endpoint(
    state_root: &std::path::Path,
    configuration_layers: &[std::path::PathBuf],
    inscriptions_root: Option<std::path::PathBuf>,
    bundle_name: Option<&str>,
    session_selector: Option<&str>,
) -> Result<LinkEndpoint, RuntimeError> {
    let roots = RuntimeRoots::resolve(&RuntimeRootOverrides {
        configuration_layers: configuration_layers.to_vec(),
        state_root: Some(state_root.to_path_buf()),
        inscriptions_root,
    })?;
    ensure_starter_configuration_layout(&roots)?;
    let resolved_session =
        resolve_tui_session_identity(&roots.configuration_roots, bundle_name, session_selector)?;
    let relay_paths = RelayRuntimePaths::resolve(&roots.state_root);
    Ok(LinkEndpoint {
        socket: relay_paths.relay_socket,
        namespace: resolved_session.namespace,
        session_id: resolved_session.session_id,
    })
}

/// Sends one relay request as the endpoint's operator identity, mapping
/// transport failures to relay errors.
fn link_request(
    endpoint: &LinkEndpoint,
    request: &RelayRequest,
) -> Result<RelayResponse, RuntimeError> {
    request_relay(
        &endpoint.socket,
        endpoint.namespace.as_str(),
        endpoint.session_id.as_str(),
        request,
    )
    .map_err(|source| shared::map_relay_request_failure(&endpoint.socket, source))
}

/// Registers `alias@RELAY` on the destination, returning `Ok(())` whether
/// the record is fresh or already registered (second-claim tolerance: the
/// install step verifies the relay type before writing).
fn register_alias_referent(
    destination: &LinkEndpoint,
    alias: &str,
    scope: Option<String>,
) -> Result<(), RuntimeError> {
    let response = link_request(
        destination,
        &RelayRequest::NewPeer {
            principal_id: format!("{alias}@RELAY"),
            scope,
            destination: crate::relay::CredentialDestination::Response,
        },
    )?;
    match response {
        // The registration mint is explicitly discarded: no party needs it
        // for this direction. The record's credential stays valid-but-unheld
        // under the normal rotation lifecycle.
        RelayResponse::NewPeer { psk: _, .. } => Ok(()),
        RelayResponse::Error { error } if error.code == "validation_principal_exists" => Ok(()),
        RelayResponse::Error { error } => Err(shared::map_relay_error(error)),
        _ => Err(unexpected_link_response()),
    }
}

/// Issues (or, for upgrade, rotates) the inbound identity on the issuer,
/// returning the raw PSK in process memory only.
fn issue_inbound_credential(
    issuer: &LinkEndpoint,
    connect_as: &str,
    scope: Option<String>,
    upgrade: bool,
) -> Result<String, RuntimeError> {
    let request = if upgrade {
        RelayRequest::ChangePsk {
            principal_id: format!("{connect_as}@RELAY"),
            destination: crate::relay::CredentialDestination::Response,
        }
    } else {
        RelayRequest::NewPeer {
            principal_id: format!("{connect_as}@RELAY"),
            scope,
            destination: crate::relay::CredentialDestination::Response,
        }
    };
    match link_request(issuer, &request)? {
        RelayResponse::NewPeer { psk: Some(psk), .. } => Ok(psk),
        RelayResponse::ChangePsk { psk: Some(psk), .. } => Ok(psk),
        RelayResponse::NewPeer { .. } | RelayResponse::ChangePsk { .. } => {
            Err(RuntimeError::validation(
                "internal_unexpected_failure",
                "relay omitted the issued credential".to_string(),
            ))
        }
        RelayResponse::Error { error } => Err(shared::map_relay_error(error)),
        _ => Err(unexpected_link_response()),
    }
}

/// Installs `psk` into the destination's peer slot, retrying once on
/// transport failure with the same PSK (safe by install idempotency).
/// Relay-reported failures abort without retry: the relay already decided.
fn install_peer_slot(
    destination: &LinkEndpoint,
    alias: &str,
    psk: &str,
) -> Result<String, RuntimeError> {
    let request = RelayRequest::InstallPeerCredential {
        alias: alias.to_string(),
        psk: psk.to_string(),
    };
    let response = match link_request(destination, &request) {
        Ok(response) => response,
        // Transport failure is install-unknown: exactly one same-PSK retry.
        Err(_) => link_request(destination, &request)?,
    };
    match response {
        RelayResponse::InstallPeerCredential { written_path, .. } => Ok(written_path),
        RelayResponse::Error { error } => Err(shared::map_relay_error(error)),
        _ => Err(unexpected_link_response()),
    }
}

/// One-way link: register the alias referent on the destination, issue (or
/// rotate for upgrade) the inbound identity on the issuer, install into the
/// destination slot. Issuance failure halts before any install.
fn link_one_way(
    parsed: &LinkPeerArguments,
    issuer: &LinkEndpoint,
    destination: &LinkEndpoint,
    lines: &mut Vec<String>,
) -> Result<serde_json::Value, RuntimeError> {
    let upgrade = parsed.upgrade;
    register_alias_referent(destination, &parsed.alias, parsed.alias_scope.clone())?;
    lines.push(format!(
        "link peer: registered {}@RELAY on destination",
        parsed.alias
    ));
    let psk = issue_inbound_credential(issuer, &parsed.connect_as, parsed.scope.clone(), upgrade)?;
    lines.push(format!(
        "link peer: {} {}@RELAY on issuer",
        if upgrade { "rotated" } else { "issued" },
        parsed.connect_as
    ));
    let written_path = install_peer_slot(destination, &parsed.alias, &psk)?;
    lines.push(format!(
        "link peer: installed {} on destination",
        written_path
    ));
    // `psk` drops here without ever reaching output, logs, or files.
    Ok(json!({
        "alias": parsed.alias,
        "connect_as": parsed.connect_as,
        "written_path": written_path,
        "upgrade": upgrade,
    }))
}

/// Paired link: verify declared cross-equality first, retain both alias
/// mints, and cross-install with zero drops.
fn link_paired(
    parsed: &LinkPeerArguments,
    issuer: &LinkEndpoint,
    destination: &LinkEndpoint,
    lines: &mut Vec<String>,
) -> Result<serde_json::Value, RuntimeError> {
    let peer = parsed.paired.as_ref().expect("paired arguments");
    // Declared cross-equality is verified before either mint: each side's
    // declared connect-as must equal the opposite alias. This checks
    // operator-stated intent and derives nothing.
    if parsed.connect_as != peer.peer_alias || peer.peer_connect_as != parsed.alias {
        return Err(RuntimeError::validation(
            "validation_invalid_params",
            "paired link requires symmetric naming: --connect-as must equal --peer-alias and --peer-connect-as must equal --alias".to_string(),
        ));
    }
    let issuer_psk = register_paired_alias(issuer, &peer.peer_alias, peer.peer_scope.clone())?;
    let destination_psk =
        register_paired_alias(destination, &parsed.alias, parsed.alias_scope.clone())?;
    lines.push(format!(
        "link peer: registered {}@RELAY and {}@RELAY",
        peer.peer_alias, parsed.alias
    ));
    let destination_written = install_peer_slot(destination, &parsed.alias, &issuer_psk)?;
    let issuer_written = install_peer_slot(issuer, &peer.peer_alias, &destination_psk)?;
    lines.push(format!(
        "link peer: installed {destination_written} and {issuer_written}"
    ));
    // Both mints drop here without ever reaching output, logs, or files.
    Ok(json!({
        "alias": parsed.alias,
        "peer_alias": peer.peer_alias,
        "written_path": destination_written,
        "peer_written_path": issuer_written,
    }))
}

/// Registers one paired alias referent, retaining the mint. An existing
/// record aborts the pairing: its mint is unheld and unknowable, so the
/// pairing cannot proceed — rotate it into a known credential (upgrade)
/// instead.
fn register_paired_alias(
    endpoint: &LinkEndpoint,
    alias: &str,
    scope: Option<String>,
) -> Result<String, RuntimeError> {
    match link_request(
        endpoint,
        &RelayRequest::NewPeer {
            principal_id: format!("{alias}@RELAY"),
            scope,
            destination: crate::relay::CredentialDestination::Response,
        },
    )? {
        RelayResponse::NewPeer { psk: Some(psk), .. } => Ok(psk),
        RelayResponse::NewPeer { .. } => Err(RuntimeError::validation(
            "internal_unexpected_failure",
            "relay omitted the issued credential".to_string(),
        )),
        RelayResponse::Error { error } if error.code == "validation_principal_exists" => {
            Err(RuntimeError::validation(
                "validation_principal_exists",
                format!(
                    "{alias}@RELAY is already registered with an unheld credential; \
                     rotate it into a known credential first, then link one-way"
                ),
            ))
        }
        RelayResponse::Error { error } => Err(shared::map_relay_error(error)),
        _ => Err(unexpected_link_response()),
    }
}

/// Generic failure for unexpected relay response variants. Carries no
/// payload: a debug dump could contain the issued PSK.
fn unexpected_link_response() -> RuntimeError {
    RuntimeError::validation(
        "internal_unexpected_failure",
        "relay returned an unexpected response variant".to_string(),
    )
}

fn parse_link_arguments(arguments: &[String]) -> Result<LinkPeerArguments, RuntimeError> {
    let Some(subcommand) = arguments.first().map(String::as_str) else {
        return Err(RuntimeError::validation(
            "validation_invalid_params",
            "missing link subcommand; expected 'peer'".to_string(),
        ));
    };
    if subcommand != "peer" {
        return Err(RuntimeError::InvalidArgument {
            argument: subcommand.to_string(),
            message: "unknown link subcommand".to_string(),
        });
    }

    let mut issuer_state_root: Option<std::path::PathBuf> = None;
    let mut destination_state_root: Option<std::path::PathBuf> = None;
    let mut issuer_configuration_layers: Vec<std::path::PathBuf> = Vec::new();
    let mut destination_configuration_layers: Vec<std::path::PathBuf> = Vec::new();
    let mut inscriptions_root: Option<std::path::PathBuf> = None;
    let mut alias: Option<String> = None;
    let mut connect_as: Option<String> = None;
    let mut scope: Option<String> = None;
    let mut alias_scope: Option<String> = None;
    let mut paired = false;
    let mut peer_alias: Option<String> = None;
    let mut peer_scope: Option<String> = None;
    let mut peer_connect_as: Option<String> = None;
    let mut upgrade = false;
    let mut bundle_name: Option<String> = None;
    let mut session_selector: Option<String> = None;
    let mut output_json = false;
    let mut index = 1usize;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--issuer-state-directory" => {
                issuer_state_root = Some(
                    shared::take_value(arguments, &mut index, "--issuer-state-directory")?.into(),
                );
            }
            "--destination-state-directory" => {
                destination_state_root = Some(
                    shared::take_value(arguments, &mut index, "--destination-state-directory")?
                        .into(),
                );
            }
            "--issuer-configuration-directory" => {
                issuer_configuration_layers.push(
                    shared::take_value(arguments, &mut index, "--issuer-configuration-directory")?
                        .into(),
                );
            }
            "--destination-configuration-directory" => {
                destination_configuration_layers.push(
                    shared::take_value(
                        arguments,
                        &mut index,
                        "--destination-configuration-directory",
                    )?
                    .into(),
                );
            }
            "--inscriptions-directory" | "--logs-directory" => {
                inscriptions_root = Some(
                    shared::take_value(arguments, &mut index, "--inscriptions-directory")?.into(),
                );
            }
            "--alias" => alias = Some(shared::take_value(arguments, &mut index, "--alias")?),
            "--connect-as" => {
                connect_as = Some(shared::take_value(arguments, &mut index, "--connect-as")?);
            }
            "--scope" => scope = Some(shared::take_value(arguments, &mut index, "--scope")?),
            "--alias-scope" => {
                alias_scope = Some(shared::take_value(arguments, &mut index, "--alias-scope")?);
            }
            "--paired" => paired = true,
            "--peer-alias" => {
                peer_alias = Some(shared::take_value(arguments, &mut index, "--peer-alias")?);
            }
            "--peer-scope" => {
                peer_scope = Some(shared::take_value(arguments, &mut index, "--peer-scope")?);
            }
            "--peer-connect-as" => {
                peer_connect_as = Some(shared::take_value(
                    arguments,
                    &mut index,
                    "--peer-connect-as",
                )?);
            }
            "--upgrade" => upgrade = true,
            "--bundle" | "--bundle-name" => {
                bundle_name = Some(shared::take_value(arguments, &mut index, "--bundle")?);
            }
            "--as-session" => {
                session_selector = Some(shared::take_value(arguments, &mut index, "--as-session")?);
            }
            "--json" => output_json = true,
            value if value.starts_with('-') => {
                return Err(RuntimeError::InvalidArgument {
                    argument: value.to_string(),
                    message: "unknown argument".to_string(),
                });
            }
            value => {
                return Err(RuntimeError::InvalidArgument {
                    argument: value.to_string(),
                    message: "unexpected positional argument".to_string(),
                });
            }
        }
        index += 1;
    }

    let Some(issuer_state_root) = issuer_state_root else {
        return Err(RuntimeError::validation(
            "validation_invalid_params",
            "link peer requires --issuer-state-directory".to_string(),
        ));
    };
    let Some(destination_state_root) = destination_state_root else {
        return Err(RuntimeError::validation(
            "validation_invalid_params",
            "link peer requires --destination-state-directory".to_string(),
        ));
    };
    let Some(alias) = alias
        .take()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return Err(RuntimeError::validation(
            "validation_invalid_params",
            "link peer requires a non-empty --alias".to_string(),
        ));
    };
    let Some(connect_as) = connect_as
        .take()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return Err(RuntimeError::validation(
            "validation_invalid_params",
            "link peer requires a non-empty --connect-as".to_string(),
        ));
    };
    // Pre-submission scope grammar checks so malformed input fails before
    // any relay contact; the relay validates independently.
    for scope_value in scope.iter().chain(alias_scope.iter()) {
        if let Err(error) = parse_peer_scope(Some(scope_value)) {
            return Err(RuntimeError::validation(error.code, error.message));
        }
    }
    if paired {
        if upgrade {
            return Err(RuntimeError::validation(
                "validation_invalid_params",
                "--paired and --upgrade are mutually exclusive".to_string(),
            ));
        }
        let Some(peer_alias) = peer_alias
            .take()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        else {
            return Err(RuntimeError::validation(
                "validation_invalid_params",
                "paired link requires a non-empty --peer-alias".to_string(),
            ));
        };
        let Some(peer_connect_as) = peer_connect_as
            .take()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        else {
            return Err(RuntimeError::validation(
                "validation_invalid_params",
                "paired link requires a non-empty --peer-connect-as".to_string(),
            ));
        };
        for scope_value in peer_scope.iter() {
            if let Err(error) = parse_peer_scope(Some(scope_value)) {
                return Err(RuntimeError::validation(error.code, error.message));
            }
        }
        Ok(LinkPeerArguments {
            mode: LinkPeerMode::Paired,
            issuer_state_root,
            destination_state_root,
            issuer_configuration_layers,
            destination_configuration_layers,
            inscriptions_root,
            alias,
            connect_as,
            scope,
            alias_scope,
            paired: Some(LinkPeerPairedArguments {
                peer_alias,
                peer_scope,
                peer_connect_as,
            }),
            upgrade: false,
            bundle_name,
            session_selector,
            output_json,
        })
    } else {
        if peer_alias.is_some() || peer_scope.is_some() || peer_connect_as.is_some() {
            return Err(RuntimeError::validation(
                "validation_invalid_params",
                "--peer-alias, --peer-scope, and --peer-connect-as require --paired".to_string(),
            ));
        }
        Ok(LinkPeerArguments {
            mode: LinkPeerMode::OneWay,
            issuer_state_root,
            destination_state_root,
            issuer_configuration_layers,
            destination_configuration_layers,
            inscriptions_root,
            alias,
            connect_as,
            scope,
            alias_scope,
            paired: None,
            upgrade,
            bundle_name,
            session_selector,
            output_json,
        })
    }
}

pub(super) fn print_link_help() {
    println!(
        "Usage: agentmux link peer --issuer-state-directory PATH --destination-state-directory PATH --alias ALIAS --connect-as ID [--scope SCOPE] [--alias-scope SCOPE] [--paired --peer-alias ALIAS --peer-scope SCOPE --peer-connect-as ID] [--upgrade] [--bundle NAME] [--as-session NAME] [--json]"
    );
    println!(
        "Coordinates peer credential provisioning: registers the alias on the destination, issues the inbound identity on the issuer, and installs the PSK into the destination peer slot without ever printing it. --paired provisions both directions at once; --upgrade rotates instead of issuing."
    );
}
