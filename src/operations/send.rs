//! Canonical `send` operation: validated, principal-authorized message
//! delivery through the public relay dispatch spine.
//!
//! The operation is a struct holding the public runtime handle and the
//! sealed domain-bound dispatch wiring, with an async invoke method
//! taking the relay-verified principal and the typed input. The wiring
//! is sourced once per owning runtime (serve clone, Backend
//! once-per-runtime constructor, or the mirrored constructor in
//! tests) — never rebuilt per call, never probed from the
//! filesystem by this layer.
//!
//! Dispatch runs on the blocking pool: the synchronous relay spine
//! (including potentially slow peer forwarding) must never park an
//! async executor thread. Validation and inscription emission happen
//! up front in invoke; the outcome is interpreted after the join.
//!
//! Available only under `experimental-public-send`, like the dispatch
//! boundary it calls.

use std::sync::Arc;

use serde_json::json;

use crate::relay::{
    DispatchWiring, RelayResponse, RelayRuntime, SendOperation,
    VerifiedPrincipal, dispatch_send,
};

use super::common::{emit_inscription_async, operation_bundle_for};
use super::error::OperationError;
use super::inputs::SendInput;
use super::outputs::SendOutput;

/// Canonical `send` operation: validated, principal-authorized
/// message delivery through the public relay dispatch spine.
///
/// Construct once per owning runtime from its sealed dispatch
/// wiring; invoke per verified principal and typed input. The struct
/// holds shared handles to runtime-owned relay machinery, not a
/// server: it cannot act without a per-invocation principal, and it
/// performs no authorization of its own beyond input validation —
/// relay remains the decision point.
#[derive(Clone)]
#[cfg(feature = "experimental-public-send")]
pub struct SendTool {
    runtime: Arc<RelayRuntime>,
    wiring: DispatchWiring,
}

#[cfg(feature = "experimental-public-send")]
impl std::fmt::Debug for SendTool {
    // Handles are never observable without acquiring them, and
    // formatting while held could deadlock a thread partway through
    // an inspect. Surface the structural placeholder only.
    fn fmt(
        &self,
        formatter: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        formatter.debug_struct("SendTool").finish_non_exhaustive()
    }
}

#[cfg(feature = "experimental-public-send")]
impl SendTool {
    /// Binds the operation to one owning runtime through previously
    /// installed domain wiring (serve clone, Backend once-per-runtime
    /// constructor, or the mirrored test path). No catalog or peer
    /// manager is accepted here: the pair arrives sealed inside the
    /// wiring, and mismatch with the runtime fails closed at
    /// dispatch via the brand check.
    pub fn new(runtime: Arc<RelayRuntime>, wiring: DispatchWiring) -> Self {
        Self { runtime, wiring }
    }

    /// Sends a message to explicit targets or broadcasts it to a bundle
    /// scope, authorized by relay-verified principal context.
    ///
    /// Validation runs in handler order (message shape, then
    /// broadcast/target coherence, then bundle scope) before anything
    /// touches the relay. The relay request carries a minted
    /// correlation id for relay-side tracing; the caller-visible
    /// `request_id` is the input's echo, matching the MCP contract.
    /// Relay-authored denials enter the canonical error unchanged; an
    /// unexpected response variant is an internal failure.
    /// Socket-transport failures cannot arise: dispatch stays
    /// in-process and never touches the adapter's
    /// `RelayStreamSession`.
    pub async fn invoke(
        &self,
        principal: &VerifiedPrincipal,
        input: SendInput,
    ) -> Result<SendOutput, OperationError> {
        validate_send(&input)?;
        emit_inscription_async(
            "mcp.tool.send.request",
            json!({
                "namespace": principal.home_namespace(),
                "request_id": input.request_id,
                "targets": input.targets,
                "broadcast": input.broadcast,
                "message_length": input.message.len(),
            }),
        )
        .await;
        let operation_bundle = operation_bundle_for(
            "send",
            principal.home_namespace(),
            &input.bundle,
        )?;
        let operation = SendOperation {
            targets: input.targets.clone(),
            message: input.message.clone(),
            broadcast: input.broadcast,
            operation_bundle,
        };
        // The synchronous dispatch (including potentially slow peer
        // forwarding) runs on the blocking pool so the executor
        // stays free; dropping the invoke future detaches the
        // waiter without recalling a dispatched send.
        let runtime = Arc::clone(&self.runtime);
        let wiring = self.wiring.clone();
        let principal = principal.clone();
        let response = tokio::task::spawn_blocking(move || {
            dispatch_send(&runtime, &principal, &wiring, operation)
        })
        .await
        .map_err(|source| {
            OperationError::new(
                "internal_unexpected_failure",
                "send dispatch task failed to join",
                Some(json!({ "cause": source.to_string() })),
            )
        })?;
        match response {
            RelayResponse::Send {
                schema_version,
                requester_session,
                sender_display_name,
                authenticated_identity,
                on_behalf_of,
                results,
                ..
            } => {
                emit_inscription_async(
                    "mcp.tool.send.success",
                    json!({ "result_count": results.len() }),
                )
                .await;
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
                emit_inscription_async(
                    "mcp.tool.send.relay_error",
                    json!({
                        "code": error.code.clone(),
                        "message": error.message.clone(),
                        "details": error.details.clone(),
                    }),
                )
                .await;
                Err(OperationError::from_relay(&error))
            },
            other => {
                emit_inscription_async(
                    "mcp.tool.send.unexpected_response",
                    json!({ "response": other }),
                )
                .await;
                Err(OperationError::new(
                    "internal_unexpected_failure",
                    "relay returned unexpected response variant",
                    Some(json!({ "response": other })),
                ))
            },
        }
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
