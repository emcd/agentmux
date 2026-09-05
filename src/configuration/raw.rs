use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

use super::types::{AcpChannel, NameValueEntry, TermProtocol};

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
    #[serde(default)]
    pub(super) pty: Option<RawPtyTarget>,
    /// Transport-agnostic environment applied to the child this coder spawns
    /// for a session, regardless of transport (Tmux, Pty, or ACP
    /// command-spawn). The base layer of the coder/bundle/session merge.
    #[serde(default)]
    pub(super) environment: Vec<NameValueEntry>,
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
    pub(super) headers: Vec<NameValueEntry>,
}

/// Raw `[coders.<id>.pty]` subtable. Exactly one of `tmux`, `acp`, or
/// `pty` must be set per coder entry; the validator rejects both or
/// neither (see `targets::validate_coder_transport`).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct RawPtyTarget {
    pub(super) initial_command: String,
    pub(super) resume_command: String,
    #[serde(default)]
    pub(super) prompt_regex: Option<String>,
    #[serde(default)]
    pub(super) prompt_inspect_lines: Option<usize>,
    #[serde(default)]
    pub(super) prompt_idle_column: Option<usize>,
    /// Per-coder grid columns (TOML key `cols`). Default 120.
    #[serde(default)]
    pub(super) cols: Option<u16>,
    /// Per-coder grid rows (TOML key `rows`). Default 40.
    #[serde(default)]
    pub(super) rows: Option<u16>,
    /// Per-coder terminal protocol (TOML key `term-protocol`).
    /// Selects the literal `TERM` env-var value the Pty transport
    /// sets when spawning the child. Absent means
    /// `xterm-256color` (preserves the pre-change behavior).
    #[serde(default)]
    pub(super) term_protocol: Option<TermProtocol>,
}

#[derive(Clone, Debug)]
pub(super) struct Coder {
    pub(super) target: CoderTarget,
    /// Validated coder-level environment (base merge layer).
    pub(super) environment: Vec<NameValueEntry>,
}

#[derive(Clone, Debug)]
pub(super) enum CoderTarget {
    Tmux(TmuxTarget),
    Acp(AcpTarget),
    Pty(PtyTarget),
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
    pub(super) headers: Vec<NameValueEntry>,
}

/// Validated `[coders.<id>.pty]` target fields.
#[derive(Clone, Debug)]
pub(super) struct PtyTarget {
    pub(super) initial_command: String,
    pub(super) resume_command: String,
    pub(super) prompt_regex: Option<String>,
    pub(super) prompt_inspect_lines: Option<usize>,
    pub(super) prompt_idle_column: Option<usize>,
    pub(super) cols: Option<u16>,
    pub(super) rows: Option<u16>,
    pub(super) term_protocol: Option<TermProtocol>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct RawBundleFile {
    pub(super) format_version: u32,
    #[serde(default)]
    pub(super) autostart: bool,
    #[serde(default)]
    pub(super) groups: Vec<String>,
    /// Bundle-level environment applied to every coder-backed session in the
    /// bundle. Overrides colliding coder-level names; overridden by session.
    #[serde(default)]
    pub(super) environment: Vec<NameValueEntry>,
    #[serde(default)]
    pub(super) sessions: Vec<RawSession>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct RawUsersFile {
    #[serde(default)]
    pub(super) default_session: Option<String>,
    #[serde(default)]
    pub(super) sessions: Vec<RawUsersSession>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct RawUiFile {
    #[serde(default)]
    pub(super) default_bundle: Option<String>,
    #[serde(default)]
    pub(super) bindings: Option<RawBindings>,
}

/// The `[bindings]` group, before any of it has been validated.
///
/// The two options are named fields; every other key is a binding context name
/// whose value is that context's chord table. `deny_unknown_fields` cannot
/// express that, since the context names are not known to serde, so the
/// remainder is flattened here and the loader rejects a key that names no
/// context. That rejection is what keeps a misspelled context from being
/// silently skipped, which would leave an operator believing a configuration is
/// in force that does nothing.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct RawBindings {
    #[serde(default)]
    pub(super) presets: Vec<String>,
    #[serde(default)]
    pub(super) primary_modifier_on_macos: Option<String>,
    /// Each context's chord table, left as raw values.
    ///
    /// A chord maps either to one action name or to a class-qualified table,
    /// which serde expresses as an untagged enum — but an untagged enum reports
    /// only that a value "did not match any variant", naming neither the chord
    /// nor the key at fault. The loader interprets these itself so an operator
    /// is told which key was wrong rather than that something was.
    #[serde(flatten)]
    pub(super) contexts: BTreeMap<String, BTreeMap<String, toml::Value>>,
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
    /// Session-level environment applied to this session's spawned child. The
    /// most-specific merge layer: overrides colliding coder- and bundle-level
    /// names.
    #[serde(default)]
    pub(super) environment: Vec<NameValueEntry>,
    #[serde(default)]
    pub(super) ui: Option<RawSessionMarker>,
    #[serde(default)]
    pub(super) pubsub: Option<RawSessionMarker>,
}

/// A `[sessions.ui]` or `[sessions.pubsub]` subtable; an empty body is valid.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawSessionMarker {}
