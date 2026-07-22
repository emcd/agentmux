//! `look` tool: inspect a target session snapshot — tmux pane lines or ACP
//! structured replay entries — in the associated bundle or a qualified peer.

use rmcp::{
    ErrorData as McpError,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    tool, tool_router,
};
use serde_json::{Value, json};

use crate::relay::{LookSnapshotPayload, RelayRequest, RelayResponse};
use crate::runtime::inscriptions::emit_inscription;

use crate::mcp::params::LookParams;
use crate::mcp::server::McpServer;
use crate::mcp::validation::{qualify_target, validate_look_request};

#[tool_router(router = tool_router_look, vis = "pub(crate)")]
impl McpServer {
    #[tool(
        name = "look",
        description = "Inspect a target session's latest snapshot: tmux pane lines, or ACP structured replay entries. Target is a bare id in the associated bundle or a fully-qualified id@namespace peer."
    )]
    async fn tool_look(
        &self,
        Parameters(params): Parameters<LookParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_look_request(&params)?;
        emit_inscription(
            "mcp.tool.look.request",
            &json!({
                "namespace": self.associated_namespace(),
                "requester_session": self.state.configuration.sender_session.clone(),
                "target_session": params.target_session.clone(),
                "lines": params.lines,
                "offset": params.offset,
            }),
        );
        let requester_session = self.require_associated_sender_session()?;

        // Qualify a bare target to the bound bundle (mirrors send); an
        // already-qualified `@<namespace>` target passes through so peer-bundle
        // inspection still works. Done after sender resolution so an unassociated
        // server fails as `validation_unassociated_server` regardless of target
        // shape.
        let target_session = qualify_target(&params.target_session, self.associated_namespace())?;

        let request = RelayRequest::Look {
            requester_session,
            target_session,
            lines: params.lines.map(|value| value as usize),
            offset: params.offset.map(|value| value as usize),
        };
        match self.request_relay(&request) {
            Ok(RelayResponse::Look {
                schema_version,
                requester_session,
                target_session,
                captured_at,
                authenticated_identity,
                on_behalf_of,
                snapshot,
            }) => {
                let mut response_map = serde_json::Map::new();
                response_map.insert("schema_version".to_string(), Value::String(schema_version));
                response_map.insert(
                    "requester_session".to_string(),
                    Value::String(requester_session),
                );
                response_map.insert("target_session".to_string(), Value::String(target_session));
                response_map.insert("captured_at".to_string(), Value::String(captured_at));
                if let Some(identity) = authenticated_identity {
                    response_map.insert(
                        "authenticated_identity".to_string(),
                        Value::String(identity),
                    );
                }
                if let Some(delegate) = on_behalf_of {
                    response_map.insert("on_behalf_of".to_string(), Value::String(delegate));
                }
                let snapshot_count = match snapshot {
                    LookSnapshotPayload::Lines { snapshot_lines } => {
                        let count = snapshot_lines.len();
                        response_map.insert("snapshot_format".to_string(), json!("lines"));
                        response_map.insert("snapshot_lines".to_string(), json!(snapshot_lines));
                        count
                    }
                    LookSnapshotPayload::StructuredEntriesV1 {
                        snapshot_entries,
                        entries_total,
                        returned_entries_count,
                        freshness,
                        snapshot_source,
                        stale_reason_code,
                        snapshot_age_ms,
                    } => {
                        let count = snapshot_entries.len();
                        response_map.insert(
                            "snapshot_format".to_string(),
                            json!("structured_entries_v1"),
                        );
                        response_map
                            .insert("snapshot_entries".to_string(), json!(snapshot_entries));
                        response_map.insert("entries_total".to_string(), json!(entries_total));
                        response_map.insert(
                            "returned_entries_count".to_string(),
                            json!(returned_entries_count),
                        );
                        response_map.insert("freshness".to_string(), json!(freshness));
                        response_map.insert("snapshot_source".to_string(), json!(snapshot_source));
                        if let Some(value) = stale_reason_code {
                            response_map
                                .insert("stale_reason_code".to_string(), Value::String(value));
                        }
                        if let Some(value) = snapshot_age_ms {
                            response_map.insert("snapshot_age_ms".to_string(), json!(value));
                        }
                        count
                    }
                };
                let response = Value::Object(response_map);
                emit_inscription(
                    "mcp.tool.look.success",
                    &json!({
                        "requester_session": response["requester_session"],
                        "target_session": response["target_session"],
                        "snapshot_format": response["snapshot_format"],
                        "snapshot_count": snapshot_count,
                    }),
                );
                Ok(CallToolResult::success(vec![Content::json(response)?]))
            }
            Ok(other) => Err(self.map_nonsuccess_relay_response("mcp.tool.look", other)),
            Err(source) => Err(self.map_relay_stream_failure("mcp.tool.look.io_error", source)),
        }
    }
}
