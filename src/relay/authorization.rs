use std::{collections::HashMap, fs, path::Path};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    configuration::{
        BundleConfiguration, BundleMember, load_bundle_configuration, load_tui_configuration,
    },
    relay::{POLICIES_FILE, POLICIES_FORMAT_VERSION, RelayError, relay_error},
};

use super::identity::{PrincipalType, classify_principal_id, split_principal_id};
use super::map_config;

const RELAY_FILE: &str = "relay.toml";
const DEFAULT_PERMISSION_MAX_PENDING: usize = 256;
const MIN_PERMISSION_MAX_PENDING: usize = 1;
const MAX_PERMISSION_MAX_PENDING: usize = 4096;

#[derive(Clone, Debug)]
pub(super) struct AuthorizationContext {
    controls_by_session: HashMap<String, PolicyControls>,
    ui_sessions: HashMap<String, UiSessionAuthorization>,
    permission_max_pending: usize,
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
    grant: PolicyScope,
    updown: PolicyScope,
    do_controls: HashMap<String, PolicyScope>,
    new_controls: HashMap<String, PolicyScope>,
    change_controls: HashMap<String, PolicyScope>,
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
            grant: PolicyScope::None,
            updown: PolicyScope::None,
            do_controls: HashMap::new(),
            new_controls: HashMap::new(),
            change_controls: HashMap::new(),
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
    permission: Option<RawRelayPermissionSection>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawRelayPermissionSection {
    #[serde(default)]
    max_pending: Option<usize>,
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
    #[serde(default = "default_grant_policy_scope")]
    grant: String,
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
    "all:home".to_string()
}

fn default_grant_policy_scope() -> String {
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
fn load_policy_presets(
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

pub(super) fn load_authorization_context(
    configuration_root: &Path,
    bundle: &BundleConfiguration,
) -> Result<AuthorizationContext, RelayError> {
    let policies_path = configuration_root.join(POLICIES_FILE);
    let (presets, default_policy_id) = load_policy_presets(configuration_root)?;

    let permission_max_pending = load_permission_max_pending(configuration_root)?;

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
        permission_max_pending,
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

pub(super) fn permission_max_pending(authorization: &AuthorizationContext) -> usize {
    authorization.permission_max_pending
}

fn load_permission_max_pending(configuration_root: &Path) -> Result<usize, RelayError> {
    let path = configuration_root.join(RELAY_FILE);
    if !path.exists() {
        return Ok(DEFAULT_PERMISSION_MAX_PENDING);
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
        .and_then(|relay| relay.permission)
        .and_then(|permission| permission.max_pending)
        .unwrap_or(DEFAULT_PERMISSION_MAX_PENDING);
    if (MIN_PERMISSION_MAX_PENDING..=MAX_PERMISSION_MAX_PENDING).contains(&configured) {
        return Ok(configured);
    }
    Err(relay_error(
        "validation_invalid_arguments",
        "relay permission max-pending is out of supported range",
        Some(json!({
            "path": path.display().to_string(),
            "field": "relay.permission.max-pending",
            "value": configured,
            "minimum": MIN_PERMISSION_MAX_PENDING,
            "maximum": MAX_PERMISSION_MAX_PENDING,
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
    let grant = parse_scope_for_control(
        controls.grant.as_str(),
        policies_path,
        policy_id,
        "grant",
        &[PolicyScope::None, PolicyScope::AllHome],
        "validation_invalid_policy_scope",
        "authorization policy grant control uses unsupported scope value",
    )?;
    let updown = parse_scope_for_control(
        controls.updown.as_str(),
        policies_path,
        policy_id,
        "updown",
        &[PolicyScope::None, PolicyScope::AllHome],
        "validation_invalid_policy_scope",
        "authorization policy updown control uses unsupported scope value",
    )?;
    let do_controls = parse_action_scope_map(
        controls.do_controls,
        "do",
        policies_path,
        policy_id,
        &[
            PolicyScope::None,
            PolicyScope::SelfOnly,
            PolicyScope::AllHome,
            PolicyScope::AllAll,
        ],
    )?;
    let new_controls = parse_action_scope_map(
        controls.new_controls,
        "new",
        policies_path,
        policy_id,
        &[PolicyScope::None, PolicyScope::AllHome, PolicyScope::AllAll],
    )?;
    let change_controls = parse_action_scope_map(
        controls.change_controls,
        "change",
        policies_path,
        policy_id,
        &[PolicyScope::None, PolicyScope::AllHome, PolicyScope::AllAll],
    )?;
    Ok(PolicyControls {
        find,
        list,
        look,
        send,
        raww,
        grant,
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
    allowed_scopes: &[PolicyScope],
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
            allowed_scopes,
            "validation_invalid_arguments",
            "authorization policy control uses unsupported scope value",
        )?;
        result.insert(action_id.to_string(), scope);
    }
    Ok(result)
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

/// Relay-level operator action family for namespace-agnostic authorization.
#[derive(Clone, Copy, Debug)]
pub(super) enum RelayActionFamily {
    New,
    Change,
}

impl RelayActionFamily {
    fn namespace(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Change => "change",
        }
    }
}

/// Resolves the policy controls for a requester identified by full
/// `principal_id`, independent of any bundle context.
///
/// `@GLOBAL` operators resolve their preset from the TUI configuration; session
/// principals resolve from their bundle member entry (falling back to the
/// default policy). Application and relay principals have no operator policy
/// mapping and resolve to the conservative default (which grants no relay-wide
/// action), keeping the path fail-closed.
fn resolve_relay_principal_controls(
    configuration_root: &Path,
    requester_principal_id: &str,
) -> Result<PolicyControls, RelayError> {
    let (presets, default_policy_id) = load_policy_presets(configuration_root)?;
    let conservative_default = PolicyControls::conservative_default();
    let principal_type = classify_principal_id(requester_principal_id).ok_or_else(|| {
        relay_error(
            "validation_invalid_principal_id",
            "requester principal_id is not in <id>@<namespace> form",
            Some(json!({ "principal_id": requester_principal_id })),
        )
    })?;
    match principal_type {
        PrincipalType::User => {
            let Some(tui_configuration) =
                load_tui_configuration(configuration_root).map_err(map_tui_configuration_error)?
            else {
                return Ok(conservative_default);
            };
            let Some(session) = tui_configuration.session_by_id(requester_principal_id) else {
                return Ok(conservative_default);
            };
            let Some(policy_id) = normalize_policy_id(session.policy.as_str()) else {
                return Ok(conservative_default);
            };
            presets.get(policy_id).cloned().ok_or_else(|| {
                relay_error(
                    "validation_unknown_policy",
                    "global user policy references unknown policy id",
                    Some(json!({
                        "principal_id": requester_principal_id,
                        "policy_id": policy_id,
                    })),
                )
            })
        }
        PrincipalType::Session => {
            let Some((session_id, bundle_name)) = split_principal_id(requester_principal_id) else {
                return Ok(conservative_default);
            };
            let bundle =
                load_bundle_configuration(configuration_root, bundle_name).map_err(map_config)?;
            let Some(member) = bundle.members.iter().find(|member| member.id == session_id) else {
                return Ok(conservative_default);
            };
            let policies_path = configuration_root.join(POLICIES_FILE);
            resolve_session_policy_controls(
                member,
                &presets,
                default_policy_id.as_deref(),
                &conservative_default,
                policies_path.as_path(),
            )
            .cloned()
        }
        PrincipalType::Application | PrincipalType::Relay => Ok(conservative_default),
    }
}

/// Authorizes a relay-level operator action (`new.peer`, `change.psk`).
///
/// These operations mutate the relay-wide principal store and therefore require
/// a relay-wide grant: the requester's policy must grant the control at
/// `all:all`. A bundle/namespace-relative `all:home` grant is insufficient
/// because it confers no authority beyond the requester's own namespace.
pub(super) fn authorize_relay_action(
    configuration_root: &Path,
    requester_principal_id: &str,
    family: RelayActionFamily,
    action: &str,
) -> Result<(), RelayError> {
    let controls = resolve_relay_principal_controls(configuration_root, requester_principal_id)?;
    let scope_map = match family {
        RelayActionFamily::New => &controls.new_controls,
        RelayActionFamily::Change => &controls.change_controls,
    };
    let granted = scope_map
        .get(action)
        .is_some_and(|scope| scope.allows(PolicyScope::AllAll));
    if granted {
        return Ok(());
    }
    Err(authorization_forbidden(
        format!("{}.{action}", family.namespace()).as_str(),
        requester_principal_id,
        "",
        "relay-wide operator action requires an all:all grant for this control",
        None,
        None,
        None,
    ))
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

/// Authorizes a requester's `look` capability against a target session.
///
/// The requester's controls are always resolved in its own (the request's
/// dispatch) bundle context — a session is a member only of its home bundle.
/// `cross_bundle` reflects whether the target lives in a peer bundle; it
/// deliberately raises the bar rather than copying Send's permit-all stance:
///
/// - same-bundle inspection of another session requires `all:home`;
/// - inspecting a peer bundle's session requires `all:all`, mirroring how
///   relay-wide operator actions treat `all:home` as conferring no authority
///   beyond the requester's own namespace.
///
/// The self-inspection shortcut applies only to same-bundle looks; a peer-bundle
/// target is never the requester itself, so the cross-bundle path always
/// evaluates the scope.
pub(super) fn authorize_look(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    requester_session: &str,
    target_session: &str,
    cross_bundle: bool,
) -> Result<(), RelayError> {
    if !cross_bundle && requester_session == target_session {
        return Ok(());
    }
    let controls = controls_for_requester(authorization, bundle, requester_session)?;
    let (minimum_scope, reason) = if cross_bundle {
        (
            PolicyScope::AllAll,
            "look policy scope does not permit cross-bundle inspection (requires all:all)",
        )
    } else {
        (
            PolicyScope::AllHome,
            "look policy scope permits self-only inspection",
        )
    };
    authorize_scope(
        controls.look,
        minimum_scope,
        AuthorizationDecisionContext {
            capability: "look.inspect",
            requester_session,
            bundle_name: bundle.bundle_name.as_str(),
            reason,
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

pub(super) fn authorize_grant(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    requester_session: &str,
    permission_request_id: &str,
) -> Result<(), RelayError> {
    let controls = controls_for_requester(authorization, bundle, requester_session)?;
    authorize_scope(
        controls.grant,
        PolicyScope::AllHome,
        AuthorizationDecisionContext {
            capability: "grant",
            requester_session,
            bundle_name: bundle.bundle_name.as_str(),
            reason: "grant policy scope does not allow permission decisions",
            target_session: None,
            targets: None,
        },
    )
    .map_err(|mut error| {
        if let Some(object) = error.details.as_mut().and_then(Value::as_object_mut) {
            object.insert(
                "permission_request_id".to_string(),
                Value::String(permission_request_id.to_string()),
            );
        }
        error
    })
}

pub(super) fn authorize_updown(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    requester_session: &str,
) -> Result<(), RelayError> {
    let controls = controls_for_requester(authorization, bundle, requester_session)?;
    authorize_scope(
        controls.updown,
        PolicyScope::AllHome,
        AuthorizationDecisionContext {
            capability: "updown",
            requester_session,
            bundle_name: bundle.bundle_name.as_str(),
            reason: "updown policy scope does not allow bundle up/down transitions",
            target_session: None,
            targets: None,
        },
    )
}

pub(super) fn authorize_grant_for_list(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    requester_session: &str,
) -> Result<(), RelayError> {
    let controls = controls_for_requester(authorization, bundle, requester_session)?;
    authorize_scope(
        controls.grant,
        PolicyScope::AllHome,
        AuthorizationDecisionContext {
            capability: "grant",
            requester_session,
            bundle_name: bundle.bundle_name.as_str(),
            reason: "grant policy scope does not allow permission list",
            target_session: None,
            targets: None,
        },
    )
}

pub(super) fn grant_authorized_ui_sessions(
    authorization: &AuthorizationContext,
    _bundle: &BundleConfiguration,
) -> Vec<String> {
    authorization
        .ui_sessions
        .keys()
        .filter(|session_id| {
            authorization
                .controls_by_session
                .get(session_id.as_str())
                .is_some_and(|controls| controls.grant.allows(PolicyScope::AllHome))
        })
        .cloned()
        .collect()
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
