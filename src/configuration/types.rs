use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::roots::{ConfigurationRoots, LAYER_SEPARATOR};

pub const RESERVED_GROUP_ALL: &str = "ALL";

/// Environment variable carrying the bundle identity of the bundle hosting a
/// member. Part of the bring-up context stamped by [`BringUpContext`].
pub const BUNDLE_ENVIRONMENT_VARIABLE: &str = "AGENTMUX_BUNDLE";

/// Environment variable carrying the sender-session identity of a member.
/// Part of the bring-up context stamped by [`BringUpContext`].
pub const SESSION_ENVIRONMENT_VARIABLE: &str = "AGENTMUX_SESSION";

/// Environment variable naming the state root of the relay which spawned a
/// member, and the environment tier of state-root resolution.
///
/// Not part of [`BringUpContext`]: its value belongs to the relay performing
/// the spawn rather than to the configuration being loaded, so it is injected
/// at spawn rather than stamped at load. It is declared here with the other two
/// so [`INHERITED_CONTEXT_VARIABLE_NAMES`] can be built from one list — a name
/// held elsewhere is a name that list silently omits.
pub const STATE_DIRECTORY_ENVIRONMENT_VARIABLE: &str = "AGENTMUX_STATE_DIRECTORY";

/// Environment variable carrying the configuration layer list of the relay
/// which spawned a member, and the environment tier of configuration-layer
/// resolution. Carries a [`LAYER_SEPARATOR`]-delimited list.
///
/// Part of the bring-up context stamped by [`BringUpContext`], upsert-if-absent
/// like the bundle and session names rather than authoritatively like the state
/// root. A member holding a divergent configuration root still addresses and
/// authenticates to the relay that spawned it, because the socket, the session
/// and peer pre-shared keys, and the principal store all resolve beneath the
/// state root; what diverges is the set of declarations it reads.
pub const CONFIGURATION_DIRECTORY_ENVIRONMENT_VARIABLE: &str = "AGENTMUX_CONFIGURATION_DIRECTORY";

/// Names of every agentmux context variable a spawned child may inherit.
///
/// Distinct from [`BringUpContext::VARIABLE_NAMES`], which enumerates only what
/// configuration load stamps. Consumers that *sanitize* inherited context need
/// the wider set: a test harness clearing the developer's own environment must
/// clear the state root too, or the suite silently resolves against whichever
/// relay launched it. Consumers that count or assert what load stamps need the
/// narrower one.
pub const INHERITED_CONTEXT_VARIABLE_NAMES: &[&str] = &[
    BUNDLE_ENVIRONMENT_VARIABLE,
    SESSION_ENVIRONMENT_VARIABLE,
    STATE_DIRECTORY_ENVIRONMENT_VARIABLE,
    CONFIGURATION_DIRECTORY_ENVIRONMENT_VARIABLE,
];

/// Authoritative context which bring-up holds about a member it is starting.
///
/// Configuration load stamps this context onto an agent-spawning member's
/// merged environment. The launched agent propagates it to its
/// `agentmux host mcp` subprocess, which consults it instead of inferring an
/// identity from the filesystem. Bring-up is the only party which knows this
/// authoritatively, so the context outranks every configuration file while
/// still yielding to explicit invocation intent.
///
/// Further context is carried by adding a field here and a corresponding pair
/// in [`Self::environment_entries`]; the stamping mechanism itself is agnostic
/// to which variables it carries.
#[derive(Clone, Copy, Debug)]
pub struct BringUpContext<'a> {
    /// Bundle hosting the member.
    pub bundle_name: &'a str,
    /// Member id, which is the sender-session identity.
    pub session_id: &'a str,
    /// Configuration layers the relay resolved, in list order.
    pub configuration_roots: &'a ConfigurationRoots,
}

/// A context value in the form it is carried, before it is written.
///
/// Values that are already strings are carried as such. The layer list is not:
/// rendering it means joining the layers into the delimited environment form,
/// which allocates and can fail when a layer cannot be represented faithfully.
///
/// The distinction exists so the render happens where the entry is written
/// rather than where the entries are enumerated. Enumeration produces every
/// pair, and the stamping loop then discards the ones whose name is already
/// declared; rendering during enumeration would evaluate a representation for
/// members that never receive it, and reject a configuration that an operator
/// declaration would have satisfied.
#[derive(Clone, Copy, Debug)]
pub enum ContextValue<'a> {
    /// Already in its stamped form.
    Text(&'a str),
    /// Rendered by joining the layers with [`LAYER_SEPARATOR`].
    Layers(&'a ConfigurationRoots),
}

/// Why a layer cannot survive the round trip through the environment form.
///
/// Both faults are silent if forced. Replacing undecodable bytes would stamp a
/// path that resolves to a different directory than the one the relay read;
/// splitting on an embedded separator would stamp layers the operator never
/// declared. Either way the member reads configuration the relay did not
/// select, which is the divergence the stamp exists to close.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerRepresentationFault {
    /// The path is not UTF-8, and the environment carries `String`.
    NotUnicode,
    /// The path contains [`LAYER_SEPARATOR`], indistinguishable from the
    /// boundary between two layers once joined.
    HoldsSeparator,
}

/// A layer list that cannot be expressed in the delimited environment form.
///
/// Carries the offending layer so the caller can name it. The repeatable
/// command-line flag expresses such a path for any deployment that does not
/// need the value stamped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnrepresentableLayer {
    pub layer: PathBuf,
    pub fault: LayerRepresentationFault,
}

impl<'a> ContextValue<'a> {
    /// Renders the value into the form written to a member's environment.
    ///
    /// # Errors
    ///
    /// Returns [`UnrepresentableLayer`] when a layer cannot survive the round
    /// trip: see [`LayerRepresentationFault`]. The conversion is exact, never
    /// lossy — a layer this cannot represent faithfully is reported rather
    /// than approximated, because an approximation names a directory the relay
    /// did not read.
    pub fn render(self) -> Result<String, UnrepresentableLayer> {
        match self {
            Self::Text(value) => Ok(value.to_string()),
            Self::Layers(roots) => {
                let mut rendered = Vec::with_capacity(roots.layers().len());
                for layer in roots.layers() {
                    let Some(text) = layer.to_str() else {
                        return Err(UnrepresentableLayer {
                            layer: layer.clone(),
                            fault: LayerRepresentationFault::NotUnicode,
                        });
                    };
                    if text.contains(LAYER_SEPARATOR) {
                        return Err(UnrepresentableLayer {
                            layer: layer.clone(),
                            fault: LayerRepresentationFault::HoldsSeparator,
                        });
                    }
                    rendered.push(text);
                }
                Ok(rendered.join(&LAYER_SEPARATOR.to_string()))
            }
        }
    }
}

impl<'a> BringUpContext<'a> {
    /// Names of every environment variable this context carries, and therefore
    /// of everything configuration load stamps.
    ///
    /// Enumerated apart from [`Self::environment_entries`] for consumers which
    /// need the names without a context to populate them. The two are held in
    /// agreement by test, so extending one without the other fails rather than
    /// silently leaving a variable unhandled.
    ///
    /// This is the load-time set, not everything a child inherits. A consumer
    /// sanitizing inherited context wants [`INHERITED_CONTEXT_VARIABLE_NAMES`].
    pub const VARIABLE_NAMES: &'static [&'static str] = &[
        BUNDLE_ENVIRONMENT_VARIABLE,
        SESSION_ENVIRONMENT_VARIABLE,
        CONFIGURATION_DIRECTORY_ENVIRONMENT_VARIABLE,
    ];

    /// Environment name/value pairs representing this context, each value in
    /// the form it is carried rather than the form it is written. See
    /// [`ContextValue`] for why the two differ.
    #[must_use]
    pub fn environment_entries(&self) -> Vec<(&'static str, ContextValue<'a>)> {
        vec![
            (
                BUNDLE_ENVIRONMENT_VARIABLE,
                ContextValue::Text(self.bundle_name),
            ),
            (
                SESSION_ENVIRONMENT_VARIABLE,
                ContextValue::Text(self.session_id),
            ),
            (
                CONFIGURATION_DIRECTORY_ENVIRONMENT_VARIABLE,
                ContextValue::Layers(self.configuration_roots),
            ),
        ]
    }
}

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
    /// Merged environment for this member's spawned child, resolved once at
    /// configuration load from the coder, bundle, and session layers
    /// (precedence session > bundle > coder). Each spawning transport applies
    /// these at spawn; inert on non-spawning targets (ACP `http`, ui/pubsub).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<NameValueEntry>,
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

    /// Reports whether this target spawns an agent process inheriting the
    /// member's environment.
    #[must_use]
    pub fn spawns_agent(&self) -> bool {
        match self {
            Self::Tmux(_) | Self::Acp(_) | Self::Pty(_) => true,
            Self::Ui | Self::Pubsub => false,
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

/// Pty transport configuration for one bundle member.
///
/// Pty spawns the child process under a portable-pty PTY and feeds
/// terminal output through libghostty-vt. Per-coder config keys live
/// under `[coders.<id>.pty]`. The validator rejects `cols = 0`,
/// `rows = 0`, and the mutual-exclusion rule
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
    /// Per-coder grid columns. Default 120.
    #[serde(default = "pty_cols_default")]
    pub cols: u16,
    /// Per-coder grid rows. Default 40.
    #[serde(default = "pty_rows_default")]
    pub rows: u16,
    /// Per-coder terminal protocol. Selects the literal `TERM`
    /// environment-variable value the Pty transport sets when
    /// spawning the child. Defaults to `xterm-256color` (preserves
    /// the pre-existing behavior).
    #[serde(default)]
    pub term_protocol: TermProtocol,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<NameValueEntry>,
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

/// Global user identity/policy configuration loaded from `users.toml`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct TuiConfiguration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_session: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<TuiSession>,
}

/// UI-surface operational defaults loaded from `ui.toml`.
///
/// Distinct from identity/policy (`users.toml`): `ui.toml` holds operator
/// surface preferences. Today that is only the default browsing bundle; the
/// file is designed to grow additional surface keys (theme, default screen
/// mode) later without mixing them into the identity file.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct UiConfiguration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_bundle: Option<String>,
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

/// Per-coder terminal protocol selection for the Pty transport.
///
/// Each variant maps 1:1 to the literal `TERM` environment-variable
/// string the Pty transport sets when spawning the child coder
/// process. The closed-enum shape keeps the schema self-validating
/// (an unknown value fails serde's enum-variant deserializer with a
/// structured "unknown variant" error).
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum TermProtocol {
    #[default]
    #[serde(rename = "xterm-256color")]
    Xterm256Color,
    #[serde(rename = "xterm-kitty")]
    XtermKitty,
    #[serde(rename = "alacritty")]
    Alacritty,
    #[serde(rename = "foot")]
    Foot,
    #[serde(rename = "wezterm")]
    WezTerm,
    #[serde(rename = "screen-256color")]
    Screen256Color,
}

impl TermProtocol {
    /// The literal `TERM` env-var value this variant emits.
    #[must_use]
    pub const fn as_env_var(self) -> &'static str {
        match self {
            Self::Xterm256Color => "xterm-256color",
            Self::XtermKitty => "xterm-kitty",
            Self::Alacritty => "alacritty",
            Self::Foot => "foot",
            Self::WezTerm => "wezterm",
            Self::Screen256Color => "screen-256color",
        }
    }
}
