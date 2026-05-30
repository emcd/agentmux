use rmcp::ErrorData as McpError;
use schemars::JsonSchema;
use serde_json::json;

use super::errors::validation_tool_error;
use super::params::{
    CHANGE_COMMAND_PSK, ChangePskArgs, GRANT_COMMAND_LIST, GRANT_COMMAND_RESOLVE,
    GRANT_OUTCOME_SELECTED, GrantListArgs, GrantResolveArgs, HelpParams, LIST_COMMAND_SESSIONS,
    ListArgs, LookParams, NAMESPACE_AGENTMUX, NEW_COMMAND_PEER, NewPeerArgs, RawwParams,
    SendParams, TOOL_CHANGE, TOOL_GRANT, TOOL_HELP, TOOL_LIST, TOOL_LOOK, TOOL_NEW, TOOL_RAWW,
    TOOL_SEND, TOOL_UPDOWN, UPDOWN_COMMAND_DOWN, UPDOWN_COMMAND_UP, UpdownArgs,
};

pub(super) fn help_tool(params: HelpParams) -> Result<serde_json::Value, McpError> {
    let query = params.query.as_deref().map(str::trim).unwrap_or_default();
    match query {
        "" | NAMESPACE_AGENTMUX => Ok(json!({
            "namespace": NAMESPACE_AGENTMUX,
            "shape_hints": [
                "Call help with query='list', 'grant', 'updown', 'new', or 'change' for meta-tool command lists.",
                "Call help with query='list.sessions', 'grant.list', 'grant.resolve', 'updown.up', 'updown.down', 'new.peer', or 'change.psk' for command args schemas.",
                "Call help with query='send', 'look', or 'raww' for exact tool args schemas."
            ],
            "tools": [
                {"tool": TOOL_LIST, "kind": "meta_tool", "description": "List sessions for one bundle or fan out across bundles."},
                {"tool": TOOL_SEND, "kind": "tool", "description": "Submit a message to explicit targets or broadcast."},
                {"tool": TOOL_LOOK, "kind": "tool", "description": "Inspect a target session pane snapshot for this bundle."},
                {"tool": TOOL_RAWW, "kind": "tool", "description": "Write raw text directly to one target session."},
                {"tool": TOOL_GRANT, "kind": "meta_tool", "description": "Inspect or resolve pending ACP permission requests."},
                {"tool": TOOL_UPDOWN, "kind": "meta_tool", "description": "Administer bundle runtime updown (up/down)."},
                {"tool": TOOL_NEW, "kind": "meta_tool", "description": "Register a principal credential and mint its PSK."},
                {"tool": TOOL_CHANGE, "kind": "meta_tool", "description": "Rotate the PSK for an existing principal."},
                {"tool": TOOL_HELP, "kind": "tool", "description": "Return tool/command help and JSON schemas."}
            ],
            "invoke": {
                "tool": TOOL_HELP,
                "params": {"query": TOOL_LIST}
            }
        })),
        TOOL_LIST => Ok(json!({
            "tool": TOOL_LIST,
            "kind": "meta_tool",
            "description": "List sessions for one bundle or fan out across bundles.",
            "commands": [
                {
                    "command": "list.sessions",
                    "description": "List sessions for one bundle or fan out across bundles."
                }
            ],
            "invoke": {
                "tool": TOOL_LIST,
                "params": {
                    "command": LIST_COMMAND_SESSIONS,
                    "args": {}
                }
            }
        })),
        "list.sessions" => Ok(command_help(
            "list.sessions",
            "List sessions for one bundle or fan out across bundles.",
            json_schema_for::<ListArgs>(),
            json!({
                "tool": TOOL_LIST,
                "params": {
                    "command": LIST_COMMAND_SESSIONS,
                    "args": {}
                }
            }),
        )),
        TOOL_SEND => Ok(command_help(
            TOOL_SEND,
            "Submit a message to explicit targets or broadcast.",
            json_schema_for::<SendParams>(),
            json!({
                "tool": TOOL_SEND,
                "params": {}
            }),
        )),
        TOOL_LOOK => Ok(command_help(
            TOOL_LOOK,
            "Inspect a target session pane snapshot for this bundle.",
            json_schema_for::<LookParams>(),
            json!({
                "tool": TOOL_LOOK,
                "params": {}
            }),
        )),
        TOOL_RAWW => Ok(command_help(
            TOOL_RAWW,
            "Write raw text directly to one target session.",
            json_schema_for::<RawwParams>(),
            json!({
                "tool": TOOL_RAWW,
                "params": {}
            }),
        )),
        TOOL_HELP => Ok(command_help(
            TOOL_HELP,
            "Return tool/command help and JSON schemas.",
            json_schema_for::<HelpParams>(),
            json!({
                "tool": TOOL_HELP,
                "params": {}
            }),
        )),
        TOOL_GRANT => Ok(json!({
            "tool": TOOL_GRANT,
            "kind": "meta_tool",
            "description": "Inspect or resolve pending ACP permission requests.",
            "commands": [
                {
                    "command": "grant.list",
                    "description": "List pending ACP permission requests for the associated bundle."
                },
                {
                    "command": "grant.resolve",
                    "description": "Submit an ACP-native decision on a pending permission request."
                }
            ],
            "invoke": {
                "tool": TOOL_GRANT,
                "params": {
                    "command": GRANT_COMMAND_LIST,
                    "args": {}
                }
            }
        })),
        "grant.list" => Ok(command_help(
            "grant.list",
            "List pending ACP permission requests for the associated bundle.",
            json_schema_for::<GrantListArgs>(),
            json!({
                "tool": TOOL_GRANT,
                "params": {
                    "command": GRANT_COMMAND_LIST,
                    "args": {}
                }
            }),
        )),
        "grant.resolve" => Ok(command_help(
            "grant.resolve",
            "Submit an ACP-native decision on a pending permission request.",
            json_schema_for::<GrantResolveArgs>(),
            json!({
                "tool": TOOL_GRANT,
                "params": {
                    "command": GRANT_COMMAND_RESOLVE,
                    "args": {
                        "permission_request_id": "<uuid>",
                        "outcome": GRANT_OUTCOME_SELECTED,
                        "option_id": "<option-id>"
                    }
                }
            }),
        )),
        TOOL_UPDOWN => Ok(json!({
            "tool": TOOL_UPDOWN,
            "kind": "meta_tool",
            "description": "Administer bundle runtime updown (up/down) for the associated bundle.",
            "commands": [
                {
                    "command": "updown.up",
                    "description": "Host the associated bundle runtime."
                },
                {
                    "command": "updown.down",
                    "description": "Unhost the associated bundle runtime."
                }
            ],
            "invoke": {
                "tool": TOOL_UPDOWN,
                "params": {
                    "command": UPDOWN_COMMAND_UP,
                    "args": {}
                }
            }
        })),
        "updown.up" => Ok(command_help(
            "updown.up",
            "Host the associated bundle runtime.",
            json_schema_for::<UpdownArgs>(),
            json!({
                "tool": TOOL_UPDOWN,
                "params": {
                    "command": UPDOWN_COMMAND_UP,
                    "args": {}
                }
            }),
        )),
        "updown.down" => Ok(command_help(
            "updown.down",
            "Unhost the associated bundle runtime.",
            json_schema_for::<UpdownArgs>(),
            json!({
                "tool": TOOL_UPDOWN,
                "params": {
                    "command": UPDOWN_COMMAND_DOWN,
                    "args": {}
                }
            }),
        )),
        TOOL_NEW => Ok(json!({
            "tool": TOOL_NEW,
            "kind": "meta_tool",
            "description": "Register a principal credential and mint its PSK.",
            "commands": [
                {
                    "command": "new.peer",
                    "description": "Generate a PSK for a principal_id and return it (or write it to an output path)."
                }
            ],
            "invoke": {
                "tool": TOOL_NEW,
                "params": {
                    "command": NEW_COMMAND_PEER,
                    "args": {"principal_id": "<id>@<namespace>"}
                }
            }
        })),
        "new.peer" => Ok(command_help(
            "new.peer",
            "Generate a PSK for a principal_id and return it (or write it to an output path).",
            json_schema_for::<NewPeerArgs>(),
            json!({
                "tool": TOOL_NEW,
                "params": {
                    "command": NEW_COMMAND_PEER,
                    "args": {"principal_id": "<id>@<namespace>"}
                }
            }),
        )),
        TOOL_CHANGE => Ok(json!({
            "tool": TOOL_CHANGE,
            "kind": "meta_tool",
            "description": "Rotate the PSK for an existing principal.",
            "commands": [
                {
                    "command": "change.psk",
                    "description": "Generate a new PSK for an existing principal_id and return it."
                }
            ],
            "invoke": {
                "tool": TOOL_CHANGE,
                "params": {
                    "command": CHANGE_COMMAND_PSK,
                    "args": {"principal_id": "<id>@<namespace>"}
                }
            }
        })),
        "change.psk" => Ok(command_help(
            "change.psk",
            "Generate a new PSK for an existing principal_id and return it.",
            json_schema_for::<ChangePskArgs>(),
            json!({
                "tool": TOOL_CHANGE,
                "params": {
                    "command": CHANGE_COMMAND_PSK,
                    "args": {"principal_id": "<id>@<namespace>"}
                }
            }),
        )),
        _ => Err(validation_tool_error(
            "validation_invalid_params",
            "unknown help query; try empty query, 'agentmux', 'list', 'list.sessions', 'send', 'look', 'raww', 'grant', 'grant.list', 'grant.resolve', 'updown', 'updown.up', 'updown.down', 'new', 'new.peer', 'change', or 'change.psk'",
            Some(json!({"query": query})),
        )),
    }
}

fn command_help(
    command: &str,
    description: &str,
    args_schema: serde_json::Value,
    invoke: serde_json::Value,
) -> serde_json::Value {
    json!({
        "command": command,
        "description": description,
        "args_schema": args_schema,
        "invoke": invoke,
    })
}

fn json_schema_for<T: JsonSchema>() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(T)).unwrap_or(serde_json::Value::Null)
}
