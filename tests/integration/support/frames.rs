//! Cross-lane frame-shape filters for adapter integration tests.
//!
//! Structural and binding refusals are written synchronously inline
//! in the dispatch path (data lane) while submit terminals and
//! requestChoice travel the async mailbox -> executor ->
//! control-first writer. With preloaded input the writer may serve
//! queued control before it drains held data, so exact wire indices
//! pin one schedule. These helpers assert the contract-derived
//! shape instead: hello first, then filtered counts by kind, code,
//! outcome, and correlation.

use serde_json::{Value, json};

/// Frames carrying a JSON-RPC error with the given contract `code`
/// in `error.data.code` (e.g. `validation_structural`,
/// `binding_mismatch`), in wire order.
pub(crate) fn frames_with_error<'a>(
    frames: &'a [Value],
    code: &str,
) -> Vec<&'a Value> {
    frames
        .iter()
        .filter(|frame| {
            frame.get("error").and_then(|error| error.get("code"))
                == Some(&json!(-32000))
                && frame.get("error").and_then(|error| {
                    error.get("data").and_then(|data| data.get("code"))
                }) == Some(&json!(code))
        })
        .collect()
}

/// Frames carrying a terminal `result.outcome` (e.g. `Submitted`,
/// `RefusedBusy`), in wire order.
pub(crate) fn frames_with_outcome<'a>(
    frames: &'a [Value],
    outcome: &str,
) -> Vec<&'a Value> {
    frames
        .iter()
        .filter(|frame| {
            frame.get("result").and_then(|result| result.get("outcome"))
                == Some(&json!(outcome))
        })
        .collect()
}

/// Frames carrying the given `method` (e.g.
/// `adapter.requestChoice`), in wire order.
pub(crate) fn frames_with_method<'a>(
    frames: &'a [Value],
    method: &str,
) -> Vec<&'a Value> {
    frames
        .iter()
        .filter(|frame| frame.get("method") == Some(&json!(method)))
        .collect()
}

/// Asserts exactly one lifecycle event of the given kind (e.g.
/// `ready`) is present — ready/drain events are control-lane, so
/// only presence and kind pin, never the index.
pub(crate) fn assert_lifecycle_kind(frames: &[Value], kind: &str) {
    let kinds: Vec<&Value> =
        frames_with_method(frames, "adapter.lifecycleEvent")
            .into_iter()
            .filter(|frame| frame["params"]["kind"] == kind)
            .collect();
    assert_eq!(kinds.len(), 1, "one {kind} event: {frames:?}");
}

/// Sorted `result.correlation_id`s for frames with the given terminal
/// outcome, for order-independent busy/terminal correlation checks.
pub(crate) fn outcome_correlations(
    frames: &[Value],
    outcome: &str,
) -> Vec<String> {
    let mut ids: Vec<String> = frames_with_outcome(frames, outcome)
        .iter()
        .map(|frame| {
            frame["result"]["correlation_id"]
                .as_str()
                .unwrap_or("?")
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

/// Asserts first-appearance order of two methods on the wire
/// (same-lane FIFO pins it). Both methods must be present: absence
/// panics rather than comparing a missing position.
pub(crate) fn assert_method_order(frames: &[Value], first: &str, then: &str) {
    let position = |method: &str| -> usize {
        frames
            .iter()
            .position(|frame| frame.get("method") == Some(&json!(method)))
            .unwrap_or_else(|| {
                panic!("expected {method} on the wire: {frames:?}")
            })
    };
    assert!(
        position(first) < position(then),
        "{first} must precede {then}: {frames:?}"
    );
}
