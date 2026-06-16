use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

pub(super) const LOOK_LINES_MIN: u64 = 1;
pub(super) const LOOK_LINES_MAX: u64 = 1000;
pub(super) const LIST_SESSIONS_SCHEMA_VERSION: &str = "1";
pub(super) const LIST_COMMAND_PRINCIPALS: &str = "principals";
pub(super) const LIST_COMMAND_DECISIONS: &str = "decisions";
pub(super) const TOOL_HELP: &str = "help";
pub(super) const TOOL_LIST: &str = "list";
pub(super) const TOOL_LOOK: &str = "look";
pub(super) const TOOL_RAWW: &str = "raww";
pub(super) const TOOL_SEND: &str = "send";
pub(super) const TOOL_CHOOSE: &str = "choose";
pub(super) const CHOOSE_OUTCOME_SELECTED: &str = "selected";
pub(super) const CHOOSE_OUTCOME_CANCELLED: &str = "cancelled";
pub(super) const TOOL_UPDOWN: &str = "updown";
pub(super) const UPDOWN_COMMAND_UP: &str = "up";
pub(super) const UPDOWN_COMMAND_DOWN: &str = "down";
pub(super) const TOOL_NEW: &str = "new";
pub(super) const NEW_COMMAND_PEER: &str = "peer";
pub(super) const TOOL_CHANGE: &str = "change";
pub(super) const CHANGE_COMMAND_PSK: &str = "psk";
pub(super) const NAMESPACE_AGENTMUX: &str = "agentmux";

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(super) struct ListParams {
    /// List command selector. Allowed values: `principals`, `decisions`.
    #[serde(default)]
    pub(super) command: Option<String>,
    /// Command-scoped arguments.
    #[schemars(with = "std::collections::BTreeMap<String, serde_json::Value>")]
    #[serde(default)]
    pub(super) args: Value,
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(super) struct HelpParams {
    /// Namespace, tool, or command query (for example `list` or `list.principals`).
    #[serde(default)]
    pub(super) query: Option<String>,
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(super) struct ListArgs {
    /// Listing scope selector. Omitted/null selects the associated/home bundle;
    /// a bundle name selects that bundle; `GLOBAL` selects relay-wide
    /// principals; `*` fans out across all namespaces.
    #[serde(default)]
    pub(super) namespace: Option<String>,
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
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
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(super) struct LookParams {
    /// Session identifier to inspect. Routing context is inferred from the
    /// `@<namespace>` suffix; no explicit namespace parameter is accepted.
    pub(super) target_session: String,
    /// Optional snapshot window size: tmux pane lines, or ACP replay entries.
    #[serde(default)]
    pub(super) lines: Option<u64>,
    /// Optional entries to skip from the newest end before the tail-N window,
    /// for walking backward through older ACP replay context. ACP targets only.
    #[serde(default)]
    pub(super) offset: Option<u64>,
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(super) struct ListDecisionsArgs {
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(super) struct ChooseParams {
    /// Required choice request identifier returned by `list` command="decisions".
    #[serde(default)]
    pub(super) choice_request_id: Option<String>,
    /// Required decision outcome (`selected` or `cancelled`).
    #[serde(default)]
    pub(super) outcome: Option<String>,
    /// Required option_id when outcome is `selected`; forbidden when `cancelled`.
    #[serde(default)]
    pub(super) option_id: Option<String>,
    /// Unknown fields captured for explicit validation. Caller-supplied
    /// sender-like identity fields (`decided_by`, `ui_session_id`,
    /// `operator_session_id`) land here and are rejected; the decision actor is
    /// association-derived by the relay.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(super) struct UpdownParams {
    /// Updown subcommand selector. Required; allowed values: `up`, `down`.
    #[serde(default)]
    pub(super) command: Option<String>,
    /// Command-scoped arguments.
    #[schemars(with = "std::collections::BTreeMap<String, serde_json::Value>")]
    #[serde(default)]
    pub(super) args: Value,
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(super) struct UpdownArgs {
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(super) struct NewParams {
    /// New subcommand selector. Required; allowed value: `peer`.
    #[serde(default)]
    pub(super) command: Option<String>,
    /// Command-scoped arguments.
    #[schemars(with = "std::collections::BTreeMap<String, serde_json::Value>")]
    #[serde(default)]
    pub(super) args: Value,
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(super) struct NewPeerArgs {
    /// Principal identifier to register, in `<id>@<namespace>` form.
    #[serde(default)]
    pub(super) principal_id: Option<String>,
    /// Optional scope recorded on the principal (set for `@RELAY`/`@EXTERNAL`).
    #[serde(default)]
    pub(super) scope: Option<String>,
    /// Optional absolute path; when present the PSK is written there instead of
    /// being returned in the response.
    #[serde(default)]
    pub(super) output_path: Option<String>,
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(super) struct ChangeParams {
    /// Change subcommand selector. Required; allowed value: `psk`.
    #[serde(default)]
    pub(super) command: Option<String>,
    /// Command-scoped arguments.
    #[schemars(with = "std::collections::BTreeMap<String, serde_json::Value>")]
    #[serde(default)]
    pub(super) args: Value,
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(super) struct ChangePskArgs {
    /// Principal identifier whose credential is rotated.
    #[serde(default)]
    pub(super) principal_id: Option<String>,
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
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
