use std::{collections::HashMap, fs, path::Path};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    configuration::{BundleConfiguration, BundleMember},
    relay::{POLICIES_FILE, POLICIES_FORMAT_VERSION, RelayError, relay_error},
};

#[derive(Clone, Debug)]
pub(super) struct AuthorizationContext {
    controls_by_session: HashMap<String, PolicyControls>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GeneralScope {
    None,
    SelfOnly,
    AllHome,
    AllAll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListScope {
    AllHome,
    AllAll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SendScope {
    AllHome,
    AllAll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuthorizationScope {
    None,
    SelfOnly,
    AllHome,
    AllAll,
}

impl From<GeneralScope> for AuthorizationScope {
    fn from(value: GeneralScope) -> Self {
        match value {
            GeneralScope::None => Self::None,
            GeneralScope::SelfOnly => Self::SelfOnly,
            GeneralScope::AllHome => Self::AllHome,
            GeneralScope::AllAll => Self::AllAll,
        }
    }
}

impl From<ListScope> for AuthorizationScope {
    fn from(value: ListScope) -> Self {
        match value {
            ListScope::AllHome => Self::AllHome,
            ListScope::AllAll => Self::AllAll,
        }
    }
}

impl From<SendScope> for AuthorizationScope {
    fn from(value: SendScope) -> Self {
        match value {
            SendScope::AllHome => Self::AllHome,
            SendScope::AllAll => Self::AllAll,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScopeRequirement {
    Any,
    NonSelf,
}

struct AuthorizationDecisionContext<'a> {
    capability: &'a str,
    requester_session: &'a str,
    bundle_name: &'a str,
    reason: &'a str,
    target_session: Option<&'a str>,
    targets: Option<&'a [String]>,
}

#[derive(Clone, Debug)]
struct PolicyControls {
    find: GeneralScope,
    list: ListScope,
    look: GeneralScope,
    send: SendScope,
    do_controls: HashMap<String, GeneralScope>,
}

impl PolicyControls {
    fn conservative_default() -> Self {
        Self {
            find: GeneralScope::SelfOnly,
            list: ListScope::AllHome,
            look: GeneralScope::SelfOnly,
            send: SendScope::AllHome,
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
    #[serde(default, rename = "do")]
    do_controls: HashMap<String, String>,
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
    Ok(AuthorizationContext {
        controls_by_session,
    })
}

fn parse_policy_controls(
    controls: RawPolicyControls,
    policies_path: &Path,
    policy_id: &str,
) -> Result<PolicyControls, RelayError> {
    let find = parse_general_scope(controls.find.as_str(), policies_path, policy_id, "find")?;
    let list = parse_list_scope(controls.list.as_str(), policies_path, policy_id)?;
    let look = parse_general_scope(controls.look.as_str(), policies_path, policy_id, "look")?;
    let send = parse_send_scope(controls.send.as_str(), policies_path, policy_id)?;
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
        let scope = parse_general_scope(
            scope_value.as_str(),
            policies_path,
            policy_id,
            format!("do.{action_id}").as_str(),
        )?;
        do_controls.insert(action_id.to_string(), scope);
    }
    Ok(PolicyControls {
        find,
        list,
        look,
        send,
        do_controls,
    })
}

fn parse_general_scope(
    raw: &str,
    policies_path: &Path,
    policy_id: &str,
    control: &str,
) -> Result<GeneralScope, RelayError> {
    match raw.trim() {
        "none" => Ok(GeneralScope::None),
        "self" => Ok(GeneralScope::SelfOnly),
        "all:home" => Ok(GeneralScope::AllHome),
        "all:all" => Ok(GeneralScope::AllAll),
        value => Err(relay_error(
            "validation_invalid_arguments",
            "authorization policy control uses unsupported scope value",
            Some(json!({
                "path": policies_path.display().to_string(),
                "policy_id": policy_id,
                "control": control,
                "value": value,
            })),
        )),
    }
}

fn parse_list_scope(
    raw: &str,
    policies_path: &Path,
    policy_id: &str,
) -> Result<ListScope, RelayError> {
    match raw.trim() {
        "all:home" => Ok(ListScope::AllHome),
        "all:all" => Ok(ListScope::AllAll),
        value => Err(relay_error(
            "validation_invalid_arguments",
            "authorization policy list control uses unsupported scope value",
            Some(json!({
                "path": policies_path.display().to_string(),
                "policy_id": policy_id,
                "control": "list",
                "value": value,
            })),
        )),
    }
}

fn parse_send_scope(
    raw: &str,
    policies_path: &Path,
    policy_id: &str,
) -> Result<SendScope, RelayError> {
    match raw.trim() {
        "all:home" => Ok(SendScope::AllHome),
        "all:all" => Ok(SendScope::AllAll),
        value => Err(relay_error(
            "validation_invalid_arguments",
            "authorization policy send control uses unsupported scope value",
            Some(json!({
                "path": policies_path.display().to_string(),
                "policy_id": policy_id,
                "control": "send",
                "value": value,
            })),
        )),
    }
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
        controls.list.into(),
        ScopeRequirement::Any,
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
        controls.send.into(),
        ScopeRequirement::Any,
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
        controls.look.into(),
        ScopeRequirement::NonSelf,
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

fn authorize_scope(
    scope: AuthorizationScope,
    requirement: ScopeRequirement,
    context: AuthorizationDecisionContext<'_>,
) -> Result<(), RelayError> {
    if scope_allows_requirement(scope, requirement) {
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

fn scope_allows_requirement(scope: AuthorizationScope, requirement: ScopeRequirement) -> bool {
    match requirement {
        ScopeRequirement::Any => !matches!(scope, AuthorizationScope::None),
        ScopeRequirement::NonSelf => {
            matches!(
                scope,
                AuthorizationScope::AllHome | AuthorizationScope::AllAll
            )
        }
    }
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
