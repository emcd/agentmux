//! Canonical `send` operation: validated, principal-authorized message
//! delivery through the public relay dispatch spine.
//!
//! This moves the MCP `send` handler body here: the same validation
//! order, the same inscription emission, and the same outcome
//! interpretation — minus association, which stays adapter-side. The
//! handler keeps working unchanged until the adapter is reduced to
//! delegate to this operation; when that happens the handler must
//! drop its own `mcp.tool.send.*` emissions (they live here now)
//! rather than emit twice.
//!
//! Available only under `experimental-public-send`, like the dispatch
//! boundary it calls.

use serde_json::json;

use crate::configuration::effective_bundle_definitions;
use crate::relay::{
    BundleCatalog, PeerConnectionManager, RelayResponse, RelayRuntime,
    SendOperation, VerifiedPrincipal, dispatch_send,
    load_relay_runtime_configuration,
};
use crate::runtime::inscriptions::emit_inscription;
use crate::runtime::paths::BundleRuntimePaths;

use super::error::OperationError;
use super::inputs::{BundleScope, SendInput};
use super::outputs::SendOutput;

/// Sends a message to explicit targets or broadcasts it to a bundle
/// scope, authorized by relay-verified principal context.
///
/// Validation runs in handler order (message shape, then
/// broadcast/target coherence, then bundle scope) before anything
/// touches the relay. The relay request carries a minted correlation
/// id for relay-side tracing; the caller-visible `request_id` is the
/// input's echo, matching the MCP contract. Relay-authored denials
/// enter the canonical error unchanged; an unexpected response
/// variant is an internal failure. Socket-transport failures cannot
/// arise: dispatch stays in-process and never touches the adapter's
/// `RelayStreamSession`.
pub async fn send(
    runtime: &RelayRuntime,
    principal: &VerifiedPrincipal,
    input: SendInput,
) -> Result<SendOutput, OperationError> {
    validate_send(&input)?;
    emit_inscription(
        "mcp.tool.send.request",
        &json!({
            "namespace": principal.home_namespace(),
            "request_id": input.request_id,
            "targets": input.targets,
            "broadcast": input.broadcast,
            "message_length": input.message.len(),
        }),
    );
    let operation_bundle = match &input.bundle {
        // The canonical home resolves into the concrete principal-home
        // scope, matching the MCP bound-bundle default — including for
        // broadcast, which requires an explicit scope at the dispatch
        // boundary. Relay-wide homes are the exception, mirroring the
        // dispatch layer's own rule (qualify_targets_for_dispatch
        // never defaults bare targets for the GLOBAL namespace): a
        // bare name fused with a GLOBAL home would classify as a
        // relay-wide user and silently become an explicit routing
        // default, so GLOBAL homes stay parameter-less and dispatch
        // applies its unqualified-target and parameter-required
        // rejections instead.
        BundleScope::Home if principal.home_namespace() == "GLOBAL" => None,
        BundleScope::Home => Some(principal.home_namespace().to_string()),
        BundleScope::Named(scope) => Some(scope.clone()),
        // Send fans out within a single bundle scope (the dispatch
        // layer expands one scope bundle per broadcast); `All` names
        // no such scope, so it is a validation failure here rather
        // than a silent home default.
        BundleScope::All => {
            return Err(OperationError::new(
                "validation_invalid_arguments",
                "send requires a single bundle scope: home or a named bundle",
                None,
            ));
        },
    };
    let operation = SendOperation {
        targets: input.targets.clone(),
        message: input.message.clone(),
        broadcast: input.broadcast,
        operation_bundle,
    };
    let (catalog, peer_manager) = dispatch_auxiliaries(runtime)?;
    match dispatch_send(runtime, principal, &catalog, &peer_manager, operation)
    {
        RelayResponse::Send {
            schema_version,
            requester_session,
            sender_display_name,
            authenticated_identity,
            on_behalf_of,
            results,
            ..
        } => {
            emit_inscription(
                "mcp.tool.send.success",
                &json!({ "result_count": results.len() }),
            );
            Ok(SendOutput {
                schema_version,
                request_id: input.request_id,
                requester_session,
                sender_display_name,
                authenticated_identity,
                on_behalf_of,
                results,
            })
        },
        RelayResponse::Error { error } => {
            emit_inscription(
                "mcp.tool.send.relay_error",
                &json!({
                    "code": error.code.clone(),
                    "message": error.message.clone(),
                    "details": error.details.clone(),
                }),
            );
            Err(OperationError::from_relay(&error))
        },
        other => {
            emit_inscription(
                "mcp.tool.send.unexpected_response",
                &json!({ "response": other }),
            );
            Err(OperationError::new(
                "internal_unexpected_failure",
                "relay returned unexpected response variant",
                Some(json!({ "response": other })),
            ))
        },
    }
}

/// Handler-order validation on the typed input, with the exact codes
/// and messages the MCP handler produces today (unknown-field
/// rejection has no typed-input equivalent and stays at the JSON
/// boundary in the adapter).
fn validate_send(input: &SendInput) -> Result<(), OperationError> {
    if input.message.trim().is_empty() {
        return Err(OperationError::new(
            "validation_invalid_arguments",
            "message must be non-empty",
            None,
        ));
    }
    if input.broadcast && !input.targets.is_empty() {
        return Err(OperationError::new(
            "validation_conflicting_targets",
            "targets must be empty when broadcast=true",
            None,
        ));
    }
    if !input.broadcast && input.targets.is_empty() {
        return Err(OperationError::new(
            "validation_empty_targets",
            "provide at least one target or set broadcast=true",
            None,
        ));
    }
    Ok(())
}

/// Builds the ephemeral dispatch auxiliaries from the runtime roots,
/// mirroring the socket path's per-request catalog: every configured
/// bundle stays addressable (cross-bundle fan-out needs more than the
/// home bundle), and the peer manager reads the configured peers.
/// Failures here are environment faults, reported as internal
/// failures with the source preserved in details.
fn dispatch_auxiliaries(
    runtime: &RelayRuntime,
) -> Result<(BundleCatalog, PeerConnectionManager), OperationError> {
    let roots = runtime.configuration_roots();
    let state_root = runtime.state_root();
    let definitions =
        effective_bundle_definitions(roots).map_err(|source| {
            OperationError::new(
                "internal_unexpected_failure",
                "bundle enumeration failed",
                Some(json!({ "error": source.to_string() })),
            )
        })?;
    let mut paths = Vec::with_capacity(definitions.len());
    for bundle in definitions.keys() {
        match BundleRuntimePaths::resolve(state_root, bundle) {
            Ok(resolved) => paths.push(resolved),
            Err(source) => {
                return Err(OperationError::new(
                    "internal_unexpected_failure",
                    "bundle path resolution failed",
                    Some(json!({ "error": source.to_string() })),
                ));
            },
        }
    }
    let catalog = BundleCatalog::from_paths(paths);
    let peers = load_relay_runtime_configuration(roots, None, None)
        .map_err(|source| {
            OperationError::new(
                "internal_unexpected_failure",
                "peer configuration failed to load",
                Some(json!({
                    "code": source.code.clone(),
                    "message": source.message.clone(),
                })),
            )
        })?
        .peers;
    let peer_manager =
        PeerConnectionManager::from_configuration(state_root, &peers);
    Ok((catalog, peer_manager))
}
