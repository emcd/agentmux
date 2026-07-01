//! Policy loading: parse `policies.toml` / `relay.toml` into validated presets
//! and build the [`AuthorizationContext`] a request is authorized against.

use std::{collections::HashMap, fs, path::Path};

use serde::Deserialize;
use serde_json::json;

use crate::{
    configuration::{BundleConfiguration, load_tui_configuration},
    relay::{POLICIES_FILE, POLICIES_FORMAT_VERSION, RelayError, relay_error},
};

use super::context::{AuthorizationContext, PolicyControls, PolicyScope, UiSessionAuthorization};
use super::resolution::{normalize_policy_id, resolve_session_policy_controls};
use crate::relay::identity::split_principal_id;

const RELAY_PRINCIPAL_NAMESPACE: &str = "RELAY";

const RELAY_FILE: &str = "relay.toml";
const DEFAULT_CHOICES_PENDING_MAX: usize = 256;
const MIN_CHOICES_PENDING_MAX: usize = 1;
const MAX_CHOICES_PENDING_MAX: usize = 4096;
const DEFAULT_WATCH_BUNDLES: bool = true;
const DEFAULT_REQUIRE_SESSION_CREDENTIALS: bool = false;
const ENV_WATCH_BUNDLES: &str = "AGENTMUX_RELAY_WATCH_BUNDLES";
const ENV_REQUIRE_SESSION_CREDENTIALS: &str = "AGENTMUX_RELAY_REQUIRE_SESSION_CREDENTIALS";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawPoliciesFile {
    format_version: u32,
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    policies: Vec<RawPolicyPreset>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawPolicyPreset {
    id: String,
    #[serde(default, rename = "description")]
    _description: Option<String>,
    controls: RawPolicyControls,
}

/// Raw `relay.toml` shape. Relay-wide keys live at the file root — the file is
/// itself the relay configuration table, so a nested `[relay]` table is rejected
/// as an unknown field. `deny_unknown_fields` makes every typo or stale key a
/// fail-fast structured error rather than a silent default.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawRelayFile {
    #[serde(default)]
    watch_bundles: Option<bool>,
    #[serde(default)]
    require_session_credentials: Option<bool>,
    #[serde(default)]
    choices: Option<RawRelayChoicesSection>,
    /// This relay's own outbound identity, presented as `<relay-id>@RELAY` when
    /// dialing a configured peer. Required once any `[[peers]]` entry exists;
    /// validated as a bare relay id (see [`validate_bare_relay_id`]).
    #[serde(default)]
    relay_id: Option<String>,
    #[serde(default)]
    peers: Vec<RawPeerEntry>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawRelayChoicesSection {
    #[serde(default)]
    pending_max: Option<usize>,
}

/// Raw `[[peers]]` entry: an active outbound peer relay endpoint. `id` is the
/// peer's canonical `<id>@RELAY` principal and `address` its listening endpoint —
/// in this slice an absolute Unix domain socket path (TCP is future). The table
/// is outbound-only and carries no `scope`: inbound cross-relay authorization is
/// the scope set by `new peer <id>@RELAY --scope` and read by the ingress filter,
/// so `deny_unknown_fields` rejects a stray `scope` key here.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawPeerEntry {
    id: String,
    address: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawPolicyControls {
    find: String,
    list: String,
    look: String,
    send: String,
    #[serde(default = "default_raww_policy_scope")]
    raww: String,
    #[serde(default = "default_choose_policy_scope")]
    choose: String,
    #[serde(default = "default_updown_policy_scope")]
    updown: String,
    #[serde(default, rename = "do")]
    do_controls: HashMap<String, String>,
    #[serde(default, rename = "new")]
    new_controls: HashMap<String, String>,
    #[serde(default, rename = "change")]
    change_controls: HashMap<String, String>,
}

fn default_raww_policy_scope() -> String {
    "none".to_string()
}

fn default_choose_policy_scope() -> String {
    "none".to_string()
}

fn default_updown_policy_scope() -> String {
    "none".to_string()
}

/// Loads and validates the authorization policy presets and the configured
/// default policy id, independent of any bundle.
///
/// Shared by the per-bundle `load_authorization_context` and the relay-wide
/// `authorize_relay_action` path, which authorizes namespace-agnostic operator
/// actions (`new.peer`, `change.psk`) that have no bundle context.
pub(super) fn load_policy_presets(
    configuration_root: &Path,
) -> Result<(HashMap<String, PolicyControls>, Option<String>), RelayError> {
    let policies_path = configuration_root.join(POLICIES_FILE);
    let policies_raw = fs::read_to_string(&policies_path).map_err(|source| {
        relay_error(
            "validation_invalid_arguments",
            "failed to load authorization policy artifact",
            Some(json!({
                "path": policies_path.display().to_string(),
                "cause": source.to_string(),
            })),
        )
    })?;
    let policies_file = toml::from_str::<RawPoliciesFile>(&policies_raw).map_err(|source| {
        relay_error(
            "validation_invalid_arguments",
            "failed to parse authorization policy artifact",
            Some(json!({
                "path": policies_path.display().to_string(),
                "cause": source.to_string(),
            })),
        )
    })?;
    if policies_file.format_version != POLICIES_FORMAT_VERSION {
        return Err(relay_error(
            "validation_invalid_arguments",
            "authorization policy artifact has unsupported format-version",
            Some(json!({
                "path": policies_path.display().to_string(),
                "format_version": policies_file.format_version,
            })),
        ));
    }

    let mut presets = HashMap::<String, PolicyControls>::new();
    for policy in policies_file.policies {
        let policy_id = normalize_policy_id(policy.id.as_str()).ok_or_else(|| {
            relay_error(
                "validation_invalid_arguments",
                "policy id must be non-empty",
                Some(json!({
                    "path": policies_path.display().to_string(),
                })),
            )
        })?;
        if presets.contains_key(policy_id) {
            return Err(relay_error(
                "validation_invalid_arguments",
                "authorization policy id must be unique",
                Some(json!({
                    "path": policies_path.display().to_string(),
                    "policy_id": policy_id,
                })),
            ));
        }
        let controls = parse_policy_controls(policy.controls, policies_path.as_path(), policy_id)?;
        presets.insert(policy_id.to_string(), controls);
    }

    let default_policy_id = policies_file
        .default
        .as_deref()
        .and_then(normalize_policy_id)
        .map(ToString::to_string);
    if let Some(default_policy_id) = default_policy_id.as_deref()
        && !presets.contains_key(default_policy_id)
    {
        return Err(relay_error(
            "validation_invalid_arguments",
            "authorization default policy references unknown policy id",
            Some(json!({
                "path": policies_path.display().to_string(),
                "policy_id": default_policy_id,
            })),
        ));
    }
    Ok((presets, default_policy_id))
}

pub(in crate::relay) fn load_authorization_context(
    configuration_root: &Path,
    bundle: Option<&BundleConfiguration>,
) -> Result<AuthorizationContext, RelayError> {
    let policies_path = configuration_root.join(POLICIES_FILE);
    let (presets, default_policy_id) = load_policy_presets(configuration_root)?;

    let choices_pending_max =
        load_relay_file_configuration(configuration_root)?.choices_pending_max;

    // A relay-wide (`GLOBAL`) home namespace has no bundle members; its
    // requester controls come entirely from the operator policy loaded below
    // (the TUI session presets), independent of any bundle.
    let conservative_default = PolicyControls::conservative_default();
    let mut controls_by_session =
        HashMap::with_capacity(bundle.map_or(0, |bundle| bundle.members.len()));
    if let Some(bundle) = bundle {
        for member in &bundle.members {
            let controls = resolve_session_policy_controls(
                member,
                &presets,
                default_policy_id.as_deref(),
                &conservative_default,
                policies_path.as_path(),
            )?;
            controls_by_session.insert(member.id.clone(), controls.clone());
        }
    }
    let mut ui_sessions = HashMap::<String, UiSessionAuthorization>::new();
    if let Some(tui_configuration) =
        load_tui_configuration(configuration_root).map_err(map_tui_configuration_error)?
    {
        for session in tui_configuration.sessions {
            let session_id = session.id.clone();
            let policy_id = normalize_policy_id(session.policy.as_str()).ok_or_else(|| {
                relay_error(
                    "validation_unknown_policy",
                    "ui session policy reference is empty",
                    Some(json!({
                        "session_selector": session_id.as_str(),
                        "session_id": session_id.as_str(),
                    })),
                )
            })?;
            let controls = presets.get(policy_id).ok_or_else(|| {
                relay_error(
                    "validation_unknown_policy",
                    "ui session policy references unknown policy id",
                    Some(json!({
                        "session_selector": session_id.as_str(),
                        "session_id": session_id.as_str(),
                        "policy_id": policy_id,
                    })),
                )
            })?;
            if let Some(existing_controls) = controls_by_session.get(session_id.as_str())
                && existing_controls != controls
            {
                return Err(relay_error(
                    "validation_invalid_arguments",
                    "session_id maps to conflicting authorization policies",
                    Some(json!({
                        "session_id": session_id.as_str(),
                    })),
                ));
            }
            controls_by_session.insert(session_id.clone(), controls.clone());
            ui_sessions
                .entry(session_id)
                .and_modify(|existing| {
                    if existing.display_name.is_none() {
                        existing.display_name = session.name.clone();
                    }
                })
                .or_insert(UiSessionAuthorization {
                    display_name: session.name.clone(),
                });
        }
    }
    Ok(AuthorizationContext {
        controls_by_session,
        ui_sessions,
        choices_pending_max,
    })
}

/// Fully-resolved relay-wide runtime configuration, after applying the
/// precedence ladder (CLI override > environment override > `relay.toml` >
/// documented defaults). This is the single normalized object relay startup and
/// `agentmux check configuration` read; consumers MUST NOT re-parse `relay.toml`
/// or re-apply defaulting/precedence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayRuntimeConfiguration {
    pub watch_bundles: bool,
    pub require_session_credentials: bool,
    pub choices_pending_max: usize,
    /// This relay's own outbound identity (`<relay-id>@RELAY`), present iff
    /// `relay-id` is configured. Required whenever `peers` is non-empty.
    pub relay_id: Option<String>,
    pub peers: Vec<PeerConfiguration>,
}

/// A validated `[[peers]]` entry naming an outbound peer relay. `id` is the
/// peer's canonical `<id>@RELAY` principal and `address` an absolute Unix domain
/// socket path. The bare id portion (see [`PeerConfiguration::alias`]) is both
/// the bang-path `!<relay_id>` selector and the peer credential filename stem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerConfiguration {
    pub id: String,
    pub address: String,
}

impl PeerConfiguration {
    /// Returns the bare id portion of the peer's canonical `<id>@RELAY`
    /// principal — the value matched against the bang-path `!<relay_id>` suffix
    /// and used as the `<peer_alias>` in the credential path. Falls back to the
    /// whole id if it is somehow unqualified; load-time validation guarantees the
    /// `@RELAY` form, so this is defensive.
    #[must_use]
    pub fn alias(&self) -> &str {
        split_principal_id(self.id.as_str())
            .map(|(local, _)| local)
            .unwrap_or(self.id.as_str())
    }
}

/// File-derived relay settings before override resolution. `watch_bundles` and
/// `require_session_credentials` stay `Option` so the precedence ladder can tell
/// "absent in file" apart from an explicit value; `choices_pending_max` and
/// `peers` have no override layer and are resolved (and validated) here.
struct RelayFileConfiguration {
    watch_bundles: Option<bool>,
    require_session_credentials: Option<bool>,
    choices_pending_max: usize,
    relay_id: Option<String>,
    peers: Vec<PeerConfiguration>,
}

/// Loads and resolves the relay-wide runtime configuration from
/// `<config-root>/relay.toml`, applying the precedence ladder
/// (CLI override > environment override > `relay.toml` > documented defaults)
/// for the boolean controls. A missing file yields the documented defaults; a
/// malformed file, unknown field, wrong field type, out-of-range choices bound,
/// invalid environment override, or invalid peer entry fails fast with a
/// structured [`RelayError`].
pub fn load_relay_runtime_configuration(
    configuration_root: &Path,
    cli_watch_bundles: Option<bool>,
    cli_require_session_credentials: Option<bool>,
) -> Result<RelayRuntimeConfiguration, RelayError> {
    let file = load_relay_file_configuration(configuration_root)?;
    let environment_watch_bundles = relay_bool_env_override(ENV_WATCH_BUNDLES)?;
    let environment_require_session_credentials =
        relay_bool_env_override(ENV_REQUIRE_SESSION_CREDENTIALS)?;
    Ok(RelayRuntimeConfiguration {
        watch_bundles: resolve_relay_bool_setting(
            cli_watch_bundles,
            environment_watch_bundles,
            file.watch_bundles,
            DEFAULT_WATCH_BUNDLES,
        ),
        require_session_credentials: resolve_relay_bool_setting(
            cli_require_session_credentials,
            environment_require_session_credentials,
            file.require_session_credentials,
            DEFAULT_REQUIRE_SESSION_CREDENTIALS,
        ),
        choices_pending_max: file.choices_pending_max,
        relay_id: file.relay_id,
        peers: file.peers,
    })
}

/// Resolves one boolean relay setting through the precedence ladder: a CLI
/// override wins, then an environment override, then the `relay.toml` value,
/// then the documented default. Pure so the precedence is unit-testable without
/// touching process state.
#[must_use]
pub fn resolve_relay_bool_setting(
    cli_override: Option<bool>,
    environment_override: Option<bool>,
    file_value: Option<bool>,
    default: bool,
) -> bool {
    cli_override
        .or(environment_override)
        .or(file_value)
        .unwrap_or(default)
}

/// Parses a canonical boolean override value (`true`/`false` only) for the named
/// environment variable, surfacing a structured error for anything else. Split
/// from process-env reading so the validation is unit-testable without mutating
/// process state.
pub fn parse_relay_bool_env_value(variable: &str, value: &str) -> Result<bool, RelayError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(relay_error(
            "validation_invalid_arguments",
            "relay environment override must be exactly 'true' or 'false'",
            Some(json!({
                "variable": variable,
                "value": value,
                "expected": ["true", "false"],
            })),
        )),
    }
}

/// Reads a canonical boolean environment override. Returns `Ok(None)` when the
/// variable is absent (or holds non-UTF-8, treated as absent), `Ok(Some(_))` for
/// exactly `true`/`false`, and a structured error for any other value.
fn relay_bool_env_override(variable: &str) -> Result<Option<bool>, RelayError> {
    match std::env::var(variable).ok() {
        None => Ok(None),
        Some(value) => parse_relay_bool_env_value(variable, value.as_str()).map(Some),
    }
}

/// Parses and validates `relay.toml` into file-derived settings without applying
/// overrides. A missing file yields documented defaults with no configured
/// peers. Choices range and peer-entry validation happen here so both relay
/// startup and `agentmux check configuration` (which loads through this path)
/// report the same structured errors.
fn load_relay_file_configuration(
    configuration_root: &Path,
) -> Result<RelayFileConfiguration, RelayError> {
    let path = configuration_root.join(RELAY_FILE);
    if !path.exists() {
        return Ok(RelayFileConfiguration {
            watch_bundles: None,
            require_session_credentials: None,
            choices_pending_max: DEFAULT_CHOICES_PENDING_MAX,
            relay_id: None,
            peers: Vec::new(),
        });
    }
    let raw = fs::read_to_string(&path).map_err(|source| {
        relay_error(
            "validation_invalid_arguments",
            "failed to load relay configuration",
            Some(json!({
                "path": path.display().to_string(),
                "cause": source.to_string(),
            })),
        )
    })?;
    let parsed = toml::from_str::<RawRelayFile>(raw.as_str()).map_err(|source| {
        relay_error(
            "validation_invalid_arguments",
            "failed to parse relay configuration",
            Some(json!({
                "path": path.display().to_string(),
                "cause": source.to_string(),
            })),
        )
    })?;
    let choices_pending_max = parsed
        .choices
        .and_then(|choices| choices.pending_max)
        .unwrap_or(DEFAULT_CHOICES_PENDING_MAX);
    if !(MIN_CHOICES_PENDING_MAX..=MAX_CHOICES_PENDING_MAX).contains(&choices_pending_max) {
        return Err(relay_error(
            "validation_invalid_arguments",
            "relay choices pending-max is out of supported range",
            Some(json!({
                "path": path.display().to_string(),
                "field": "choices.pending-max",
                "value": choices_pending_max,
                "minimum": MIN_CHOICES_PENDING_MAX,
                "maximum": MAX_CHOICES_PENDING_MAX,
            })),
        ));
    }
    let mut peers = Vec::with_capacity(parsed.peers.len());
    for (index, peer) in parsed.peers.into_iter().enumerate() {
        let alias = validate_peer_id(peer.id.as_str(), index, path.as_path())?;
        let address = peer.address.trim();
        if address.is_empty() {
            return Err(relay_error(
                "validation_invalid_arguments",
                "relay peer address must be a non-empty string",
                Some(json!({
                    "path": path.display().to_string(),
                    "field": "peers.address",
                    "peer_index": index,
                })),
            ));
        }
        // The relay serves only a Unix domain socket today, so a peer endpoint is
        // an absolute filesystem path. Reject a relative path or a TCP-style
        // `host:port` form (which is not absolute) with a pointed message; remote
        // peering is deferred (see the change's Non-Goals).
        if !Path::new(address).is_absolute() {
            return Err(relay_error(
                "validation_invalid_arguments",
                "relay peer address must be an absolute Unix socket path (TCP host:port is not yet supported)",
                Some(json!({
                    "path": path.display().to_string(),
                    "field": "peers.address",
                    "peer_index": index,
                    "value": address,
                })),
            ));
        }
        peers.push(PeerConfiguration {
            id: format!("{alias}@{RELAY_PRINCIPAL_NAMESPACE}"),
            address: address.to_string(),
        });
    }
    // `relay-id` names this relay's outbound identity. It is required once any
    // peer is configured (the relay could not present an identity to dial one
    // otherwise) and grammar-checked whenever present so a malformed value fails
    // fast rather than surfacing only on the first outbound delivery.
    let relay_id = match parsed.relay_id {
        Some(raw) => Some(validate_bare_relay_id(
            raw.as_str(),
            "relay-id",
            path.as_path(),
        )?),
        None => None,
    };
    if relay_id.is_none() && !peers.is_empty() {
        return Err(relay_error(
            "validation_invalid_arguments",
            "relay-id is required when [[peers]] are configured",
            Some(json!({
                "path": path.display().to_string(),
                "field": "relay-id",
            })),
        ));
    }
    Ok(RelayFileConfiguration {
        watch_bundles: parsed.watch_bundles,
        require_session_credentials: parsed.require_session_credentials,
        choices_pending_max,
        relay_id,
        peers,
    })
}

/// Validates a peer entry's `id` as a canonical `<id>@RELAY` principal and
/// returns its bare id portion. The namespace partition must be `RELAY` and the
/// bare local part must satisfy the relay-id grammar.
fn validate_peer_id(raw: &str, peer_index: usize, path: &Path) -> Result<String, RelayError> {
    let trimmed = raw.trim();
    let Some((local, namespace)) = split_principal_id(trimmed) else {
        return Err(relay_error(
            "validation_invalid_arguments",
            "relay peer id must be a canonical <id>@RELAY principal",
            Some(json!({
                "path": path.display().to_string(),
                "field": "peers.id",
                "peer_index": peer_index,
                "value": raw,
            })),
        ));
    };
    if namespace != RELAY_PRINCIPAL_NAMESPACE {
        return Err(relay_error(
            "validation_invalid_arguments",
            "relay peer id must be in the RELAY namespace (<id>@RELAY)",
            Some(json!({
                "path": path.display().to_string(),
                "field": "peers.id",
                "peer_index": peer_index,
                "value": raw,
            })),
        ));
    }
    validate_bare_relay_id(local, "peers.id", path)
}

/// Validates a bare relay id — the local part of a `<id>@RELAY` principal and the
/// value of the top-level `relay-id` key. The grammar is deliberately strict: the
/// id becomes a peer credential filename stem (`<id>.psk`) and is matched against
/// the bang-path `!<relay_id>` suffix, so it must be non-empty after trimming and
/// free of the `@` namespace qualifier, the `!` bang-path separator, and any path
/// separator. Returns the trimmed id.
fn validate_bare_relay_id(raw: &str, field: &str, path: &Path) -> Result<String, RelayError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(relay_error(
            "validation_invalid_arguments",
            "relay id must be non-empty",
            Some(json!({
                "path": path.display().to_string(),
                "field": field,
            })),
        ));
    }
    if value.contains('@') || value.contains('!') || value.contains('/') {
        return Err(relay_error(
            "validation_invalid_arguments",
            "relay id must not contain '@', '!', or a path separator",
            Some(json!({
                "path": path.display().to_string(),
                "field": field,
                "value": value,
            })),
        ));
    }
    Ok(value.to_string())
}

fn parse_policy_controls(
    controls: RawPolicyControls,
    policies_path: &Path,
    policy_id: &str,
) -> Result<PolicyControls, RelayError> {
    let find = parse_scope_for_control(
        controls.find.as_str(),
        policies_path,
        policy_id,
        "find",
        "validation_invalid_arguments",
        "authorization policy control uses unknown scope value",
    )?;
    let list = parse_scope_for_control(
        controls.list.as_str(),
        policies_path,
        policy_id,
        "list",
        "validation_invalid_arguments",
        "authorization policy list control uses unknown scope value",
    )?;
    let look = parse_scope_for_control(
        controls.look.as_str(),
        policies_path,
        policy_id,
        "look",
        "validation_invalid_arguments",
        "authorization policy control uses unknown scope value",
    )?;
    let send = parse_scope_for_control(
        controls.send.as_str(),
        policies_path,
        policy_id,
        "send",
        "validation_invalid_arguments",
        "authorization policy send control uses unknown scope value",
    )?;
    let raww = parse_scope_for_control(
        controls.raww.as_str(),
        policies_path,
        policy_id,
        "raww",
        "validation_invalid_policy_scope",
        "authorization policy raww control uses unknown scope value",
    )?;
    let choose = parse_scope_for_control(
        controls.choose.as_str(),
        policies_path,
        policy_id,
        "choose",
        "validation_invalid_policy_scope",
        "authorization policy choose control uses unknown scope value",
    )?;
    let updown = parse_scope_for_control(
        controls.updown.as_str(),
        policies_path,
        policy_id,
        "updown",
        "validation_invalid_policy_scope",
        "authorization policy updown control uses unknown scope value",
    )?;
    let do_controls = parse_action_scope_map(controls.do_controls, "do", policies_path, policy_id)?;
    let new_controls =
        parse_action_scope_map(controls.new_controls, "new", policies_path, policy_id)?;
    let change_controls =
        parse_action_scope_map(controls.change_controls, "change", policies_path, policy_id)?;
    Ok(PolicyControls {
        find,
        list,
        look,
        send,
        raww,
        choose,
        updown,
        do_controls,
        new_controls,
        change_controls,
    })
}

fn parse_action_scope_map(
    raw_map: HashMap<String, String>,
    namespace: &str,
    policies_path: &Path,
    policy_id: &str,
) -> Result<HashMap<String, PolicyScope>, RelayError> {
    let mut result = HashMap::with_capacity(raw_map.len());
    for (action_id, scope_value) in raw_map {
        let action_id = action_id.trim();
        if action_id.is_empty() {
            return Err(relay_error(
                "validation_invalid_arguments",
                "policy action id must be non-empty",
                Some(json!({
                    "path": policies_path.display().to_string(),
                    "policy_id": policy_id,
                    "namespace": namespace,
                })),
            ));
        }
        let scope = parse_scope_for_control(
            scope_value.as_str(),
            policies_path,
            policy_id,
            format!("{namespace}.{action_id}").as_str(),
            "validation_invalid_arguments",
            "authorization policy control uses unknown scope value",
        )?;
        result.insert(action_id.to_string(), scope);
    }
    Ok(result)
}

// The policies file is authoritative: every control accepts the full
// none/self/home/all ladder, and consuming authorization checks give each
// value its effect via rank order. Do not reintroduce per-control
// allowed-scope caps here.
fn parse_scope_for_control(
    raw: &str,
    policies_path: &Path,
    policy_id: &str,
    control: &str,
    error_code: &str,
    unknown_value_message: &str,
) -> Result<PolicyScope, RelayError> {
    let value = raw.trim();
    match value {
        "none" => Ok(PolicyScope::None),
        "self" => Ok(PolicyScope::SelfOnly),
        "home" => Ok(PolicyScope::Home),
        "all" => Ok(PolicyScope::All),
        _ => Err(relay_error(
            error_code,
            unknown_value_message,
            Some(json!({
                "path": policies_path.display().to_string(),
                "policy_id": policy_id,
                "control": control,
                "value": value,
                "expected": ["none", "self", "home", "all"],
            })),
        )),
    }
}

pub(super) fn map_tui_configuration_error(
    source: crate::configuration::ConfigurationError,
) -> RelayError {
    match source {
        crate::configuration::ConfigurationError::InvalidConfiguration { path, message } => {
            relay_error(
                "validation_invalid_arguments",
                "tui configuration is invalid",
                Some(json!({
                    "path": path.display().to_string(),
                    "cause": message,
                })),
            )
        }
        crate::configuration::ConfigurationError::Io { context, source } => relay_error(
            "validation_invalid_arguments",
            "failed to load tui configuration",
            Some(json!({
                "context": context,
                "cause": source.to_string(),
            })),
        ),
        other => relay_error(
            "validation_invalid_arguments",
            "failed to load tui configuration",
            Some(json!({
                "cause": other.to_string(),
            })),
        ),
    }
}
