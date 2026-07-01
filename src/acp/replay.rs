//! Replay-buffer maintenance helpers for the ACP transport.
//!
//! This module exists primarily so the unit and integration tests under
//! `tests/unit/acp/` and `tests/integration/acp/` can exercise the
//! replay-buffer coalescence, cap-maintenance, and parser logic at the
//! helper level without resorting to inline `#[cfg(test)]` modules in
//! `src/acp/`. Visibility is `pub` for that reason.
//!
//! This is **not** a load-bearing public API contract. The standalone
//! ACP test-harness binary (`src/bin/agentmux_acp.rs`) reaches the
//! replay buffer only through the high-level `AcpStdioClient` API
//! (`load_session`, `replay_entries_since`); it does not consume any
//! of these helpers directly, and no production code path in `src/acp/`
//! outside the helpers themselves depends on them being `pub`. Treat
//! the surface here as a test-reach seam: a future consumer (for
//! example, a Pty-side replay bridge or a replay-introspection MCP
//! tool) could use it, but there is no current commitment to keep the
//! surface stable on that basis alone. If a new helper is being
//! considered here, prefer first whether the test can be rewritten
//! against `AcpStdioClient`'s observable API; only fall back to a new
//! helper when the behavior is genuinely internal and not reachable
//! from outside the buffer-management code paths.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::runtime::inscriptions::emit_inscription;
use crate::transports::ToolCallStatus;

use super::{PendingToolCall, ReplayEntry, UserSource};

pub const REPLAY_BUFFER_MAX_ENTRIES: usize = 1000;

/// Non-coalescing replay-buffer append. Reserved for the prompt path
/// (`AcpStdioClient::prompt`): each operator submission becomes its own
/// `ReplayEntry::User` regardless of any preceding `User` tail, so that two
/// back-to-back prompts remain two distinct entries (per-call / per-submission
/// boundary must be preserved; the operator aggregation principle says we
/// only ever merge messages that actually arrive as streaming deltas, and
/// user prompts are delivered whole).
///
/// Reader-thread ingestion from the ACP server uses
/// `coalesce_replay_entries_on_append` instead, which performs same-kind
/// adjacency coalescence for streaming User/Agent/Cognition/Update kinds.
///
/// After the append, the helper enforces the 1000-entry cap via
/// `enforce_replay_buffer_cap_and_maintain_positions` so a prompt-path
/// append that trips the cap cannot leave `pending_tool_calls` with stale
/// `buffer_position` values. The reader-thread parser calls the same
/// cap-maintain helper at the end of its ingest pass; both paths share
/// the helper to keep the invariant "every remaining pending entry's
/// recorded position points to a valid Invocation" atomic with cap drain.
pub fn append_replay_entries(
    buffer: &mut Vec<ReplayEntry>,
    pending_calls: &mut HashMap<String, PendingToolCall>,
    entries: Vec<ReplayEntry>,
) {
    buffer.extend(entries);
    enforce_replay_buffer_cap_and_maintain_positions(buffer, pending_calls);
}

/// Coalescing replay-buffer append for the reader-thread ingestion path
/// (`AcpStdioClient::dispatch_session_update`). Walks `new_entries` and
/// merges adjacent same-kind entries with the buffer tail, preserving all
/// line content in receive order.
///
/// Coalescence rules:
/// - `User`, `Agent`, `Cognition`: adjacent same-kind entries merge into one
///   entry by `lines: Vec<String>` extension. `User` merges only when both
///   entries share the same `UserSource`; cross-source adjacency
///   (prompt-path tail + reader-thread arrival, or vice versa) does not
///   merge.
/// - `Update`: adjacent entries merge only when their `update_kind` is
///   identical; the merge extends `lines` and preserves the shared
///   `update_kind`.
/// - `Invocation`: NEVER merges with an adjacent entry (per-call boundary
///   must be preserved; the parser-side `tool_call` + `tool_call_update`
///   replace-in-place mechanism coalesces a single call's result onto the
///   same entry, by `call_id`, independent of buffer position).
/// - Different-kind adjacency never merges.
///
/// The helper walks `new_entries` once. When the buffer tail and the next
/// `new_entry` are coalescible per the rules above, the helper extends the
/// tail's `lines` in place; otherwise the entry is pushed. The
/// within-notification-multi-entry case (the wire may carry multiple
/// entries per `session/update` notification via `params.update` as a JSON
/// array) is covered naturally by the same walk: consecutive entries in
/// `new_entries` are checked against each other and the tail.
///
/// The 1000-entry cap is NO LONGER drained here. The reader-thread ingestion
/// path now calls `enforce_replay_buffer_cap_and_maintain_positions` once at
/// the end of the parser's ingest pass so cap enforcement and the
/// recorded-position adjustment for `PendingToolCall` happen as one atomic
/// operation. The prompt path uses the simpler `append_replay_entries`
/// helper, which also routes through the same cap-maintain helper so a
/// prompt-path overflow cannot leave the parser-side `pending_tool_calls`
/// map with stale `buffer_position` values (the position maintenance is
/// idempotent on an empty map).
pub fn coalesce_replay_entries_on_append(
    buffer: &mut Vec<ReplayEntry>,
    new_entries: Vec<ReplayEntry>,
) {
    for new_entry in new_entries {
        if let Some(tail) = buffer.last_mut()
            && try_merge_adjacent(tail, &new_entry)
        {
            continue;
        }
        buffer.push(new_entry);
    }
}

/// Enforces the 1000-entry replay-buffer cap and atomically adjusts every
/// recorded `PendingToolCall::buffer_position` to remain valid after the
/// drain. The drain removes the oldest entries (positions `0..overflow`),
/// so every remaining position must shift down by `overflow`. Any pending
/// whose recorded position fell below `overflow` had its Pending
/// Invocation evicted; that pending is removed from the map entirely.
///
/// The atomicity is the contract: callers must run this helper (a) AFTER
/// coalescence has settled, and (b) BEFORE handing the buffer back to a
/// consumer. The parser's quiescent invariant -- every remaining
/// `pending_calls[call_id].buffer_position` points to a valid Invocation
/// entry with matching `call_id` in the buffer -- holds immediately after
/// this call returns, and the helper is the only code path that evicts
/// from the buffer front (the prompt path's `append_replay_entries` is a
/// separate non-coalescing path with its own cap drain).
pub fn enforce_replay_buffer_cap_and_maintain_positions(
    buffer: &mut Vec<ReplayEntry>,
    pending_calls: &mut HashMap<String, PendingToolCall>,
) {
    if buffer.len() <= REPLAY_BUFFER_MAX_ENTRIES {
        return;
    }
    let overflow = buffer.len() - REPLAY_BUFFER_MAX_ENTRIES;
    buffer.drain(0..overflow);
    pending_calls.retain(|_, pending| {
        if pending.buffer_position < overflow {
            false
        } else {
            pending.buffer_position -= overflow;
            true
        }
    });
}

/// Returns `true` if `new_entry` was merged into `tail` in place (the caller
/// should NOT push a new entry). Returns `false` if the caller should push
/// `new_entry` as-is. Pass-by-reference with internal clone of merge-target
/// fields; the merge cost is `O(new_entry's lines)` per mergeable pair.
fn try_merge_adjacent(tail: &mut ReplayEntry, new_entry: &ReplayEntry) -> bool {
    use ReplayEntry::{
        Agent as AgentEntry, Cognition as CognitionEntry, Update as UpdateEntry, User as UserEntry,
    };
    match (tail, new_entry) {
        (
            UserEntry {
                lines: tail_lines,
                source: tail_source,
            },
            UserEntry {
                lines: new_lines,
                source: new_source,
            },
        ) if tail_source == new_source => {
            tail_lines.extend(new_lines.iter().cloned());
            true
        }
        (AgentEntry { lines: tail_lines }, AgentEntry { lines: new_lines }) => {
            tail_lines.extend(new_lines.iter().cloned());
            true
        }
        (CognitionEntry { lines: tail_lines }, CognitionEntry { lines: new_lines }) => {
            tail_lines.extend(new_lines.iter().cloned());
            true
        }
        (
            UpdateEntry {
                update_kind: tail_kind,
                lines: tail_lines,
            },
            UpdateEntry {
                update_kind: new_kind,
                lines: new_lines,
            },
        ) if tail_kind == new_kind => {
            tail_lines.extend(new_lines.iter().cloned());
            true
        }
        // Invocation never merges with adjacent entries, regardless of
        // identity (the per-call boundary must be preserved). Different-kind
        // pairs never merge. Update entries with different `update_kind`
        // never merge.
        _ => false,
    }
}

/// Parses an ACP `session/update` notification's `params.update` payload
/// into the replay buffer with full buffer-awareness: each entry is
/// applied in wire order, the per-`call_id` `PendingToolCall` map is
/// updated, and the 1000-entry cap is enforced atomically at the end of
/// the pass.
///
/// The function is the entry point of the reader-thread ingestion path;
/// `AcpStdioClient::dispatch_session_update` calls it under both
/// `replay_buffer` and `pending_tool_calls` locks (in that order).
///
/// After the loop, `enforce_replay_buffer_cap_and_maintain_positions` runs
/// once: it drains the buffer to the cap and adjusts every recorded
/// position down by the drain count, removing pendings whose Pending
/// Invocations were evicted. The parser's quiescent invariant -- every
/// remaining `pending_calls[call_id].buffer_position` points to a valid
/// Invocation entry with matching `call_id` in the buffer -- holds
/// immediately after this call returns.
pub fn parse_replay_entries_from_params(
    params: &Value,
    pending_calls: &mut HashMap<String, PendingToolCall>,
    buffer: &mut Vec<ReplayEntry>,
) {
    let update_field = params.get("update").unwrap_or(&Value::Null);
    let updates: Vec<&Value> = match update_field.as_array() {
        Some(arr) => arr.iter().collect(),
        None if !update_field.is_null() => vec![update_field],
        None => return,
    };
    // Wire-order preservation: each entry in `updates` is processed in
    // array order. Non-tool-call entries (User/Agent/Cognition/Update)
    // are appended via the coalesce helper immediately so they land in
    // the buffer in the order the wire sent them; tool_call entries are
    // pushed in place (Invocations never merge with adjacent entries);
    // tool_call_update entries mutate the existing buffer entry in place
    // by `call_id` or push a single orphan Completed entry. Adjacent
    // same-kind non-tool entries within the same notification still
    // coalesce because each append goes through the same coalesce
    // helper that walks the buffer tail.
    for update in updates {
        let Some(update_kind) = update
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .map(String::from)
        else {
            emit_inscription(
                "acp.reader.session_update_missing_kind",
                &json!({"update": update}),
            );
            continue;
        };
        match update_kind.as_str() {
            "user_message_chunk" => {
                let lines = collect_text_lines_from_value(update);
                if !lines.is_empty() {
                    coalesce_replay_entries_on_append(
                        buffer,
                        vec![ReplayEntry::User {
                            lines,
                            source: UserSource::ReaderThread,
                        }],
                    );
                }
            }
            "agent_message_chunk" => {
                let lines = collect_text_lines_from_value(update);
                if !lines.is_empty() {
                    coalesce_replay_entries_on_append(buffer, vec![ReplayEntry::Agent { lines }]);
                }
            }
            "agent_thought_chunk" => {
                let lines = collect_text_lines_from_value(update);
                if !lines.is_empty() {
                    coalesce_replay_entries_on_append(
                        buffer,
                        vec![ReplayEntry::Cognition { lines }],
                    );
                }
            }
            "tool_call" => {
                let Some(call_id) = update
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .map(String::from)
                else {
                    emit_inscription(
                        "acp.reader.tool_call_missing_id",
                        &json!({"update": update}),
                    );
                    continue;
                };
                let invocation = update.clone();
                let pending_entry = ReplayEntry::Invocation {
                    call_id: call_id.clone(),
                    status: ToolCallStatus::Pending,
                    invocation,
                    result: None,
                };
                coalesce_replay_entries_on_append(buffer, vec![pending_entry.clone()]);
                let buffer_position = buffer.len() - 1;
                pending_calls.insert(
                    call_id,
                    PendingToolCall {
                        entry: pending_entry,
                        buffer_position,
                    },
                );
            }
            "tool_call_update" => {
                let Some(call_id) = update
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .map(String::from)
                else {
                    emit_inscription(
                        "acp.reader.tool_call_update_missing_id",
                        &json!({"update": update}),
                    );
                    continue;
                };
                let result = update.clone();
                match pending_calls.remove(&call_id) {
                    Some(pending) => match &mut buffer[pending.buffer_position] {
                        ReplayEntry::Invocation {
                            status,
                            result: existing_result,
                            ..
                        } => {
                            *status = ToolCallStatus::Completed;
                            *existing_result = Some(result);
                        }
                        _ => panic!(
                            "tool_call_lifecycle invariant violated: pending call_id={} recorded at buffer position {} but entry is not Invocation",
                            call_id, pending.buffer_position
                        ),
                    },
                    None => {
                        let orphan_entry = ReplayEntry::Invocation {
                            call_id,
                            status: ToolCallStatus::Completed,
                            invocation: serde_json::json!({}),
                            result: Some(result),
                        };
                        coalesce_replay_entries_on_append(buffer, vec![orphan_entry]);
                    }
                }
            }
            _ => {
                let entry = ReplayEntry::Update {
                    update_kind,
                    lines: collect_text_lines_from_value(update),
                };
                coalesce_replay_entries_on_append(buffer, vec![entry]);
            }
        }
    }
    enforce_replay_buffer_cap_and_maintain_positions(buffer, pending_calls);
}

fn collect_text_lines_from_value(value: &Value) -> Vec<String> {
    let mut output = Vec::new();
    collect_text_lines_recursive(value, &mut output);
    output
}

fn collect_text_lines_recursive(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_text_lines_recursive(value, output);
            }
        }
        Value::Object(values) => {
            if let Some(text) = values.get("text").and_then(Value::as_str) {
                super::text::append_text_lines(text, output);
            }
            for value in values.values() {
                collect_text_lines_recursive(value, output);
            }
        }
        _ => {}
    }
}
