//! MCP parameter types to canonical operation inputs.
//!
//! Adapter-owned representation mapping. Each function converts one
//! already-deserialized MCP parameter struct into the operation input
//! it maps to, normalizing representation exactly as the current
//! handlers do (trimming, empty-to-absent, enum parsing, destination
//! folding) and rejecting representation failures with the identical
//! code, message, and details the handlers produce today. Unknown-field
//! rejection stays at the JSON boundary in the handlers'
//! `validate_*` pre-checks and is not duplicated here; handlers keep
//! calling those pre-checks until the bodies move into operations.
//! Command-string dispatch also stays in the handlers (it selects
//! which mapping and operation to invoke).
//!
//! The double-checking this creates is idempotent by construction:
//! mapping a validated struct succeeds with the same normalized
//! values the handlers put in the `RelayRequest` today.
//!
//! Dead code is expected until section 3.1 wires these mappings into
//! the reduced handlers; the allow below marks that seam.
#![allow(dead_code)]

use serde_json::{Value, json};

use crate::operations::error::OperationError;
use crate::operations::inputs::{
    BundleScope, ChangePskInput, ChangeScopeInput, DecideInput, DecideOutcome,
    DropPeerInput, LinkPeerInput, ListNamespacesInput, ListPrincipalsInput,
    LookInput, NewPeerInput, RawwInput, SendInput,
};
use crate::relay::CredentialDestination;
use crate::relay::peer_scope::{is_relay_principal_id, parse_peer_scope};
use crate::runtime::paths::{is_valid_bundle_name, is_valid_peer_token};

use super::params::{
    ChangePskArgs, ChangeScopeArgs, ChooseParams, DropPeerArgs, LinkPeerArgs,
    ListArgs, ListNamespacesArgs, LookParams, NewPeerArgs, RawwParams,
    SendParams,
};

fn op_error(
    code: &str,
    message: &str,
    details: Option<Value>,
) -> OperationError {
    OperationError::new(code, message, details)
}

fn required_trimmed(
    value: Option<&str>,
    code: &str,
    message: &str,
    details: Option<Value>,
) -> Result<String, OperationError> {
    value
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .ok_or_else(|| op_error(code, message, details))
}

/// Maps `SendParams` into a `SendInput`. The message is validated
/// non-empty but carried verbatim; targets stay raw for the operation
/// to qualify against `bundle`.
pub(super) fn send_input(
    params: &SendParams,
    bundle: BundleScope,
) -> Result<SendInput, OperationError> {
    if params.message.trim().is_empty() {
        return Err(op_error(
            "validation_invalid_arguments",
            "message must be non-empty",
            None,
        ));
    }
    if params.broadcast && !params.targets.is_empty() {
        return Err(op_error(
            "validation_conflicting_targets",
            "targets must be empty when broadcast=true",
            None,
        ));
    }
    if !params.broadcast && params.targets.is_empty() {
        return Err(op_error(
            "validation_empty_targets",
            "provide at least one target or set broadcast=true",
            None,
        ));
    }
    Ok(SendInput {
        targets: params.targets.clone(),
        broadcast: params.broadcast,
        message: params.message.clone(),
        request_id: params.request_id.clone(),
        bundle,
    })
}

/// Maps `LookParams` into a `LookInput`. The target stays raw for the
/// operation to qualify.
pub(super) fn look_input(
    params: &LookParams,
) -> Result<LookInput, OperationError> {
    if params.target_session.trim().is_empty() {
        return Err(op_error(
            "validation_unknown_target",
            "target_session must be non-empty",
            None,
        ));
    }
    if let Some(lines) = params.lines
        && !(super::params::LOOK_LINES_MIN..=super::params::LOOK_LINES_MAX)
            .contains(&lines)
    {
        return Err(op_error(
            "validation_invalid_lines",
            "lines must be between 1 and 1000",
            Some(json!({
                "lines": lines,
                "min": super::params::LOOK_LINES_MIN,
                "max": super::params::LOOK_LINES_MAX,
            })),
        ));
    }
    Ok(LookInput {
        target: params.target_session.clone(),
        lines: params.lines,
        offset: params.offset,
    })
}

/// Maps `RawwParams` into a `RawwInput`. Emptiness of `text` is not
/// validated, matching the current contract.
pub(super) fn raww_input(
    params: &RawwParams,
) -> Result<RawwInput, OperationError> {
    if params.target_session.trim().is_empty() {
        return Err(op_error(
            "validation_unknown_target",
            "target_session must be non-empty",
            None,
        ));
    }
    Ok(RawwInput {
        target: params.target_session.clone(),
        text: params.text.clone(),
        no_enter: params.no_enter,
        request_id: params.request_id.clone(),
    })
}

/// Maps `ChooseParams` into a `DecideInput`. Ids are trimmed; the
/// outcome string selects the typed variant, with `cancelled`
/// forbidding even a whitespace-only `option_id` by presence.
pub(super) fn decide_input(
    params: &ChooseParams,
) -> Result<DecideInput, OperationError> {
    let choice_request_id = required_trimmed(
        params.choice_request_id.as_deref(),
        "validation_invalid_params",
        "choice_request_id must be a non-empty string",
        Some(json!({"field": "choice_request_id"})),
    )?;
    let outcome = required_trimmed(
        params.outcome.as_deref(),
        "validation_invalid_params",
        "outcome must be \"selected\" or \"cancelled\"",
        Some(json!({"field": "outcome"})),
    )?;
    match outcome.as_str() {
        super::params::CHOOSE_OUTCOME_SELECTED => {
            let option_id = required_trimmed(
                params.option_id.as_deref(),
                "validation_invalid_params",
                "selected outcome requires explicit non-empty option_id",
                Some(json!({
                    "field": "option_id",
                    "outcome": super::params::CHOOSE_OUTCOME_SELECTED,
                })),
            )?;
            Ok(DecideInput {
                choice_request_id,
                outcome: DecideOutcome::Selected { option_id },
            })
        },
        super::params::CHOOSE_OUTCOME_CANCELLED => {
            if params.option_id.is_some() {
                return Err(op_error(
                    "validation_invalid_params",
                    "cancelled outcome must omit option_id",
                    Some(json!({
                        "field": "option_id",
                        "outcome": super::params::CHOOSE_OUTCOME_CANCELLED,
                    })),
                ));
            }
            Ok(DecideInput {
                choice_request_id,
                outcome: DecideOutcome::Cancelled,
            })
        },
        other => Err(op_error(
            "validation_invalid_params",
            "outcome must be \"selected\" or \"cancelled\"",
            Some(json!({"field": "outcome", "value": other})),
        )),
    }
}

/// Maps `ListArgs` into a `ListPrincipalsInput`, preserving the exact
/// local/foreign selector semantics: empty trims to absent, `*` fans
/// out locally, concrete names are bundle-checked, and a set relay
/// requires one concrete namespace.
pub(super) fn list_principals_input(
    args: &ListArgs,
) -> Result<ListPrincipalsInput, OperationError> {
    let namespace = args
        .namespace
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match args.relay.as_deref().map(str::trim) {
        Some(relay) => {
            if relay.is_empty() || relay == "*" {
                return Err(op_error(
                    "validation_invalid_params",
                    "relay must be a configured peer alias, not empty or \"*\"",
                    Some(json!({"relay": args.relay})),
                ));
            }
            let Some(namespace) = namespace else {
                return Err(op_error(
                    "validation_invalid_params",
                    "namespace is required and must name one concrete namespace when relay is set",
                    Some(json!({"relay": relay})),
                ));
            };
            if namespace == "*"
                || matches!(namespace, "ALL" | "EXTERNAL" | "RELAY")
            {
                return Err(op_error(
                    "validation_invalid_params",
                    "foreign namespace must name one concrete namespace, not \"*\" or a reserved token",
                    Some(json!({"namespace": namespace})),
                ));
            }
            concrete_namespace(namespace)?;
            Ok(ListPrincipalsInput {
                namespace: BundleScope::Named(namespace.to_string()),
                relay: Some(relay.to_string()),
            })
        },
        None => {
            if let Some(namespace) = namespace
                && matches!(namespace, "ALL" | "EXTERNAL" | "RELAY")
            {
                return Err(op_error(
                    "validation_invalid_params",
                    "namespace must be a bundle name, \"GLOBAL\", or \"*\"",
                    Some(json!({"namespace": namespace})),
                ));
            }
            let namespace = match namespace {
                // Omitted selects the home bundle; `*` fans out.
                None => BundleScope::Home,
                Some("*") => BundleScope::All,
                Some(name) => {
                    if name != "GLOBAL" {
                        concrete_namespace(name)?;
                    }
                    BundleScope::Named(name.to_string())
                },
            };
            Ok(ListPrincipalsInput {
                namespace,
                relay: None,
            })
        },
    }
}

/// Maps `ListNamespacesArgs` into a `ListNamespacesInput`. The
/// carried selector is the trimmed value (whitespace-only is rejected
/// up front, so the empty filter never fires on validated input).
pub(super) fn list_namespaces_input(
    args: &ListNamespacesArgs,
) -> Result<ListNamespacesInput, OperationError> {
    if let Some(relay) = args.relay.as_deref().map(str::trim)
        && (relay.is_empty() || relay == "*")
    {
        return Err(op_error(
            "validation_invalid_params",
            "relay must be a configured peer alias, not empty or \"*\"",
            Some(json!({"relay": args.relay})),
        ));
    }
    // Normalized like the handler's request construction and echo:
    // trimmed, with whitespace-only treated as absent.
    Ok(ListNamespacesInput {
        relay: args
            .relay
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    })
}

fn concrete_namespace(namespace: &str) -> Result<(), OperationError> {
    if is_valid_bundle_name(namespace) && !matches!(namespace, "." | "..") {
        return Ok(());
    }
    Err(op_error(
        "validation_invalid_params",
        "namespace must be a well-formed bundle name",
        Some(json!({"namespace": namespace})),
    ))
}

/// Folds the mutually-exclusive `output_path` / `write_to_config`
/// sink arguments into a relay `CredentialDestination`, rejecting
/// both-supplied. Presence — not emptiness — drives exclusion, and an
/// explicitly supplied empty path is forwarded for the relay to
/// reject rather than degrading to `Response`.
fn credential_destination(
    output_path: Option<&str>,
    write_to_config: bool,
) -> Result<CredentialDestination, OperationError> {
    match (output_path, write_to_config) {
        (Some(_), true) => Err(op_error(
            "validation_invalid_params",
            "output_path and write_to_config are mutually exclusive",
            None,
        )),
        (Some(path), false) => Ok(CredentialDestination::Path {
            path: path.to_string(),
        }),
        (None, true) => Ok(CredentialDestination::Config),
        (None, false) => Ok(CredentialDestination::Response),
    }
}

/// Maps `NewPeerArgs` into a `NewPeerInput`, including the
/// relay-principal scope grammar pre-check.
pub(super) fn new_peer_input(
    args: &NewPeerArgs,
) -> Result<NewPeerInput, OperationError> {
    let normalized_principal =
        args.principal_id.as_deref().map(str::trim).unwrap_or("");
    if normalized_principal.ends_with("@RELAY")
        && !is_relay_principal_id(normalized_principal)
    {
        return Err(op_error(
            "validation_invalid_principal_id",
            "principal_id is not in <id>@<namespace> form",
            Some(json!({"field": "principal_id"})),
        ));
    }
    if let Some(scope) = args.scope.as_deref()
        && is_relay_principal_id(normalized_principal)
        && let Err(error) = parse_peer_scope(Some(scope))
    {
        return Err(op_error(
            error.code.as_str(),
            error.message.as_str(),
            Some(json!({"field": "scope"})),
        ));
    }
    let principal_id = required_trimmed(
        args.principal_id.as_deref(),
        "validation_invalid_params",
        "principal_id is required for new peer",
        None,
    )?;
    let scope = args
        .scope
        .as_ref()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    Ok(NewPeerInput {
        principal_id,
        scope,
        destination: credential_destination(
            args.output_path.as_deref(),
            args.write_to_config.unwrap_or(false),
        )?,
    })
}

/// Maps `ChangePskArgs` into a `ChangePskInput`.
pub(super) fn change_psk_input(
    args: &ChangePskArgs,
) -> Result<ChangePskInput, OperationError> {
    let principal_id = required_trimmed(
        args.principal_id.as_deref(),
        "validation_invalid_params",
        "principal_id is required for change psk",
        None,
    )?;
    Ok(ChangePskInput {
        principal_id,
        destination: credential_destination(
            args.output_path.as_deref(),
            args.write_to_config.unwrap_or(false),
        )?,
    })
}

/// Maps `ChangeScopeArgs` into a `ChangeScopeInput`. Checks run on the
/// trimmed principal id while the scope travels verbatim (empty clears;
/// omission is an error, never a clear).
pub(super) fn change_scope_input(
    args: &ChangeScopeArgs,
) -> Result<ChangeScopeInput, OperationError> {
    let principal_id = args
        .principal_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(principal_id) = principal_id else {
        return Err(op_error(
            "validation_invalid_params",
            "principal_id is required for change scope",
            Some(json!({"field": "principal_id"})),
        ));
    };
    if !is_relay_principal_id(principal_id) {
        return Err(op_error(
            "validation_invalid_principal_id",
            "change scope applies only to peer relay principals (<id>@RELAY)",
            Some(
                json!({"field": "principal_id", "principal_id": principal_id}),
            ),
        ));
    }
    let Some(scope) = args.scope.as_deref() else {
        return Err(op_error(
            "validation_invalid_params",
            "scope is required for change scope; pass an explicit empty string to clear the grant",
            Some(json!({"field": "scope"})),
        ));
    };
    if let Err(error) = parse_peer_scope(Some(scope)) {
        return Err(op_error(
            error.code.as_str(),
            error.message.as_str(),
            Some(json!({"field": "scope"})),
        ));
    }
    Ok(ChangeScopeInput {
        principal_id: principal_id.to_string(),
        scope: scope.to_string(),
    })
}

/// Maps `DropPeerArgs` into a `DropPeerInput`.
pub(super) fn drop_peer_input(
    args: &DropPeerArgs,
) -> Result<DropPeerInput, OperationError> {
    Ok(DropPeerInput {
        principal_id: required_trimmed(
            args.principal_id.as_deref(),
            "validation_invalid_params",
            "principal_id is required for drop peer",
            None,
        )?,
    })
}

/// Maps `LinkPeerArgs` into a `LinkPeerInput`. The alias is
/// token-checked (trimmed) then required; the PSK is required
/// non-empty but never trimmed or logged.
pub(super) fn link_peer_input(
    args: &LinkPeerArgs,
) -> Result<LinkPeerInput, OperationError> {
    let alias = args.alias.as_deref().map(str::trim).unwrap_or("");
    if !is_valid_peer_token(alias) {
        return Err(op_error(
            "validation_invalid_params",
            "alias is not a safe peer token",
            Some(json!({"field": "alias"})),
        ));
    }
    let alias = required_trimmed(
        args.alias.as_deref(),
        "validation_invalid_params",
        "alias is required for link peer",
        None,
    )?;
    let psk = args
        .psk
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            op_error(
                "validation_invalid_params",
                "psk is required for link peer",
                None,
            )
        })?
        .to_string();
    Ok(LinkPeerInput { alias, psk })
}

/// Mapping normalization tests: one test function by the inline-test
/// rule. The mappers are adapter-internal by design and unwired until
/// section 3.1, so no public path exercises them yet.
#[cfg(test)]
mod operation_mapping_tests {
    use super::super::params::{ListArgs, ListNamespacesArgs};
    use super::{list_namespaces_input, list_principals_input};
    use crate::operations::inputs::BundleScope;

    /// Selector normalization is representation-exact: surrounding
    /// whitespace trims, whitespace-only/omitted selects the local
    /// default, and empty/`*` relay selectors or non-concrete foreign
    /// namespaces fail with the handler-identical code.
    #[test]
    fn selector_mapping_normalizes_whitespace() {
        let namespaced = |namespace: Option<&str>, relay: Option<&str>| {
            list_principals_input(&ListArgs {
                namespace: namespace.map(str::to_string),
                relay: relay.map(str::to_string),
                ..Default::default()
            })
        };
        // Padded local bundle selects that bundle trimmed.
        let input =
            namespaced(Some("  b1  "), None).expect("padded bundle trims");
        assert_eq!(input.namespace, BundleScope::Named("b1".to_string()));
        assert_eq!(input.relay, None);
        // Padded foreign pair trims both sides.
        let input = namespaced(Some(" ns1 "), Some("  peer-a  "))
            .expect("padded foreign pair trims");
        assert_eq!(input.namespace, BundleScope::Named("ns1".to_string()));
        assert_eq!(input.relay, Some("peer-a".to_string()));
        // Omitted/blank namespace stays local; omitted relay stays local.
        let input = namespaced(None, None).expect("omitted stays local");
        assert_eq!(input.namespace, BundleScope::Home);
        let input = namespaced(Some(""), None).expect("blank stays local");
        assert_eq!(input.namespace, BundleScope::Home);
        // Star fans out; GLOBAL names itself.
        let input = namespaced(Some("*"), None).expect("star fans out");
        assert_eq!(input.namespace, BundleScope::All);
        let input =
            namespaced(Some("GLOBAL"), None).expect("GLOBAL names itself");
        assert_eq!(input.namespace, BundleScope::Named("GLOBAL".to_string()));
        // Reserved and missing foreign namespaces fail like the handler.
        for namespace in ["ALL", "*", ""] {
            let error = namespaced(Some(namespace), Some("peer-a"))
                .expect_err("reserved namespace must fail");
            assert_eq!(error.code, "validation_invalid_params");
        }
        let error = namespaced(None, Some("peer-a"))
            .expect_err("missing foreign namespace must fail");
        assert_eq!(error.code, "validation_invalid_params");
        for relay in ["", "*"] {
            let error = namespaced(Some("ns1"), Some(relay))
                .expect_err("bad relay selector must fail");
            assert_eq!(error.code, "validation_invalid_params");
        }

        let relayed = |relay: Option<&str>| {
            list_namespaces_input(&ListNamespacesArgs {
                relay: relay.map(str::to_string),
                ..Default::default()
            })
        };
        // Padded relay trims; omitted stays local.
        let input = relayed(Some("  peer-a  ")).expect("padded relay trims");
        assert_eq!(input.relay, Some("peer-a".to_string()));
        let input = relayed(None).expect("omitted relay stays local");
        assert_eq!(input.relay, None);
        // Empty and star relay selectors fail like the handler.
        for relay in ["", "*"] {
            let error = relayed(Some(relay))
                .expect_err("bad relay selector must fail");
            assert_eq!(error.code, "validation_invalid_params");
        }
    }
}
