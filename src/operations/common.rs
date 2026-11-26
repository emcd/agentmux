//! Shared operation plumbing: async inscription emission off the
//! executor, and the bundle-scope mapping every operation applies.
//!
//! Both helpers are protocol-neutral and carry no MCP types. The mapping
//! keeps the relay-wide GLOBAL carve-out in one place so no operation
//! can silently turn a forbidden home fallback into an explicit
//! routing default.

use serde_json::Value;

use super::error::OperationError;
use super::inputs::BundleScope;
use crate::runtime::inscriptions::emit_inscription;

/// Maps an operation bundle scope to the dispatch operation-bundle
/// parameter. `Home` resolves into the concrete principal-home
/// scope (matching the MCP bound-bundle default, including for
/// broadcast-capable operations); `Named` passes through. A
/// relay-wide GLOBAL home never defaults bare targets — dispatch
/// applies its unqualified and parameter-required rejections
/// instead. `All` names no single scope and is rejected rather than
/// silently defaulted.
pub(super) fn operation_bundle_for(
    op: &str,
    home_namespace: &str,
    bundle: &BundleScope,
) -> Result<Option<String>, OperationError> {
    match bundle {
        BundleScope::Home if home_namespace == "GLOBAL" => Ok(None),
        BundleScope::Home => Ok(Some(home_namespace.to_string())),
        BundleScope::Named(scope) => Ok(Some(scope.clone())),
        BundleScope::All => Err(OperationError::new(
            "validation_invalid_arguments",
            &format!(
                "{op} requires a single bundle scope: home or a named bundle"
            ),
            None,
        )),
    }
}

/// Emits one diagnostic inscription without parking the executor: the
/// sink performs synchronous filesystem I/O (directory creation, open
/// in append mode, write), so it runs on the blocking pool. Awaited
/// in order, request before dispatch and outcome after, preserving the
/// handler emission sequence. Fire-and-forget like the sync call: a
/// failed write is dropped, never an operation error; abandoning the
/// invoke future between awaits simply skips later events, which
/// reports nothing untrue.
pub(super) async fn emit_inscription_async(
    event: &'static str,
    details: Value,
) {
    let _ = tokio::task::spawn_blocking(move || {
        emit_inscription(event, &details);
    })
    .await;
}
