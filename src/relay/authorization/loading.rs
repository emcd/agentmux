//! Policy loading: parse `policies.toml` / `relay.toml` into validated presets
//! and build the [`AuthorizationContext`] a request is authorized against.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

use serde::Deserialize;
use serde_json::json;

use crate::{
    configuration::{
        BundleConfiguration, load_tui_configuration, policies_configuration_path,
        relay_configuration_path,
    },
    relay::{POLICIES_FORMAT_VERSION, RelayError, relay_error},
};

use super::context::{AuthorizationContext, PolicyControls, PolicyScope, UiSessionAuthorization};
use super::resolution::{normalize_policy_id, resolve_session_policy_controls};

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
    #[serde(default)]
    peers: Vec<RawPeerEntry>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawRelayChoicesSection {
    #[serde(default)]
    pending_max: Option<usize>,
}

/// Raw `[[peers]]` entry: an active outbound peer relay endpoint.
///
/// `alias` is this relay's *local* name for the peer — the bang-path `!<alias>`
/// routing selector and the peer credential filename stem, internal to us and
/// never seen by the peer. `connect-as` is the identity the peer issued us via its
/// `new peer <connect-as>@RELAY` — presented in Hello when we dial it, since each
/// peer determines the identity it expects from us (two peers can issue different
/// or even colliding identities to this relay). `address` is the peer's listening
/// endpoint — in this slice an absolute Unix domain socket path (TCP is future).
/// The table is outbound-only and carries no `scope`: inbound cross-relay
/// authorization is the scope this relay sets via `new peer <id>@RELAY --scope`
/// and reads through the ingress filter, so `deny_unknown_fields` rejects a stray
/// `scope` key here.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawPeerEntry {
    alias: String,
    address: String,
    connect_as: String,
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
    let policies_path = policies_configuration_path(configuration_root);
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
    let policies_path = policies_configuration_path(configuration_root);
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
    pub peers: Vec<PeerConfiguration>,
}

/// A validated `[[peers]]` entry naming an outbound peer relay.
///
/// `alias` is this relay's local name for the peer: the bang-path `!<alias>`
/// routing selector and the `<peer_alias>` credential filename stem. `connect_as`
/// is the identity the peer issued us, presented as `<connect_as>@RELAY` in Hello
/// when we dial it. `address` is an absolute Unix domain socket path. The
/// presented identity is per-peer because the *receiver* determines it (via its
/// `new peer`), so no single relay-wide identity exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerConfiguration {
    pub alias: String,
    pub address: String,
    pub connect_as: String,
}

/// File-derived relay settings before override resolution. `watch_bundles` and
/// `require_session_credentials` stay `Option` so the precedence ladder can tell
/// "absent in file" apart from an explicit value; `choices_pending_max` and
/// `peers` have no override layer and are resolved (and validated) here.
struct RelayFileConfiguration {
    watch_bundles: Option<bool>,
    require_session_credentials: Option<bool>,
    choices_pending_max: usize,
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
    let path = relay_configuration_path(configuration_root);
    if !path.exists() {
        return Ok(RelayFileConfiguration {
            watch_bundles: None,
            require_session_credentials: None,
            choices_pending_max: DEFAULT_CHOICES_PENDING_MAX,
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
    // The alias is this relay's local name for the peer: it is the bang-path
    // selector (`!<alias>`) and the credential filename stem
    // (`<state-root>/peers/<alias>.psk`), so it MUST be unique. Two entries
    // sharing an alias would silently collapse to one routable endpoint (the
    // connection manager keys on alias), with the surviving one decided by
    // insertion order rather than operator intent — so reject the collision
    // fail-fast at load. Duplicate `connect-as` stays allowed: the presented
    // identity is receiver-issued and two peers may legitimately issue this
    // relay the same one.
    let mut seen_aliases: HashSet<String> = HashSet::with_capacity(peers.capacity());
    for (index, peer) in parsed.peers.into_iter().enumerate() {
        let alias =
            validate_peer_id_token(peer.alias.as_str(), "peers.alias", index, path.as_path())?;
        if !seen_aliases.insert(alias.clone()) {
            return Err(relay_error(
                "validation_invalid_arguments",
                "relay peer alias must be unique across all [[peers]] entries",
                Some(json!({
                    "path": path.display().to_string(),
                    "field": "peers.alias",
                    "peer_index": index,
                    "value": alias,
                })),
            ));
        }
        let connect_as = validate_peer_id_token(
            peer.connect_as.as_str(),
            "peers.connect-as",
            index,
            path.as_path(),
        )?;
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
            alias,
            address: address.to_string(),
            connect_as,
        });
    }
    Ok(RelayFileConfiguration {
        watch_bundles: parsed.watch_bundles,
        require_session_credentials: parsed.require_session_credentials,
        choices_pending_max,
        peers,
    })
}

/// Validates a peer id token — a bang-path `!<alias>` selector or a `connect-as`
/// identity local part. The grammar is deliberately strict: the `alias` becomes a
/// credential filename stem (`<alias>.psk`) and the bang-path selector, and the
/// `connect-as` is qualified to `<connect-as>@RELAY` when presented, so both must
/// be non-empty after trimming and free of the `@` namespace qualifier, the `!`
/// bang-path separator, and any path separator. `field` names the offending key
/// for the error. Returns the trimmed token.
fn validate_peer_id_token(
    raw: &str,
    field: &str,
    peer_index: usize,
    path: &Path,
) -> Result<String, RelayError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(relay_error(
            "validation_invalid_arguments",
            "relay peer id token must be non-empty",
            Some(json!({
                "path": path.display().to_string(),
                "field": field,
                "peer_index": peer_index,
            })),
        ));
    }
    if value.contains('@') || value.contains('!') || value.contains('/') {
        return Err(relay_error(
            "validation_invalid_arguments",
            "relay peer id token must not contain '@', '!', or a path separator",
            Some(json!({
                "path": path.display().to_string(),
                "field": field,
                "peer_index": peer_index,
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
