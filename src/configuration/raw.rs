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
    /// Per-coder bounded prime window for the tmux quiescence wait. When
    /// `Some`, the tmux transport's internal delivery task resolves the
    /// wait as `SendOutcome::Timeout` if no observable output is produced
    /// within the configured milliseconds. `None` (absent) issues no
    /// prime-window verdict; it does not make the wait unbounded, because
    /// `readiness_timeout_ms` applies regardless.
    #[serde(default)]
    pub(super) prime_timeout_ms: Option<u64>,
    /// Per-coder bound on the entire readiness wait for a flush group.
    /// Absent takes [`TMUX_READINESS_TIMEOUT_MS_DEFAULT`]; a present value
    /// is validated against [`TMUX_READINESS_TIMEOUT_MS_RANGE`].
    #[serde(default)]
    pub(super) readiness_timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct RawAcpTarget {
    pub(super) channel: AcpChannel,
    #[serde(default)]
    pub(super) command: Option<String>,
    #[serde(default)]
    pub(super) url: Option<String>,
    /// Per-coder `prime_timeout_ms` for cross-transport symmetry. ACP has
    /// no elapsed-time bound of its own today; the field is still
    /// accepted at the typed-config layer and forwarded onto the
    /// shared `DeliveryEnvelope.prime_timeout_ms` so the loader does
    /// not reject legacy bundle configs, but the ACP transport does
    /// not consume or bound on it. The field is retained pending a
    /// coordinated cross-transport removal.
    ///
    /// TOML key under `[coders.<id>.acp]` is `prime-timeout-ms`. Legacy
    /// `turn-timeout-ms` configs fail the raw loader's
    /// `deny_unknown_fields` check at bundle load.
    #[serde(default)]
    pub(super) prime_timeout_ms: Option<u64>,
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
    /// Per-coder bounded prime window for the Pty quiescence wait.
    /// Same semantics as the Tmux / ACP `prime_timeout_ms` field.
    #[serde(default)]
    pub(super) prime_timeout_ms: Option<u64>,
    /// Per-coder wedge detection switch. Defaults to `true` (enabled)
    /// when absent; operators MAY set `false` to opt out and preserve
    /// the prior unbounded-wait behavior for a wedged pane.
    #[serde(default)]
    pub(super) wedge_detection: Option<bool>,
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
    pub(super) prime_timeout_ms: Option<u64>,
    pub(super) readiness_timeout_ms: Option<u64>,
}

#[derive(Clone, Debug)]
pub(super) struct AcpTarget {
    pub(super) channel: AcpChannel,
    pub(super) command: Option<String>,
    pub(super) url: Option<String>,
    pub(super) prime_timeout_ms: Option<u64>,
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
    pub(super) prime_timeout_ms: Option<u64>,
    pub(super) wedge_detection: Option<bool>,
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
