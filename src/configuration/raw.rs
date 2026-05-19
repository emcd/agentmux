use std::path::PathBuf;

use serde::Deserialize;

use super::types::{AcpChannel, NameValueEntry};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct RawCodersFile {
    pub(super) format_version: u32,
    #[serde(default)]
    pub(super) coders: Vec<RawCoder>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct RawCoder {
    pub(super) id: String,
    #[serde(default)]
    pub(super) tmux: Option<RawTmuxTarget>,
    #[serde(default)]
    pub(super) acp: Option<RawAcpTarget>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct RawTmuxTarget {
    pub(super) initial_command: String,
    pub(super) resume_command: String,
    #[serde(default)]
    pub(super) prompt_regex: Option<String>,
    #[serde(default)]
    pub(super) prompt_inspect_lines: Option<usize>,
    #[serde(default)]
    pub(super) prompt_idle_column: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct RawAcpTarget {
    pub(super) channel: AcpChannel,
    #[serde(default)]
    pub(super) command: Option<String>,
    #[serde(default)]
    pub(super) url: Option<String>,
    #[serde(default)]
    pub(super) turn_timeout_ms: Option<u64>,
    #[serde(default)]
    pub(super) headers: Vec<NameValueEntry>,
    #[serde(default)]
    pub(super) environment: Vec<NameValueEntry>,
}

#[derive(Clone, Debug)]
pub(super) struct Coder {
    pub(super) target: CoderTarget,
}

#[derive(Clone, Debug)]
pub(super) enum CoderTarget {
    Tmux(TmuxTarget),
    Acp(AcpTarget),
}

#[derive(Clone, Debug)]
pub(super) struct TmuxTarget {
    pub(super) initial_command: String,
    pub(super) resume_command: String,
    pub(super) prompt_regex: Option<String>,
    pub(super) prompt_inspect_lines: Option<usize>,
    pub(super) prompt_idle_column: Option<usize>,
}

#[derive(Clone, Debug)]
pub(super) struct AcpTarget {
    pub(super) channel: AcpChannel,
    pub(super) command: Option<String>,
    pub(super) url: Option<String>,
    pub(super) turn_timeout_ms: Option<u64>,
    pub(super) headers: Vec<NameValueEntry>,
    pub(super) environment: Vec<NameValueEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct RawBundleFile {
    pub(super) format_version: u32,
    #[serde(default)]
    pub(super) autostart: bool,
    #[serde(default)]
    pub(super) groups: Vec<String>,
    #[serde(default)]
    pub(super) sessions: Vec<RawSession>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct RawUsersFile {
    #[serde(default)]
    pub(super) default_bundle: Option<String>,
    #[serde(default)]
    pub(super) default_session: Option<String>,
    #[serde(default)]
    pub(super) sessions: Vec<RawUsersSession>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct RawUsersSession {
    pub(super) id: String,
    #[serde(default)]
    pub(super) name: Option<String>,
    pub(super) policy: String,
    #[serde(default)]
    pub(super) ui: Option<RawSessionMarker>,
    #[serde(default)]
    pub(super) pubsub: Option<RawSessionMarker>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct RawPoliciesFile {
    pub(super) format_version: u32,
    #[serde(default, rename = "default")]
    pub(super) _default: Option<String>,
    #[serde(default)]
    pub(super) policies: Vec<RawPolicyPreset>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct RawPolicyPreset {
    pub(super) id: String,
    #[serde(default, rename = "description")]
    pub(super) _description: Option<String>,
    #[serde(default, rename = "controls")]
    pub(super) _controls: Option<toml::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct RawSession {
    pub(super) id: String,
    #[serde(default)]
    pub(super) name: Option<String>,
    pub(super) directory: PathBuf,
    #[serde(default)]
    pub(super) policy: Option<String>,
    #[serde(default)]
    pub(super) coder: Option<String>,
    #[serde(default)]
    pub(super) coder_session_id: Option<String>,
    #[serde(default)]
    pub(super) ui: Option<RawSessionMarker>,
    #[serde(default)]
    pub(super) pubsub: Option<RawSessionMarker>,
}

/// A `[sessions.ui]` or `[sessions.pubsub]` subtable; an empty body is valid.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawSessionMarker {}
