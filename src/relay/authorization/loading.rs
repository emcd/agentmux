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

const RELAY_FILE: &str = "relay.toml";
const DEFAULT_CHOICES_PENDING_MAX: usize = 256;
const MIN_CHOICES_PENDING_MAX: usize = 1;
const MAX_CHOICES_PENDING_MAX: usize = 4096;

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

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawRelayFile {
    #[serde(default)]
    relay: Option<RawRelaySection>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawRelaySection {
    #[serde(default)]
    choices: Option<RawRelayChoicesSection>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawRelayChoicesSection {
    #[serde(default)]
    pending_max: Option<usize>,
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

    let choices_pending_max = load_choices_pending_max(configuration_root)?;

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

fn load_choices_pending_max(configuration_root: &Path) -> Result<usize, RelayError> {
    let path = configuration_root.join(RELAY_FILE);
    if !path.exists() {
        return Ok(DEFAULT_CHOICES_PENDING_MAX);
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
    let configured = parsed
        .relay
        .and_then(|relay| relay.choices)
        .and_then(|choices| choices.pending_max)
        .unwrap_or(DEFAULT_CHOICES_PENDING_MAX);
    if (MIN_CHOICES_PENDING_MAX..=MAX_CHOICES_PENDING_MAX).contains(&configured) {
        return Ok(configured);
    }
    Err(relay_error(
        "validation_invalid_arguments",
        "relay choices pending-max is out of supported range",
        Some(json!({
            "path": path.display().to_string(),
            "field": "relay.choices.pending-max",
            "value": configured,
            "minimum": MIN_CHOICES_PENDING_MAX,
            "maximum": MAX_CHOICES_PENDING_MAX,
        })),
    ))
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
