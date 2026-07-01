//! Capability/profile checks: the uniform `authorize_*` decisions over a
//! resolved route or a relay-wide operator action. All scope comparisons funnel
//! through [`authorize_scope`]; cross-bundle reach is decided data-driven by the
//! scope the policies file configures for the capability, never per operation
//! here.

use std::path::Path;

use serde_json::{Value, json};

use crate::{
    configuration::BundleConfiguration,
    relay::{RelayError, relay_error},
};

use super::super::canonical_session_id;
use super::super::identity::scope_permits;
use super::super::routing::{
    Addressing, Capability, OperationProfile, ResolvedRoute, ResolvedTarget, ScopeTier,
    required_tier,
};
use super::context::{AuthorizationContext, PolicyControls, PolicyScope};
use super::resolution::{controls_for_requester, resolve_relay_principal_controls};

/// How the dispatch spine authorizes a request's requester against a route.
///
/// The two modes are mutually exclusive by requester kind: a bundle/relay-wide
/// requester with a policy preset is authorized by [`RouteAuthorization::Policy`]
/// (tier check), while a peer relay (`<id>@RELAY`) forwarding a cross-relay
/// `Send`/`Raww` — which carries no bundle policy — is authorized by
/// [`RouteAuthorization::Ingress`] (per-target scope check).
pub(in crate::relay) enum RouteAuthorization<'a> {
    /// Normal path: resolve the requester's policy controls in its home bundle
    /// and check the route's required scope tier.
    Policy(&'a AuthorizationContext),
    /// Cross-relay ingress: each target must be covered by the peer principal's
    /// registered ingress `scope`; deny-by-default when the scope is absent.
    Ingress { scope: Option<&'a str> },
}

impl RouteAuthorization<'_> {
    /// Applies the authorization for this mode to `route`.
    pub(in crate::relay) fn authorize(
        &self,
        dispatch_namespace: &str,
        profile: OperationProfile,
        route: &ResolvedRoute,
    ) -> Result<(), RelayError> {
        match self {
            Self::Policy(context) => authorize_route(dispatch_namespace, context, profile, route),
            Self::Ingress { scope } => authorize_ingress(*scope, route),
        }
    }
}

struct AuthorizationDecisionContext<'a> {
    capability: &'a str,
    requester_session: &'a str,
    namespace: &'a str,
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
/// relay-wide reach: the requester's policy must hold the control at
/// `all`. A bundle/namespace-relative `home` scope is insufficient
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
        .is_some_and(|scope| scope.allows(PolicyScope::All));
    if granted {
        return Ok(());
    }
    Err(authorization_forbidden(
        format!("{}.{action}", family.namespace()).as_str(),
        requester_principal_id,
        "",
        "relay-wide operator action requires the all scope for this control",
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
/// requester is mapped to a uniform scope tier (self / home / all), and
/// the maximum required tier across the route is checked against the requester's
/// *configured* scope for the operation's capability.
///
/// No operation supplies a cross-bundle policy: the relay never decides per
/// operation whether crossing is allowed. Whether a capability reaches the
/// cross-bundle (`all`) tier is governed solely by the scope the policies file
/// configures for it — every control accepts the full `none`/`self`/`home`/
/// `all` ladder at parse time, and rank order gives each value its effect.
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
            namespace: dispatch_namespace,
            reason: tier_denial_reason(minimum),
            target_session,
            targets,
        },
    )
}

/// Deny-by-default ingress gate for a cross-relay (peer relay) requester.
///
/// A forwarded `Send`/`Raww` from a peer relay carries no bundle policy, so the
/// tier-based [`authorize_route`] does not apply. Instead, each session target
/// must be covered by the peer principal's registered `scope` — an exact
/// `session@bundle` identity or a bare `bundle` namespace — checked via
/// [`scope_permits`]. An absent scope covers nothing (fail-closed). A target
/// outside the scope is rejected with `authorization_forbidden` carrying an
/// ingress-denied reason. Target *existence* is validated earlier by the spine's
/// prepare stage, so an unknown target still sorts before this
/// (`validation_unknown_target` before `authorization_forbidden`).
/// Rejects a cross-relay (bang-path) target presented by an authenticated peer
/// relay ingress requester.
///
/// A peer relay may address only plain local targets on the receiving relay.
/// Forwarding its bang-path target onward would chain through this relay to this
/// relay's own outbound peers — under this relay's `<relay-id>@RELAY` identity —
/// escaping the ingress trust boundary and expanding the peer's effective reach.
/// The rejection is applied before any forwarding and independent of whether an
/// onward peer is configured, so a peer can never use the receiving relay as a
/// hop in this slice. Multi-hop cross-relay routing (relay chaining) is out of
/// scope for this slice and would need its own trust-propagation design.
pub(in crate::relay) fn reject_cross_relay_ingress(target: &ResolvedTarget) -> RelayError {
    let target_label = match (target.session_id.as_deref(), target.relay_id.as_deref()) {
        (Some(session_id), Some(relay_id)) => format!(
            "{}!{relay_id}",
            canonical_session_id(session_id, target.namespace.as_str())
        ),
        _ => target.namespace.clone(),
    };
    relay_error(
        "authorization_forbidden",
        "a peer relay ingress requester may not address a cross-relay target",
        Some(json!({
            "capability": "ingress",
            "reason": "cross-relay chaining is not permitted for a peer relay ingress requester",
            "target_session": target_label,
        })),
    )
}

fn authorize_ingress(scope: Option<&str>, route: &ResolvedRoute) -> Result<(), RelayError> {
    for target in &route.targets {
        let Some(session_id) = target.session_id.as_deref() else {
            continue;
        };
        let canonical = canonical_session_id(session_id, target.namespace.as_str());
        if !scope_permits(scope, canonical.as_str()) {
            return Err(relay_error(
                "authorization_forbidden",
                "request denied by cross-relay ingress policy",
                Some(json!({
                    "capability": "ingress",
                    "target_session": canonical,
                    "reason": "peer relay scope does not cover this target",
                    "scope": scope,
                })),
            ));
        }
    }
    Ok(())
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
        ScopeTier::Home => PolicyScope::Home,
        ScopeTier::All => PolicyScope::All,
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
                canonical_session_id(session_id.as_str(), target.namespace.as_str())
            })
        })
        .collect()
}

fn tier_denial_reason(minimum: PolicyScope) -> &'static str {
    match minimum {
        PolicyScope::All => "policy scope does not permit cross-bundle access (requires all)",
        _ => "policy scope does not permit this access",
    }
}

pub(in crate::relay) fn authorize_choose(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    requester_session: &str,
    choice_request_id: &str,
) -> Result<(), RelayError> {
    let controls = controls_for_requester(
        authorization,
        bundle.bundle_name.as_str(),
        requester_session,
    )?;
    authorize_scope(
        controls.choose,
        PolicyScope::Home,
        AuthorizationDecisionContext {
            capability: "choose",
            requester_session,
            namespace: bundle.bundle_name.as_str(),
            reason: "choose policy scope does not allow choice decisions",
            target_session: None,
            targets: None,
        },
    )
    .map_err(|mut error| {
        if let Some(object) = error.details.as_mut().and_then(Value::as_object_mut) {
            object.insert(
                "choice_request_id".to_string(),
                Value::String(choice_request_id.to_string()),
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
        PolicyScope::Home,
        AuthorizationDecisionContext {
            capability: "updown",
            requester_session,
            namespace: bundle.bundle_name.as_str(),
            reason: "updown policy scope does not allow bundle up/down transitions",
            target_session: None,
            targets: None,
        },
    )
}

pub(in crate::relay) fn authorize_choose_for_list(
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
        controls.choose,
        PolicyScope::Home,
        AuthorizationDecisionContext {
            capability: "choose",
            requester_session,
            namespace: bundle.bundle_name.as_str(),
            reason: "choose policy scope does not allow choices list",
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
        context.namespace,
        context.reason,
        context.target_session,
        context.targets,
        None,
    ))
}

fn authorization_forbidden(
    capability: &str,
    requester_session: &str,
    namespace: &str,
    reason: &str,
    target_session: Option<&str>,
    targets: Option<&[String]>,
    policy_rule_id: Option<&str>,
) -> RelayError {
    let mut details = json!({
        "capability": capability,
        "requester_session": requester_session,
        "namespace": namespace,
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
