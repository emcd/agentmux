//! Canonical `look` operation: validated, principal-authorized snapshot
//! inspection through the public relay dispatch spine.
//!
//! This moves the MCP `look` handler body here: the same validation
//! order, the same inscription emission, and the same outcome
//! interpretation — minus association, which stays adapter-side.
//!
//! Available only under `experimental-public-send`, like the dispatch
//! boundary it calls.

use std::sync::Arc;

use serde_json::json;

use crate::relay::{
    DispatchWiring, LookOperation, RelayResponse, RelayRuntime,
    VerifiedPrincipal, dispatch_look,
};

use super::common::{emit_inscription_async, operation_bundle_for};
use super::error::OperationError;
use super::inputs::{BundleScope, LookInput};
use super::outputs::LookOutput;

/// Canonical `look` operation: validated, principal-authorized
/// snapshot inspection through the public relay dispatch spine.
///
/// Construct once per owning runtime from its sealed dispatch
/// wiring; invoke per verified principal and typed input. The struct
/// holds shared handles to runtime-owned relay machinery, not a
/// server: it cannot act without a per-invocation principal, and it
/// performs no authorization of its own beyond input validation —
/// relay remains the decision point.
#[derive(Clone)]
#[cfg(feature = "experimental-public-send")]
pub struct LookTool {
    runtime: Arc<RelayRuntime>,
    wiring: DispatchWiring,
}

#[cfg(feature = "experimental-public-send")]
impl std::fmt::Debug for LookTool {
    // Handles are never observable without acquiring them; formatting
    // under a held lock could deadlock a thread partway through an
    // inspect. Surface the structural placeholder only.
    fn fmt(
        &self,
        formatter: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        formatter.debug_struct("LookTool").finish_non_exhaustive()
    }
}

#[cfg(feature = "experimental-public-send")]
impl LookTool {
    /// Binds the operation to one owning runtime through previously
    /// installed domain wiring (serve clone, Backend once-per-runtime
    /// constructor, or the mirrored test path). No catalog or peer
    /// manager is accepted here: the pair arrives sealed inside the
    /// wiring, and mismatch with the runtime fails closed at
    /// dispatch via the brand check.
    pub fn new(runtime: Arc<RelayRuntime>, wiring: DispatchWiring) -> Self {
        Self { runtime, wiring }
    }

    /// Inspects a target session's latest snapshot — tmux pane lines
    /// or ACP structured replay — authorized by relay-verified
    /// principal context.
    ///
    /// Validation runs in handler order (target shape, then window
    /// bounds, then bundle scope) before anything touches the relay.
    /// Relay-authored denials enter the canonical error unchanged; an
    /// unexpected response variant is an internal failure.
    /// Socket-transport failures cannot arise: dispatch stays
    /// in-process and never touches the adapter's
    /// `RelayStreamSession`.
    pub async fn invoke(
        &self,
        principal: &VerifiedPrincipal,
        input: LookInput,
    ) -> Result<LookOutput, OperationError> {
        validate_look(&input)?;
        emit_inscription_async(
            "mcp.tool.look.request",
            json!({
                "namespace": principal.home_namespace(),
                "requester_session": principal.principal_id(),
                "target_session": input.target,
                "lines": input.lines,
                "offset": input.offset,
            }),
        )
        .await;
        // Look has no explicit bundle parameter (like its MCP
        // counterpart, which always binds the associated bundle):
        // bare targets fuse against the principal home, qualified
        // targets pass through. Relay-wide homes pass no bundle so
        // dispatch applies its unqualified-target rejection.
        let operation_bundle = operation_bundle_for(
            "look",
            principal.home_namespace(),
            &BundleScope::Home,
        )?;
        let operation = LookOperation {
            target_session: input.target.clone(),
            lines: input.lines.map(|value| value as usize),
            offset: input.offset.map(|value| value as usize),
            operation_bundle,
        };
        // The synchronous dispatch runs on the blocking pool so the
        // executor stays free; dropping the invoke future detaches
        // the waiter without recalling a dispatched look.
        let runtime = Arc::clone(&self.runtime);
        let wiring = self.wiring.clone();
        let principal = principal.clone();
        let response = tokio::task::spawn_blocking(move || {
            dispatch_look(&runtime, &principal, &wiring, operation)
        })
        .await
        .map_err(|source| {
            OperationError::new(
                "internal_unexpected_failure",
                "look dispatch task failed to join",
                Some(json!({ "cause": source.to_string() })),
            )
        })?;
        match response {
            RelayResponse::Look {
                schema_version,
                requester_session,
                target_session,
                captured_at,
                authenticated_identity,
                on_behalf_of,
                snapshot,
            } => {
                let (snapshot_format, snapshot_count) = match &snapshot {
                    crate::relay::LookSnapshotPayload::Lines {
                        snapshot_lines,
                    } => ("lines", snapshot_lines.len()),
                    crate::relay::LookSnapshotPayload::StructuredEntriesV1 {
                        snapshot_entries,
                        ..
                    } => ("structured_entries_v1", snapshot_entries.len()),
                };
                emit_inscription_async(
                    "mcp.tool.look.success",
                    json!({
                        "requester_session": requester_session,
                        "target_session": target_session,
                        "snapshot_format": snapshot_format,
                        "snapshot_count": snapshot_count,
                    }),
                )
                .await;
                Ok(LookOutput {
                    schema_version,
                    requester_session,
                    target_session,
                    captured_at,
                    authenticated_identity,
                    on_behalf_of,
                    snapshot,
                })
            },
            RelayResponse::Error { error } => {
                emit_inscription_async(
                    "mcp.tool.look.relay_error",
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
                    "mcp.tool.look.unexpected_response",
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
/// and messages the MCP handler produces today. Offset range checks
/// stay relay-side (per-transport gates); unknown-field rejection
/// stays at the JSON boundary in the adapter.
fn validate_look(input: &LookInput) -> Result<(), OperationError> {
    if input.target.trim().is_empty() {
        return Err(OperationError::new(
            "validation_unknown_target",
            "target_session must be non-empty",
            None,
        ));
    }
    if let Some(lines) = input.lines
        && !(1..=1000).contains(&lines)
    {
        return Err(OperationError::new(
            "validation_invalid_lines",
            "lines must be between 1 and 1000",
            Some(json!({ "lines": lines, "min": 1, "max": 1000 })),
        ));
    }
    Ok(())
}
