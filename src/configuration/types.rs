use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const RESERVED_GROUP_ALL: &str = "ALL";

/// Declared session type for one configured session entry.
///
/// The session type is fixed at operator configuration time by the single
/// session-type subtable on a `[[sessions]]` entry. It is never asserted by a
/// client at connect time.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionType {
    Tmux,
    Acp,
    Ui,
    Pubsub,
}

/// One configured bundle member.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BundleMember {
    /// Canonical routing identity from `[[sessions]].id`.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Optional human-facing recipient label from `[[sessions]].name`.
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<PathBuf>,
    /// Declared session type and its delivery target configuration.
    pub target: TargetConfiguration,
    /// Optional persistent agent session handle sourced from the active
    /// session-type subtable's `coder-session-id` (not from `[[coders]]`).
    /// ACP delivery uses this to select `session/load` vs `session/new`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coder_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
}

/// Optional prompt-readiness template for one bundle member.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PromptReadinessTemplate {
    pub prompt_regex: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_lines: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_idle_cursor_column: Option<usize>,
}

/// Validated session type and delivery target configuration for one member.
///
/// `Tmux` and `Acp` carry transport configuration; `Ui` and `Pubsub` are bare
/// markers whose delivery paths are not yet implemented.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "transport", content = "config")]
pub enum TargetConfiguration {
    Tmux(TmuxTargetConfiguration),
    Acp(AcpTargetConfiguration),
    Ui,
    Pubsub,
}

impl TargetConfiguration {
    /// Reports the declared session type for this target.
    #[must_use]
    pub fn session_type(&self) -> SessionType {
        match self {
            Self::Tmux(_) => SessionType::Tmux,
            Self::Acp(_) => SessionType::Acp,
            Self::Ui => SessionType::Ui,
            Self::Pubsub => SessionType::Pubsub,
        }
    }
}

/// Tmux transport configuration for one bundle member.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct TmuxTargetConfiguration {
    pub start_command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_readiness: Option<PromptReadinessTemplate>,
}

/// ACP transport configuration for one bundle member.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AcpTargetConfiguration {
    pub channel: AcpChannel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<NameValueEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<NameValueEntry>,
}

/// Configuration for one named bundle.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BundleConfiguration {
    pub schema_version: String,
    pub bundle_name: String,
    pub autostart: bool,
    pub groups: Vec<String>,
    pub members: Vec<BundleMember>,
}

/// Group membership metadata for one bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleGroupMembership {
    pub bundle_name: String,
    pub autostart: bool,
    pub groups: Vec<String>,
}

/// One global user session entry from `users.toml`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct TuiSession {
    /// Canonical global identity in `session@GLOBAL` form.
    pub id: String,
    /// Optional operator-facing label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Policy preset reference.
    pub policy: String,
    /// Declared session type for this global user entry.
    pub session_type: SessionType,
}

/// Global user configuration loaded from `users.toml`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct TuiConfiguration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_bundle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_session: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<TuiSession>,
}

impl TuiConfiguration {
    #[must_use]
    pub fn session_by_id(&self, selector: &str) -> Option<&TuiSession> {
        self.sessions.iter().find(|session| session.id == selector)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct NameValueEntry {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AcpChannel {
    Stdio,
    Http,
}
