use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

pub(super) const LOOK_LINES_MIN: u64 = 1;
pub(super) const LOOK_LINES_MAX: u64 = 1000;
pub(super) const LIST_SESSIONS_SCHEMA_VERSION: &str = "1";
pub(super) const LIST_COMMAND_SESSIONS: &str = "sessions";
pub(super) const TOOL_HELP: &str = "help";
pub(super) const TOOL_LIST: &str = "list";
pub(super) const TOOL_LOOK: &str = "look";
pub(super) const TOOL_RAWW: &str = "raww";
pub(super) const TOOL_SEND: &str = "send";
pub(super) const TOOL_GRANT: &str = "grant";
pub(super) const GRANT_COMMAND_LIST: &str = "list";
pub(super) const GRANT_COMMAND_RESOLVE: &str = "resolve";
pub(super) const GRANT_OUTCOME_SELECTED: &str = "selected";
pub(super) const GRANT_OUTCOME_CANCELLED: &str = "cancelled";
pub(super) const NAMESPACE_AGENTMUX: &str = "agentmux";

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub(super) struct ListParams {
    /// List command selector. MVP requires command="sessions".
    #[serde(default)]
    pub(super) command: Option<String>,
    /// Command-scoped arguments.
    #[schemars(with = "std::collections::BTreeMap<String, serde_json::Value>")]
    #[serde(default)]
    pub(super) args: Value,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub(super) struct HelpParams {
    /// Namespace, tool, or command query (for example `list` or `list.sessions`).
    #[serde(default)]
    pub(super) query: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub(super) struct ListArgs {
    /// Optional bundle selector. Mutually exclusive with all=true.
    #[serde(default)]
    pub(super) bundle_name: Option<String>,
    /// Optional all-bundles fanout selector.
    #[serde(default)]
    pub(super) all: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct SendParams {
    /// Optional client request identifier echoed in responses.
    #[serde(default)]
    pub(super) request_id: Option<String>,
    /// Message body to route to targets.
    pub(super) message: String,
    /// Explicit target recipients by canonical session id (one or many).
    #[serde(default)]
    pub(super) targets: Vec<String>,
    /// Broadcast to all known sessions for the bundle.
    #[serde(default)]
    pub(super) broadcast: bool,
    /// Optional quiescence timeout override in milliseconds.
    #[serde(default)]
    pub(super) quiescence_timeout_ms: Option<u64>,
    /// Optional ACP turn timeout override in milliseconds.
    #[serde(default)]
    pub(super) acp_turn_timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct LookParams {
    /// Session identifier to inspect.
    pub(super) target_session: String,
    /// Optional override for bundle name (MVP rejects cross-bundle requests).
    #[serde(default)]
    pub(super) bundle_name: Option<String>,
    /// Optional number of pane snapshot lines to capture.
    #[serde(default)]
    pub(super) lines: Option<u64>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub(super) struct GrantParams {
    /// Grant subcommand selector. Required; allowed values: `list`, `resolve`.
    #[serde(default)]
    pub(super) command: Option<String>,
    /// Command-scoped arguments.
    #[schemars(with = "std::collections::BTreeMap<String, serde_json::Value>")]
    #[serde(default)]
    pub(super) args: Value,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub(super) struct GrantListArgs {
    /// Optional bundle selector. When present must equal the associated bundle.
    #[serde(default)]
    pub(super) bundle_name: Option<String>,
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub(super) struct GrantResolveArgs {
    /// Required permission request identifier returned by `grant list`.
    #[serde(default)]
    pub(super) permission_request_id: Option<String>,
    /// Required decision outcome (`selected` or `cancelled`).
    #[serde(default)]
    pub(super) outcome: Option<String>,
    /// Required option_id when outcome is `selected`; forbidden when `cancelled`.
    #[serde(default)]
    pub(super) option_id: Option<String>,
    /// Optional bundle selector. When present must equal the associated bundle.
    #[serde(default)]
    pub(super) bundle_name: Option<String>,
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct RawwParams {
    /// Session identifier to write to.
    pub(super) target_session: String,
    /// Raw text content to write.
    pub(super) text: String,
    /// When true, suppress trailing Enter after raw write dispatch.
    #[serde(default)]
    pub(super) no_enter: bool,
    /// Optional client request identifier echoed in responses.
    #[serde(default)]
    pub(super) request_id: Option<String>,
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}
