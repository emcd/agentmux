//! ACP reader thread: pumps the agent child's stdout into replay,
//! permission, and response dispatch.
//!
//! Mechanical split of the former `client.rs`; no semantic changes.

use std::{
    io::{BufRead, BufReader},
    process::ChildStdout,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use serde_json::{Value, json};

use crate::acp::permissions::{
    PermissionResponder, build_permission_request_from_params,
    send_permission_response,
};
use crate::acp::replay::parse_replay_entries_from_params;
use crate::runtime::inscriptions::emit_inscription;

use super::types::{
    CeasedOnDrop, PromptCompletion, ResponseEnvelope, SharedActivePrompt,
    SharedPending, SharedPendingToolCalls, SharedReplay, SharedStdin,
};

pub(super) fn spawn_reader_thread(
    reader: BufReader<ChildStdout>,
    stdin: SharedStdin,
    replay_buffer: SharedReplay,
    pending_tool_calls: SharedPendingToolCalls,
    pending_responses: SharedPending,
    active_prompt: SharedActivePrompt,
    reader_ceased: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("acp-reader".to_string())
        .spawn(move || {
            let _ceased = CeasedOnDrop(reader_ceased);
            run_reader_loop(
                reader,
                &stdin,
                &replay_buffer,
                &pending_tool_calls,
                &pending_responses,
                &active_prompt,
            );
        })
        .expect("spawn ACP reader thread")
}

fn run_reader_loop(
    mut reader: BufReader<ChildStdout>,
    stdin: &SharedStdin,
    replay_buffer: &SharedReplay,
    pending_tool_calls: &SharedPendingToolCalls,
    pending_responses: &SharedPending,
    active_prompt: &SharedActivePrompt,
) {
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {},
            Err(source) => {
                emit_inscription(
                    "acp.reader.read_failed",
                    &json!({"cause": source.to_string()}),
                );
                break;
            },
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let decoded = match serde_json::from_str::<Value>(trimmed) {
            Ok(value) => value,
            Err(source) => {
                emit_inscription(
                    "acp.reader.parse_failed",
                    &json!({"line": trimmed, "cause": source.to_string()}),
                );
                continue;
            },
        };

        if let Some(method) = decoded.get("method").and_then(Value::as_str) {
            match method {
                "session/update" => dispatch_session_update(
                    &decoded,
                    replay_buffer,
                    pending_tool_calls,
                ),
                "session/request_permission" => {
                    dispatch_permission_request(&decoded, active_prompt, stdin)
                },
                other => emit_inscription(
                    "acp.reader.unknown_method",
                    &json!({"method": other}),
                ),
            }
            continue;
        }

        if let Some(id) = decoded.get("id").and_then(Value::as_u64) {
            if try_dispatch_prompt_response(&decoded, id, active_prompt) {
                continue;
            }
            let envelope = if let Some(error) = decoded.get("error") {
                ResponseEnvelope::Error(error.to_string())
            } else {
                ResponseEnvelope::Result(
                    decoded.get("result").cloned().unwrap_or(Value::Null),
                )
            };
            let sender =
                pending_responses.lock().expect("pending mutex").remove(&id);
            match sender {
                Some(sender) => {
                    let _ = sender.send(envelope);
                },
                None => emit_inscription(
                    "acp.reader.orphan_response",
                    &json!({"id": id}),
                ),
            }
            continue;
        }

        emit_inscription(
            "acp.reader.unrecognized_message",
            &json!({"line": trimmed}),
        );
    }

    pending_responses.lock().expect("pending mutex").clear();
    let active_on_exit =
        active_prompt.lock().expect("active_prompt mutex").take();
    if let Some(active) = active_on_exit {
        let handler = active
            .on_completion
            .lock()
            .expect("on_completion mutex")
            .take();
        if let Some(handler) = handler {
            handler(PromptCompletion::ConnectionClosed {
                reason: "ACP transport closed before response".to_string(),
            });
        }
    }
}

fn try_dispatch_prompt_response(
    decoded: &Value,
    id: u64,
    active_prompt: &SharedActivePrompt,
) -> bool {
    let active_clone = {
        let slot = active_prompt.lock().expect("active_prompt mutex");
        match slot.as_ref() {
            Some(active) if active.request_id == id => {
                Some(Arc::clone(active))
            },
            _ => None,
        }
    };
    let Some(active) = active_clone else {
        return false;
    };
    let completion = if let Some(error) = decoded.get("error") {
        PromptCompletion::ProtocolError(error.to_string())
    } else {
        let stop_reason = decoded
            .get("result")
            .and_then(|result| result.get("stopReason"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
        match stop_reason {
            Some(stop_reason) => PromptCompletion::Completed { stop_reason },
            None => PromptCompletion::ProtocolError(
                "ACP session/prompt response missing result.stopReason"
                    .to_string(),
            ),
        }
    };
    let handler = active
        .on_completion
        .lock()
        .expect("on_completion mutex")
        .take();
    *active_prompt.lock().expect("active_prompt mutex") = None;
    if let Some(handler) = handler {
        handler(completion);
    }
    true
}

fn dispatch_session_update(
    decoded: &Value,
    replay_buffer: &SharedReplay,
    pending_tool_calls: &SharedPendingToolCalls,
) {
    let params = decoded.get("params").unwrap_or(&Value::Null);
    if params.get("update").is_none_or(Value::is_null) {
        return;
    }
    // Lock order: buffer first, then pending. The prompt path follows the
    // same order so the two paths cannot deadlock against each other.
    let mut buffer = replay_buffer.lock().expect("replay_buffer mutex");
    let mut pending =
        pending_tool_calls.lock().expect("pending_tool_calls mutex");
    parse_replay_entries_from_params(params, &mut pending, &mut buffer);
}

fn dispatch_permission_request(
    decoded: &Value,
    active_prompt: &SharedActivePrompt,
    stdin: &SharedStdin,
) {
    let request_id = match decoded.get("id").and_then(Value::as_u64) {
        Some(id) => id,
        None => {
            emit_inscription(
                "acp.reader.permission_request_missing_id",
                &json!({}),
            );
            return;
        },
    };
    let params = decoded.get("params").unwrap_or(&Value::Null);
    let session_id = params.get("sessionId").and_then(Value::as_str);

    let active_clone = active_prompt
        .lock()
        .expect("active_prompt mutex")
        .as_ref()
        .map(Arc::clone);
    let active = match active_clone {
        Some(active) => active,
        None => {
            emit_inscription(
                "acp.reader.permission_dropped_no_active_prompt",
                &json!({"id": request_id}),
            );
            send_permission_response(stdin, request_id, None);
            return;
        },
    };

    if session_id.is_some_and(|sid| sid != active.session_id.as_str()) {
        emit_inscription(
            "acp.reader.permission_dropped_session_mismatch",
            &json!({"id": request_id}),
        );
        send_permission_response(stdin, request_id, None);
        return;
    }

    // Single-in-flight contract (see ActivePrompt::permission_in_flight):
    // a second permission request landing while one is unresolved is dropped
    // here with a cancelled response. The agent receives a normal response
    // (so it does not stall), and an inscription records the drop for
    // diagnostics. The PermissionResponder for the in-flight request clears
    // the flag on drop, so this gate is purely a concurrency guard, not a
    // permanent block.
    if active
        .permission_in_flight
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        emit_inscription(
            "acp.reader.permission_dropped_already_in_flight",
            &json!({"id": request_id}),
        );
        send_permission_response(stdin, request_id, None);
        return;
    }

    let request = build_permission_request_from_params(params, request_id);
    let responder = PermissionResponder::new(
        Arc::clone(stdin),
        request_id,
        Arc::clone(&active.permission_in_flight),
    );

    // The handler is expected to move the responder onto a separate task
    // (typically a short-lived thread) so the reader returns to its
    // `read_line` loop immediately. If the handler is unset (e.g. relay
    // delivery did not register one), the responder is dropped here, which
    // synthesizes a cancelled response and clears the in-flight flag.
    let mut handler_slot = active
        .on_permission_request
        .lock()
        .expect("permission mutex");
    match handler_slot.as_mut() {
        Some(handler) => handler(request, responder),
        None => {
            emit_inscription(
                "acp.reader.permission_dropped_no_handler",
                &json!({"id": request_id}),
            );
            // Responder dropped: sends cancelled, clears flag.
            drop(responder);
        },
    }
}
