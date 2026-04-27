use std::{collections::HashMap, fs, path::Path};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    configuration::{BundleConfiguration, BundleMember, load_tui_configuration},
    relay::{POLICIES_FILE, POLICIES_FORMAT_VERSION, RelayError, relay_error},
};

#[derive(Clone, Debug)]
pub(super) struct AuthorizationContext {
    controls_by_session: HashMap<String, PolicyControls>,
    ui_sessions: HashMap<String, UiSessionAuthorization>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PolicyScope {
    None,
    SelfOnly,
    AllHome,
    AllAll,
}

impl PolicyScope {
    fn rank(self) -> u8 {
        match self {
            Self::None => 0,
            Self::SelfOnly => 1,
            Self::AllHome => 2,
            Self::AllAll => 3,
        }
    }

    fn allows(self, minimum: Self) -> bool {
        self.rank() >= minimum.rank()
    }
}

struct AuthorizationDecisionContext<'a> {
    capability: &'a str,
    requester_session: &'a str,
    bundle_name: &'a str,
    reason: &'a str,
    target_session: Option<&'a str>,
    targets: Option<&'a [String]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PolicyControls {
    find: PolicyScope,
    list: PolicyScope,
    look: PolicyScope,
    send: PolicyScope,
    raww: PolicyScope,
    do_controls: HashMap<String, PolicyScope>,
}

#[derive(Clone, Debug)]
struct UiSessionAuthorization {
    display_name: Option<String>,
}

impl PolicyControls {
    fn conservative_default() -> Self {
        Self {
            find: PolicyScope::SelfOnly,
            list: PolicyScope::AllHome,
            look: PolicyScope::AllHome,
            send: PolicyScope::AllHome,
            raww: PolicyScope::AllHome,
            do_controls: HashMap::new(),
        }
    }
}

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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawPolicyControls {
    find: String,
    list: String,
    look: String,
    send: String,
    #[serde(default = "default_raww_policy_scope")]
    raww: String,
    #[serde(default, rename = "do")]
    do_controls: HashMap<String, String>,
}

fn default_raww_policy_scope() -> String {
    "all:home".to_string()
}

pub(super) fn load_authorization_context(
    configuration_root: &Path,
    bundle: &BundleConfiguration,
) -> Result<AuthorizationContext, RelayError> {
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

    let conservative_default = PolicyControls::conservative_default();
    let mut controls_by_session = HashMap::with_capacity(bundle.members.len());
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
    let mut ui_sessions = HashMap::<String, UiSessionAuthorization>::new();
    if let Some(tui_configuration) =
        load_tui_configuration(configuration_root).map_err(map_tui_configuration_error)?
    {
        for session in tui_configuration.sessions {
            let session_id = session.id.clone();
            let policy_id = normalize_policy_id(session.policy_id.as_str()).ok_or_else(|| {
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
    })
}

pub(super) fn has_ui_session(authorization: &AuthorizationContext, session_id: &str) -> bool {
    authorization.ui_sessions.contains_key(session_id)
}

pub(super) fn ui_session_display_name<'a>(
    authorization: &'a AuthorizationContext,
    session_id: &str,
) -> Option<&'a str> {
    authorization
        .ui_sessions
        .get(session_id)
        .and_then(|session| session.display_name.as_deref())
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
        &[
            PolicyScope::None,
            PolicyScope::SelfOnly,
            PolicyScope::AllHome,
            PolicyScope::AllAll,
        ],
        "validation_invalid_arguments",
        "authorization policy control uses unsupported scope value",
    )?;
    let list = parse_scope_for_control(
        controls.list.as_str(),
        policies_path,
        policy_id,
        "list",
        &[PolicyScope::AllHome, PolicyScope::AllAll],
        "validation_invalid_arguments",
        "authorization policy list control uses unsupported scope value",
    )?;
    let look = parse_scope_for_control(
        controls.look.as_str(),
        policies_path,
        policy_id,
        "look",
        &[
            PolicyScope::None,
            PolicyScope::SelfOnly,
            PolicyScope::AllHome,
            PolicyScope::AllAll,
        ],
        "validation_invalid_arguments",
        "authorization policy control uses unsupported scope value",
    )?;
    let send = parse_scope_for_control(
        controls.send.as_str(),
        policies_path,
        policy_id,
        "send",
        &[PolicyScope::AllHome, PolicyScope::AllAll],
        "validation_invalid_arguments",
        "authorization policy send control uses unsupported scope value",
    )?;
    let raww = parse_scope_for_control(
        controls.raww.as_str(),
        policies_path,
        policy_id,
        "raww",
        &[
            PolicyScope::None,
            PolicyScope::SelfOnly,
            PolicyScope::AllHome,
        ],
        "validation_invalid_policy_scope",
        "authorization policy raww control uses unsupported scope value",
    )?;
    let mut do_controls = HashMap::with_capacity(controls.do_controls.len());
    for (action_id, scope_value) in controls.do_controls {
        let action_id = action_id.trim();
        if action_id.is_empty() {
            return Err(relay_error(
                "validation_invalid_arguments",
                "do control action id must be non-empty",
                Some(json!({
                    "path": policies_path.display().to_string(),
                    "policy_id": policy_id,
                })),
            ));
        }
        let scope = parse_scope_for_control(
            scope_value.as_str(),
            policies_path,
            policy_id,
            format!("do.{action_id}").as_str(),
            &[
                PolicyScope::None,
                PolicyScope::SelfOnly,
                PolicyScope::AllHome,
                PolicyScope::AllAll,
            ],
            "validation_invalid_arguments",
            "authorization policy control uses unsupported scope value",
        )?;
        do_controls.insert(action_id.to_string(), scope);
    }
    Ok(PolicyControls {
        find,
        list,
        look,
        send,
        raww,
        do_controls,
    })
}

fn parse_scope_for_control(
    raw: &str,
    policies_path: &Path,
    policy_id: &str,
    control: &str,
    allowed: &[PolicyScope],
    error_code: &str,
    unsupported_message: &str,
) -> Result<PolicyScope, RelayError> {
    let value = raw.trim();
    let parsed = match value {
        "none" => PolicyScope::None,
        "self" => PolicyScope::SelfOnly,
        "all:home" => PolicyScope::AllHome,
        "all:all" => PolicyScope::AllAll,
        _ => {
            return Err(relay_error(
                error_code,
                unsupported_message,
                Some(json!({
                    "path": policies_path.display().to_string(),
                    "policy_id": policy_id,
                    "control": control,
                    "value": value,
                })),
            ));
        }
    };
    if allowed.contains(&parsed) {
        return Ok(parsed);
    }
    Err(relay_error(
        error_code,
        unsupported_message,
        Some(json!({
            "path": policies_path.display().to_string(),
            "policy_id": policy_id,
            "control": control,
            "value": value,
        })),
    ))
}

fn resolve_session_policy_controls<'a>(
    member: &BundleMember,
    presets: &'a HashMap<String, PolicyControls>,
    default_policy_id: Option<&str>,
    conservative_default: &'a PolicyControls,
    policies_path: &Path,
) -> Result<&'a PolicyControls, RelayError> {
    if let Some(policy_id) = member.policy_id.as_deref().and_then(normalize_policy_id) {
        return presets.get(policy_id).ok_or_else(|| {
            relay_error(
                "validation_invalid_arguments",
                "session policy references unknown policy id",
                Some(json!({
                    "path": policies_path.display().to_string(),
                    "session_id": member.id,
                    "policy_id": policy_id,
                })),
            )
        });
    }
    if let Some(default_policy_id) = default_policy_id {
        return presets.get(default_policy_id).ok_or_else(|| {
            relay_error(
                "validation_invalid_arguments",
                "authorization default policy references unknown policy id",
                Some(json!({
                    "path": policies_path.display().to_string(),
                    "policy_id": default_policy_id,
                })),
            )
        });
    }
    Ok(conservative_default)
}

pub(super) fn authorize_list(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    requester_session: &str,
) -> Result<(), RelayError> {
    let controls = controls_for_requester(authorization, bundle, requester_session)?;
    authorize_scope(
        controls.list,
        PolicyScope::SelfOnly,
        AuthorizationDecisionContext {
            capability: "list.read",
            requester_session,
            bundle_name: bundle.bundle_name.as_str(),
            reason: "list policy scope does not allow recipient visibility",
            target_session: None,
            targets: None,
        },
    )
}

pub(super) fn authorize_send(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    requester_session: &str,
    target_sessions: &[String],
) -> Result<(), RelayError> {
    let controls = controls_for_requester(authorization, bundle, requester_session)?;
    // MVP target resolution is same-bundle only; cross-bundle target selection
    // is not part of the current runtime contract.
    authorize_scope(
        controls.send,
        PolicyScope::SelfOnly,
        AuthorizationDecisionContext {
            capability: "send.deliver",
            requester_session,
            bundle_name: bundle.bundle_name.as_str(),
            reason: "send policy scope does not allow delivery",
            target_session: None,
            targets: Some(target_sessions),
        },
    )
}

pub(super) fn authorize_look(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    requester_session: &str,
    target_session: &str,
) -> Result<(), RelayError> {
    if requester_session == target_session {
        return Ok(());
    }
    let controls = controls_for_requester(authorization, bundle, requester_session)?;
    authorize_scope(
        controls.look,
        PolicyScope::AllHome,
        AuthorizationDecisionContext {
            capability: "look.inspect",
            requester_session,
            bundle_name: bundle.bundle_name.as_str(),
            reason: "look policy scope permits self-only inspection",
            target_session: Some(target_session),
            targets: None,
        },
    )
}

pub(super) fn authorize_raww(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    requester_session: &str,
    target_session: &str,
) -> Result<(), RelayError> {
    let controls = controls_for_requester(authorization, bundle, requester_session)?;
    let minimum_scope = if requester_session == target_session {
        PolicyScope::SelfOnly
    } else {
        PolicyScope::AllHome
    };
    authorize_scope(
        controls.raww,
        minimum_scope,
        AuthorizationDecisionContext {
            capability: "raww.write",
            requester_session,
            bundle_name: bundle.bundle_name.as_str(),
            reason: "raww policy scope does not allow target write",
            target_session: Some(target_session),
            targets: None,
        },
    )
}

fn authorize_scope(
    scope: PolicyScope,
    minimum_scope: PolicyScope,
    context: AuthorizationDecisionContext<'_>,
) -> Result<(), RelayError> {
    if scope.allows(minimum_scope) {
        return Ok(());
    }
    Err(authorization_forbidden(
        context.capability,
        context.requester_session,
        context.bundle_name,
        context.reason,
        context.target_session,
        context.targets,
        None,
    ))
}

fn controls_for_requester<'a>(
    authorization: &'a AuthorizationContext,
    bundle: &BundleConfiguration,
    requester_session: &str,
) -> Result<&'a PolicyControls, RelayError> {
    let controls = authorization
        .controls_by_session
        .get(requester_session)
        .ok_or_else(|| {
            relay_error(
                "validation_unknown_sender",
                "requester_session has no resolved policy controls",
                Some(json!({
                    "requester_session": requester_session,
                    "bundle_name": bundle.bundle_name,
                })),
            )
        })?;
    let _ = controls.find;
    let _ = controls.do_controls.len();
    Ok(controls)
}

fn authorization_forbidden(
    capability: &str,
    requester_session: &str,
    bundle_name: &str,
    reason: &str,
    target_session: Option<&str>,
    targets: Option<&[String]>,
    policy_rule_id: Option<&str>,
) -> RelayError {
    let mut details = json!({
        "capability": capability,
        "requester_session": requester_session,
        "bundle_name": bundle_name,
        "reason": reason,
    });
    if let Some(value) = target_session
        && let Some(object) = details.as_object_mut()
    {
        object.insert(
            "target_session".to_string(),
            Value::String(value.to_string()),
        );
    }
    if let Some(values) = targets
        && !values.is_empty()
        && let Some(object) = details.as_object_mut()
    {
        object.insert(
            "targets".to_string(),
            Value::Array(values.iter().cloned().map(Value::String).collect()),
        );
    }
    if let Some(value) = policy_rule_id
        && let Some(object) = details.as_object_mut()
    {
        object.insert(
            "policy_rule_id".to_string(),
            Value::String(value.to_string()),
        );
    }
    relay_error(
        "authorization_forbidden",
        "request denied by authorization policy",
        Some(details),
    )
}

fn normalize_policy_id(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(value)
}

fn map_tui_configuration_error(source: crate::configuration::ConfigurationError) -> RelayError {
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
