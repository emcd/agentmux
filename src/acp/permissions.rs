//! Permission-request dispatch types for the ACP transport: the
//! [`PermissionResponder`] used by the reader thread to write a
//! `session/request_permission` outcome back to the agent, the type aliases
//! the relay delivery side installs, and the request-side parsing and
//! response-serialization helpers.
//!
//! Companion to [`super::permission`] (which builds `PermissionHandler`
//! values per prompt); the two modules together cover both ends of the
//! ACP `session/request_permission` round-trip.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use serde_json::{Value, json};

use crate::runtime::inscriptions::emit_inscription;

use super::client::{SharedStdin, write_line_to_stdin};
use super::{PermissionOption, PermissionRequest};

/// Owns the obligation to write the agent's `session/request_permission`
/// response. The handler installed by relay delivery moves the responder
/// onto a short-lived resolver thread; once the operator's decision arrives,
/// the resolver calls [`PermissionResponder::respond`] (or, if the resolver
/// path drops it without responding, `Drop` emits a cancelled outcome) so
/// the agent never waits forever on a permission it issued.
pub struct PermissionResponder {
    stdin: SharedStdin,
    request_id: u64,
    in_flight_flag: Arc<AtomicBool>,
    responded: bool,
}

impl PermissionResponder {
    /// Builds a new [`PermissionResponder`] tied to one in-flight request.
    /// `in_flight_flag` is the same atomic the reader thread sets/clears to
    /// gate concurrent permission requests; ownership of the flag transfers
    /// to the responder, which clears it on drop.
    pub(super) fn new(
        stdin: SharedStdin,
        request_id: u64,
        in_flight_flag: Arc<AtomicBool>,
    ) -> Self {
        Self {
            stdin,
            request_id,
            in_flight_flag,
            responded: false,
        }
    }

    pub fn respond(&mut self, decision: Option<String>) {
        if self.responded {
            return;
        }
        send_permission_response(&self.stdin, self.request_id, decision);
        self.responded = true;
    }
}

impl Drop for PermissionResponder {
    fn drop(&mut self) {
        if !self.responded {
            send_permission_response(&self.stdin, self.request_id, None);
        }
        self.in_flight_flag.store(false, Ordering::SeqCst);
    }
}

/// Permission handler installed per-prompt by relay delivery. Receives the
/// parsed [`PermissionRequest`] plus a [`PermissionResponder`] the handler
/// is expected to move onto a separate task (per
/// `todos/acp/16`); the reader thread must not block on the operator's
/// decision.
pub type PermissionHandler =
    Box<dyn FnMut(PermissionRequest, PermissionResponder) + Send + 'static>;

/// Decodes the `params` payload of an ACP `session/request_permission`
/// notification into the structured [`PermissionRequest`] the handler
/// consumes. Missing fields fall back to safe defaults: a tool-call title
/// of `"unknown tool"`, a `requested_kind` of `"other"`, and an empty
/// `options` vector.
pub(super) fn build_permission_request_from_params(
    params: &Value,
    request_id: u64,
) -> PermissionRequest {
    let tool_call_title = params
        .get("toolCall")
        .and_then(|tc| tc.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("unknown tool")
        .to_string();
    let options: Vec<PermissionOption> = params
        .get("options")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|opt| {
                    Some(PermissionOption {
                        option_id: opt.get("optionId")?.as_str()?.to_string(),
                        name: opt.get("name")?.as_str()?.to_string(),
                        kind: opt
                            .get("kind")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    PermissionRequest {
        request_id,
        tool_call_title,
        requested_kind: params
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("other")
            .to_string(),
        requested_details: params.clone(),
        options,
    }
}

/// Writes the JSON-RPC outcome for one permission request back to the
/// agent's stdin. A `selected` outcome carries the operator's chosen
/// `optionId`; a `None` decision (or a `Drop`-synthesized cancellation)
/// emits `{"outcome": "cancelled"}`. Errors at the serialize or
/// write stages emit inscriptions but never panic — the responder has
/// no useful retry path and the agent will time out the request if the
/// response never lands.
pub(super) fn send_permission_response(
    stdin: &SharedStdin,
    request_id: u64,
    selected_option_id: Option<String>,
) {
    let outcome = match selected_option_id {
        Some(option_id) => json!({"outcome": "selected", "optionId": option_id}),
        None => json!({"outcome": "cancelled"}),
    };
    let response = match serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": {"outcome": outcome},
    })) {
        Ok(value) => value,
        Err(source) => {
            emit_inscription(
                "acp.reader.permission_response_serialize_failed",
                &json!({"cause": source.to_string()}),
            );
            return;
        }
    };
    if let Err(source) = write_line_to_stdin(stdin, response.as_str()) {
        emit_inscription(
            "acp.reader.permission_response_write_failed",
            &json!({"cause": source.to_string()}),
        );
    }
}
