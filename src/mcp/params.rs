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
pub(super) const LIST_COMMAND_NAMESPACES: &str = "namespaces";
pub(super) const LIST_COMMAND_RELAYS: &str = "relays";
pub(super) const NAMESPACE_AGENTMUX: &str = "agentmux";

/// Renders a meta-tool `command` selector as a flat JSON Schema string enum.
///
/// The owning field stays a lenient `String` so unsupported values reach the
/// handler's dispatch and surface as `validation_invalid_params` rather than
/// failing early as a serde deserialize error; only the advertised schema is
/// constrained. The doc-comment description on each field is merged on top of
/// the schema returned here.
fn command_enum_schema(values: &[&str]) -> schemars::Schema {
    schemars::json_schema!({
        "type": "string",
        "enum": values,
    })
}

fn list_command_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    command_enum_schema(&["principals", "namespaces", "relays", "decisions"])
}

fn updown_command_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    command_enum_schema(&["up", "down"])
}

fn new_command_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    command_enum_schema(&["peer"])
}

fn change_command_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    command_enum_schema(&["psk"])
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(super) struct ListParams {
    /// List command selector. Allowed values: `principals`, `namespaces`,
    /// `relays`, `decisions`.
    #[schemars(schema_with = "list_command_schema")]
    pub(super) command: String,
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
    #[schemars(with = "String")]
    pub(super) query: Option<String>,
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(super) struct ListArgs {
    /// Listing scope selector. When `relay` is absent: omitted/null selects the
    /// associated/home bundle; a bundle name selects that bundle; `GLOBAL`
    /// selects relay-wide principals; `*` fans out across all namespaces. When
    /// `relay` is set, this must name one concrete foreign namespace (`*` and
    /// reserved tokens are rejected).
    #[serde(default)]
    #[schemars(with = "String")]
    pub(super) namespace: Option<String>,
    /// Optional configured outbound peer alias. When set, principal discovery is
    /// forwarded to that foreign relay and `namespace` is required; when absent,
    /// listing is local.
    #[serde(default)]
    #[schemars(with = "String")]
    pub(super) relay: Option<String>,
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(super) struct ListNamespacesArgs {
    /// Optional configured outbound peer alias. When set, namespace discovery is
    /// forwarded to that foreign relay; when absent, discovery is local.
    #[serde(default)]
    #[schemars(with = "String")]
    pub(super) relay: Option<String>,
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(super) struct ListRelaysArgs {
    /// Unknown fields captured for explicit validation. Relay enumeration takes
    /// no arguments.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(super) struct SendParams {
    /// Optional client request identifier echoed in responses.
    #[serde(default)]
    #[schemars(with = "String")]
    pub(super) request_id: Option<String>,
    /// Message body to route to targets.
    pub(super) message: String,
    /// Explicit target recipients as principal ids (one or many). Each is a
    /// bare id resolved within the associated bundle, or a fully-qualified
    /// `<id>@<namespace>` for a cross-namespace peer.
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
    /// Principal id to inspect: a bare id resolves within the associated
    /// bundle, or a fully-qualified `<id>@<namespace>` targets a cross-namespace
    /// peer. Routing context is inferred from the `@<namespace>` suffix; no
    /// explicit namespace parameter is accepted.
    pub(super) target_session: String,
    /// Optional snapshot window size: tmux pane lines, or ACP replay entries.
    #[serde(default)]
    #[schemars(with = "u64")]
    pub(super) lines: Option<u64>,
    /// Optional entries to skip from the newest end before the tail-N window,
    /// for walking backward through older ACP replay context. ACP targets only.
    #[serde(default)]
    #[schemars(with = "u64")]
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
    #[schemars(with = "String")]
    pub(super) choice_request_id: Option<String>,
    /// Required decision outcome (`selected` or `cancelled`).
    #[serde(default)]
    #[schemars(with = "String")]
    pub(super) outcome: Option<String>,
    /// Required option_id when outcome is `selected`; forbidden when `cancelled`.
    #[serde(default)]
    #[schemars(with = "String")]
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
    #[schemars(schema_with = "updown_command_schema")]
    pub(super) command: String,
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
    #[schemars(schema_with = "new_command_schema")]
    pub(super) command: String,
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
    #[schemars(with = "String")]
    pub(super) principal_id: Option<String>,
    /// Optional scope recorded on the principal (set for `@RELAY`/`@EXTERNAL`).
    #[serde(default)]
    #[schemars(with = "String")]
    pub(super) scope: Option<String>,
    /// Optional absolute path; when present the PSK is written there instead of
    /// being returned in the response. Mutually exclusive with `write_to_config`.
    #[serde(default)]
    #[schemars(with = "String")]
    pub(super) output_path: Option<String>,
    /// When true, the relay writes the PSK to the principal's canonical config
    /// path (session principals only) and omits it from the response. Mutually
    /// exclusive with `output_path`.
    #[serde(default)]
    #[schemars(with = "bool")]
    pub(super) write_to_config: Option<bool>,
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(super) struct ChangeParams {
    /// Change subcommand selector. Required; allowed value: `psk`.
    #[schemars(schema_with = "change_command_schema")]
    pub(super) command: String,
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
    #[schemars(with = "String")]
    pub(super) principal_id: Option<String>,
    /// Optional absolute path; when present the rotated PSK is written there
    /// instead of being returned. Mutually exclusive with `write_to_config`.
    #[serde(default)]
    #[schemars(with = "String")]
    pub(super) output_path: Option<String>,
    /// When true, the relay writes the rotated PSK to the principal's canonical
    /// config path (session principals only) and omits it from the response.
    /// Mutually exclusive with `output_path`.
    #[serde(default)]
    #[schemars(with = "bool")]
    pub(super) write_to_config: Option<bool>,
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(super) struct RawwParams {
    /// Principal id to write to: a bare id resolved within the associated
    /// bundle, or a fully-qualified `<id>@<namespace>` for a cross-namespace
    /// peer.
    pub(super) target_session: String,
    /// Raw text content written directly to the target's input, bypassing
    /// normal chat/message semantics.
    pub(super) text: String,
    /// When true, suppress trailing Enter after raw write dispatch.
    #[serde(default)]
    pub(super) no_enter: bool,
    /// Optional client request identifier echoed in responses.
    #[serde(default)]
    #[schemars(with = "String")]
    pub(super) request_id: Option<String>,
    /// Unknown fields captured for explicit validation.
    #[serde(flatten, default)]
    #[schemars(skip)]
    pub(super) extra_fields: BTreeMap<String, Value>,
}
