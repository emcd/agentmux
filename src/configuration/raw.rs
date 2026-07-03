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
    /// wait as `SendOutcome::Timeout` if no observable output AND no
    /// operator interaction is active for the configured milliseconds.
    /// `None` (absent) preserves the unbounded wait.
    #[serde(default)]
    pub(super) prime_timeout_ms: Option<u64>,
    /// Per-coder wedge detection switch. Defaults to `true` (enabled) when
    /// absent; operators MAY set `false` to opt out and preserve the prior
    /// unbounded-wait behavior for a wedged pane.
    #[serde(default)]
    pub(super) wedge_detection: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct RawAcpTarget {
    pub(super) channel: AcpChannel,
    #[serde(default)]
    pub(super) command: Option<String>,
    #[serde(default)]
    pub(super) url: Option<String>,
    /// Per-coder bounded prime window for the ACP per-turn prompt
    /// completion wait. When `Some`, the ACP delivery task resolves
    /// the wait as `SendOutcome::Timeout` with
    /// `reason_code = "acp_turn_timeout"` if no terminal
    /// `PromptCompletion` AND no `pending_choice_outcome` is observed
    /// within the configured milliseconds. The per-target readiness
    /// is then latched to `Unavailable` (matching the
    /// `PromptCompletion::ConnectionClosed` path) and a
    /// `delivery_prime_timeout` inscription is emitted carrying
    /// `target_session`, `timeout_ms`, and `prime_wait_elapsed_ms`.
    /// The transport signals respawn-needed so the worker can
    /// re-bootstrap the runtime on the same path used for connection
    /// closure.
    ///
    /// The prime timer does NOT fire while a `pending_choice_outcome`
    /// is in flight; the timer continues to count down without firing
    /// until the choice resolves or the turn completes. Pending choice
    /// requests remain non-expiring as long as relay and worker state
    /// remain healthy.
    ///
    /// Absent (default) preserves the unbounded wait. TOML key under
    /// `[coders.<id>.acp]` is `prime-timeout-ms` — symmetric with the
    /// Tmux-side `[coders.<id>.tmux].prime-timeout-ms` key; the table
    /// itself namespaces the transport.
    ///
    /// Renamed from the legacy `turn-timeout-ms` (pre-existing field
    /// that was declared but never consumed by the runtime; the new
    /// name makes the field load-bearing on the ACP delivery task).
    /// Legacy configs using `turn-timeout-ms` fail the
    /// `deny_unknown_fields` check at bundle load.
    #[serde(default)]
    pub(super) prime_timeout_ms: Option<u64>,
    #[serde(default)]
    pub(super) headers: Vec<NameValueEntry>,
    #[serde(default)]
    pub(super) environment: Vec<NameValueEntry>,
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
    pub(super) wedge_detection: Option<bool>,
}

#[derive(Clone, Debug)]
pub(super) struct AcpTarget {
    pub(super) channel: AcpChannel,
    pub(super) command: Option<String>,
    pub(super) url: Option<String>,
    pub(super) prime_timeout_ms: Option<u64>,
    pub(super) headers: Vec<NameValueEntry>,
    pub(super) environment: Vec<NameValueEntry>,
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
