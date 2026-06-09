//! Capability/profile checks: the uniform `authorize_*` decisions over a
//! resolved route or a relay-wide operator action. All scope comparisons funnel
//! through [`authorize_scope`]; cross-bundle reach is decided data-driven by the
//! policy schema's per-capability allowed-scope set, never per operation here.

use std::path::Path;

use serde_json::{Value, json};

use crate::{
    configuration::BundleConfiguration,
    relay::{RelayError, relay_error},
};

use super::super::canonical_session_id;
use super::super::routing::{
    Addressing, Capability, OperationProfile, ResolvedRoute, ScopeTier, required_tier,
};
use super::context::{AuthorizationContext, PolicyControls, PolicyScope};
use super::resolution::{controls_for_requester, resolve_relay_principal_controls};

struct AuthorizationDecisionContext<'a> {
    capability: &'a str,
    requester_session: &'a str,
    bundle_name: &'a str,
    reason: &'a str,
    target_session: Option<&'a str>,
    targets: Option<&'a [String]>,
}

/// Relay-level operator action family for namespace-agnostic authorization.
#[derive(Clone, Copy, Debug)]
pub(in crate::relay) enum RelayActionFamily {
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

/// Authorizes a relay-level operator action (`new.peer`, `change.psk`).
///
/// These operations mutate the relay-wide principal store and therefore require
/// a relay-wide grant: the requester's policy must grant the control at
/// `all:all`. A bundle/namespace-relative `all:home` grant is insufficient
/// because it confers no authority beyond the requester's own namespace.
pub(in crate::relay) fn authorize_relay_action(
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

/// Uniform, fully data-driven authorization for the target operations
/// (`look`, `send`, `list`, `raww`).
///
/// The requester's controls are always resolved in the dispatch (home) bundle —
/// a session is a member only of its home bundle, and a peer bundle never
/// supplies the requester's policy. Each resolved target's relationship to the
/// requester is mapped to a uniform scope tier (self / all:home / all:all), and
/// the maximum required tier across the route is checked against the requester's
/// *configured* scope for the operation's capability.
///
/// No operation supplies a cross-bundle policy: the relay never decides per
/// operation whether crossing is allowed. Whether a capability can ever be
/// configured to the cross-bundle (`all:all`) tier is governed solely by the
/// policy schema's per-capability allowed-scope set (`parse_policy_controls`) —
/// `grant` and `updown`, for instance, are capped at `all:home` there, so they
/// can never satisfy the cross-bundle threshold without a schema change, no code
/// override needed.
pub(in crate::relay) fn authorize_route(
    dispatch_namespace: &str,
    authorization: &AuthorizationContext,
    profile: OperationProfile,
    route: &ResolvedRoute,
) -> Result<(), RelayError> {
    let controls = controls_for_requester(
        authorization,
        dispatch_namespace,
        route.requester_session.as_str(),
    )?;
    let scope = scope_for_capability(controls, profile.capability);
    let minimum = minimum_scope_for_tier(required_tier(route));
    let target_identifiers = route_target_identifiers(route);
    let (target_session, targets) = match profile.addressing {
        Addressing::SingleTarget => (target_identifiers.first().map(String::as_str), None),
        Addressing::MultiTarget => (None, Some(target_identifiers.as_slice())),
        Addressing::BundleEnumerate => (None, None),
    };
    authorize_scope(
        scope,
        minimum,
        AuthorizationDecisionContext {
            capability: profile.capability.label(),
            requester_session: route.requester_session.as_str(),
            bundle_name: dispatch_namespace,
            reason: tier_denial_reason(minimum),
            target_session,
            targets,
        },
    )
}

fn scope_for_capability(controls: &PolicyControls, capability: Capability) -> PolicyScope {
    match capability {
        Capability::Look => controls.look,
        Capability::Send => controls.send,
        Capability::List => controls.list,
        Capability::Raww => controls.raww,
    }
}

fn minimum_scope_for_tier(tier: ScopeTier) -> PolicyScope {
    match tier {
        ScopeTier::Own => PolicyScope::SelfOnly,
        ScopeTier::Home => PolicyScope::AllHome,
        ScopeTier::All => PolicyScope::AllAll,
    }
}

/// Canonical `<session>@<bundle>` identifiers for a route's session targets,
/// preserving discovery order. Bundle-level entries (a `List` enumeration) carry
/// no session and contribute nothing. Used only for error diagnostics.
fn route_target_identifiers(route: &ResolvedRoute) -> Vec<String> {
    route
        .targets
        .iter()
        .filter_map(|target| {
            target.session_id.as_ref().map(|session_id| {
                canonical_session_id(session_id.as_str(), target.bundle_name.as_str())
            })
        })
        .collect()
}

fn tier_denial_reason(minimum: PolicyScope) -> &'static str {
    match minimum {
        PolicyScope::AllAll => {
            "policy scope does not permit cross-bundle access (requires all:all)"
        }
        _ => "policy scope does not permit this access",
    }
}

pub(in crate::relay) fn authorize_grant(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    requester_session: &str,
    permission_request_id: &str,
) -> Result<(), RelayError> {
    let controls = controls_for_requester(
        authorization,
        bundle.bundle_name.as_str(),
        requester_session,
    )?;
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

pub(in crate::relay) fn authorize_updown(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    requester_session: &str,
) -> Result<(), RelayError> {
    let controls = controls_for_requester(
        authorization,
        bundle.bundle_name.as_str(),
        requester_session,
    )?;
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

pub(in crate::relay) fn authorize_grant_for_list(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    requester_session: &str,
) -> Result<(), RelayError> {
    let controls = controls_for_requester(
        authorization,
        bundle.bundle_name.as_str(),
        requester_session,
    )?;
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
