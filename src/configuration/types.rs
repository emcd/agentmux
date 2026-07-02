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
    Pty,
    Ui,
    Pubsub,
}

/// Transport capability derivation (Transport Capability Contract).
///
/// Capabilities are pure functions of the transport type — never stored bool
/// fields — derived at check time from the configuration enum discriminant:
///
/// | Transport | `can_be_looked` | `can_be_written` | `can_stream_output` | `gives_choices` |
/// |-----------|-----------------|------------------|---------------------|-----------------|
/// | `Tmux`    | true            | true             | false               | false           |
/// | `Acp`     | true            | true             | true                | true            |
/// | `Pty`     | true            | true             | true                | false           |
/// | `Ui`      | false           | false            | false               | false           |
/// | `Pubsub`  | false           | false            | false               | false           |
///
/// The `Pty` row is normative and forward-looking: no `Pty` variant exists
/// yet. It is the long-term replacement for `Tmux` with identical
/// `can_be_looked`/`can_be_written` capabilities and `can_stream_output =
/// true` (PTY natively streams output byte-by-byte; tmux requires periodic
/// snapshot polling). The row activates when the `Pty` transport lands
/// (expected in `decouple-transport-layer`).
impl SessionType {
    /// The session can be targeted by `look`: its transport supports snapshot
    /// capture.
    #[must_use]
    pub fn can_be_looked(self) -> bool {
        match self {
            Self::Tmux | Self::Acp | Self::Pty => true,
            Self::Ui | Self::Pubsub => false,
        }
    }

    /// The session can be targeted by `raww`: its transport supports raw input
    /// injection.
    #[must_use]
    pub fn can_be_written(self) -> bool {
        match self {
            Self::Tmux | Self::Acp | Self::Pty => true,
            Self::Ui | Self::Pubsub => false,
        }
    }

    /// The session's transport natively produces live output chunks. Advertised
    /// ahead of any consumer; streaming look semantics are a follow-on proposal.
    #[must_use]
    pub fn can_stream_output(self) -> bool {
        match self {
            Self::Acp | Self::Pty => true,
            Self::Tmux | Self::Ui | Self::Pubsub => false,
        }
    }

    /// The session's transport can surface choice requests (ACP-style option
    /// arrays for operator/UI resolution). Describes choice production, not
    /// resolution authority: the `choices.list`/`choices.pick` paths address
    /// the choice record queue, so no request handler gates on this method.
    #[must_use]
    pub fn can_give_choices(self) -> bool {
        match self {
            Self::Acp => true,
            Self::Tmux | Self::Pty | Self::Ui | Self::Pubsub => false,
        }
    }
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
/// `Tmux`, `Acp`, and `Pty` carry transport configuration; `Ui` is
/// first-class (`src/transports/ui.rs`); only `Pubsub` remains
/// unimplemented.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "transport", content = "config")]
pub enum TargetConfiguration {
    Tmux(TmuxTargetConfiguration),
    Acp(AcpTargetConfiguration),
    Pty(PtyTargetConfiguration),
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
            Self::Pty(_) => SessionType::Pty,
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
    /// Per-coder bounded prime window for the quiescence wait. When `Some`,
    /// the tmux transport's internal delivery task resolves the wait as
    /// `SendOutcome::Timeout` if no observable output AND no operator
    /// interaction is active for the configured milliseconds. `None` (or
    /// absent) preserves the unbounded wait. The key lives under
    /// `[coders.<id>.tmux]` and is called `prime-timeout-ms` in TOML; this
    /// field carries the integer-millisecond form.
    ///
    /// Mirrored onto [`crate::transports::DeliveryEnvelope::prime_timeout_ms`]
    /// by the relay dispatch worker for tmux targets; the ACP follow-up
    /// consumes the same envelope field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prime_timeout_ms: Option<u64>,
    /// Per-coder wedge detection switch. When `true` (default), the tmux
    /// transport resolves a quiescent + not-prompt-ready + no-operator-
    /// interaction state as `SendOutcome::Failed` with
    /// `reason_code = "pane_wedged"`. Operators MAY set this to `false` to
    /// preserve the prior unbounded-wait behavior. The key lives under
    /// `[coders.<id>.tmux]` and is called `wedge-detection` in TOML.
    ///
    /// Default-true is intentional: the cost of a silently-wedged pane
    /// (delivery queue growth) is higher than the cost of a recoverable
    /// false-positive wedge (operator restart). See
    /// `tmux-wedge-detection` design.md Decision 2.
    #[serde(default = "wedge_detection_default_true")]
    pub wedge_detection: bool,
}

/// Returns the default value for [`TmuxTargetConfiguration::wedge_detection`].
fn wedge_detection_default_true() -> bool {
    true
}

/// Pty transport configuration for one bundle member.
///
/// Pty spawns the child process under a portable-pty PTY and feeds
/// terminal output through libghostty-vt. Per-coder config keys live
/// under `[coders.<id>.pty]`. The validator rejects `cols = 0`,
/// `rows = 0`, `prime-timeout-ms = 0`, and the mutual-exclusion rule
/// between `[coders.<id>.pty]` and `[coders.<id>.tmux]` /
/// `[coders.<id>.acp]` (exactly one must be set, never both, never
/// neither).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PtyTargetConfiguration {
    /// Per-coder initial command. `{{coder-session-id}}` placeholder is
    /// replaced when the bundle member carries a `coder-session-id`.
    pub initial_command: String,
    /// Per-coder resume command. Selected when the bundle member
    /// carries a `coder-session-id` (i.e. the operator is resuming
    /// a previous agent session rather than starting fresh).
    pub resume_command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_readiness: Option<PromptReadinessTemplate>,
    /// Per-coder bounded prime window for the Pty quiescence wait.
    /// Same semantics as the Tmux / ACP `prime_timeout_ms` field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prime_timeout_ms: Option<u64>,
    /// Per-coder wedge detection switch (default true). Pty does NOT
    /// have an operator-interaction concept, so this only gates the
    /// wedge classifier (not prime-timeout, which fires on empty-
    /// pane mismatch regardless of operator state).
    #[serde(default = "wedge_detection_default_true")]
    pub wedge_detection: bool,
    /// Per-coder grid columns. Default 120.
    #[serde(default = "pty_cols_default")]
    pub cols: u16,
    /// Per-coder grid rows. Default 40.
    #[serde(default = "pty_rows_default")]
    pub rows: u16,
}

/// Default cols value for `PtyTargetConfiguration::cols`.
fn pty_cols_default() -> u16 {
    120
}

/// Default rows value for `PtyTargetConfiguration::rows`.
fn pty_rows_default() -> u16 {
    40
}

/// ACP transport configuration for one bundle member.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AcpTargetConfiguration {
    pub channel: AcpChannel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Per-coder bounded prime window for the ACP per-turn prompt
    /// completion wait. When `Some`, the ACP transport's internal
    /// delivery task resolves the wait as `SendOutcome::Timeout` with
    /// `reason_code = "acp_turn_timeout"` if no terminal
    /// `PromptCompletion` AND no `pending_choice_outcome` is observed
    /// for the configured milliseconds. The per-target readiness is
    /// then latched to `Unavailable` (matching the
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
    /// remain healthy; the prime timeout is a turn-wait bound, not a
    /// choice-decision lifecycle bound.
    ///
    /// Absent (default) preserves the unbounded wait. The key lives
    /// under `[coders.<id>.acp]` and is called `prime-timeout-ms` in
    /// TOML — symmetric with the Tmux-side
    /// `[coders.<id>.tmux].prime-timeout-ms` key; the table itself
    /// namespaces the transport.
    ///
    /// Mirrored onto [`crate::transports::DeliveryEnvelope::prime_timeout_ms`]
    /// by the relay dispatch worker for ACP targets; the Tmux transport
    /// consumes the same envelope field.
    ///
    /// Wedge detection is intentionally NOT applied to the ACP path:
    /// ACP does not have a tmux-style pane-wedge classifier (no
    /// snapshot polling, no operator-interaction concept in the same
    /// shape). The prime timeout is the only wedge-classifier surface
    /// on ACP in v1; a follow-up proposal may add wedge semantics if
    /// ACP runtime signals warrant them.
    ///
    /// Renamed from the pre-existing `turn_timeout_ms` field (which was
    /// never consumed by the runtime; the pre-existing field was dead
    /// baggage). The TOML key is therefore `prime-timeout-ms`, not
    /// `turn-timeout-ms`; legacy configs using `turn-timeout-ms` fail
    /// the raw loader's `deny_unknown_fields` check at bundle load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prime_timeout_ms: Option<u64>,
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
