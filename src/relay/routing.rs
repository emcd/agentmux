//! Operation-agnostic routing and authorization dispatch layer.
//!
//! This module sits between the connection layer's bundle resolution and the
//! operation handlers. It encodes the one decision the handlers used to disagree
//! on (see `handlers.rs` history): *which authorization context the requester is
//! resolved in*. The invariant is uniform:
//!
//! > The requester is always authorized in its home/dispatch bundle. A peer
//! > bundle supplies only target existence and runtime/transport context, never
//! > the requester's policy controls.
//!
//! Authorization is fully data-driven. An operation contributes only an
//! [`OperationProfile`] naming which policy control it reads; the layer maps the
//! requester-to-target relationship to a uniform scope tier
//! (self / all:home / all:all) and checks it against the requester's *configured*
//! scope (the authorization stage in `authorization.rs`). No operation carries a
//! hardcoded cross-bundle policy: whether a capability can ever reach the
//! cross-bundle (`all:all`) tier is governed entirely by the policy schema's
//! per-capability allowed-scope set.

use serde_json::json;

use super::identity::{PrincipalType, classify_principal_id, split_principal_id};
use super::{RelayError, relay_error};

/// Which `policies.toml` control an operation's authorization reads.
#[derive(Clone, Copy, Debug)]
pub(super) enum Capability {
    Look,
    Send,
    List,
    Raww,
}

impl Capability {
    /// The diagnostic capability label surfaced on an `authorization_forbidden`
    /// error for this control.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Look => "look.inspect",
            Self::Send => "send.deliver",
            Self::List => "list.read",
            Self::Raww => "raww.write",
        }
    }
}

/// How an operation addresses the work it authorizes. Recorded on the profile so
/// the authorization stage can shape its diagnostics (single target, fan-out
/// target set, or a whole-bundle enumeration with no individual session).
#[derive(Clone, Copy, Debug)]
pub(super) enum Addressing {
    /// One resolved session target (`Look`).
    SingleTarget,
    /// A fan-out set of session / relay-wide targets across one or more bundles
    /// (`Send`).
    MultiTarget,
    /// A whole bundle, with no individual session target (`List`).
    BundleEnumerate,
}

/// The per-operation input to the otherwise uniform authorization stage.
#[derive(Clone, Copy, Debug)]
pub(super) struct OperationProfile {
    pub capability: Capability,
    pub addressing: Addressing,
}

/// One resolved unit of work the authorization stage scores.
///
/// A session target carries its hosting bundle and bundle-local id. A relay-wide
/// (`@GLOBAL`) target rides the dispatch bundle and is delivered via the registry
/// rather than by crossing into a peer bundle, so it never raises the bar to the
/// cross-bundle tier. A bundle-level entry (a `List` enumeration) names a bundle
/// with no session id.
#[derive(Clone, Debug)]
pub(super) struct ResolvedTarget {
    pub bundle_name: String,
    pub session_id: Option<String>,
    pub relay_wide: bool,
}

/// The resolved, about-to-be-authorized shape of a request: the dispatch (home)
/// namespace the requester is authorized in (a bundle name, or `GLOBAL`), the
/// requester's bundle-local id, and the resolved targets.
#[derive(Clone, Debug)]
pub(super) struct ResolvedRoute {
    pub dispatch_namespace: String,
    pub requester_session: String,
    pub targets: Vec<ResolvedTarget>,
}

/// The uniform scope tier a target requires, independent of which operation or
/// capability is being authorized. Mapped to a concrete policy scope by the
/// authorization stage.
///
/// The variants mirror the scope ladder the authorization stage maps them onto:
/// `Own` → `self`, `Home` → `all:home`, `All` → `all:all`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScopeTier {
    /// The requester acting on itself.
    Own,
    /// A different principal within the requester's own (dispatch) bundle.
    Home,
    /// A principal in a peer bundle.
    All,
}

impl ScopeTier {
    fn rank(self) -> u8 {
        match self {
            Self::Own => 0,
            Self::Home => 1,
            Self::All => 2,
        }
    }
}

impl ResolvedTarget {
    /// Whether this target is reachable at the requester's *home* tier — without
    /// crossing a namespace boundary.
    ///
    /// True for a same-bundle target and, as a **routing invariant (not a policy
    /// control)**, for any relay-wide (`@GLOBAL`) target. Relay-wide targets are
    /// delivered through the session *registry* keyed by `principal_id`, not by
    /// crossing into a peer bundle, so reaching one is not a cross-namespace act
    /// and never raises the bar to `all:all`. This is what lets a bundle-bound
    /// agent message an `@GLOBAL` operator (and one relay-wide principal message
    /// another) under `all:home`.
    ///
    /// It is asymmetric with a relay-wide *requester* reaching *into* a bundle:
    /// there the bundle is not the requester's home namespace (see
    /// [`requester_home_namespace`]), so that direction does require `all:all`.
    /// The asymmetry is intentional; the integration tests exercise both
    /// directions.
    fn reachable_at_home_tier(&self, dispatch_namespace: &str) -> bool {
        self.relay_wide || self.bundle_name == dispatch_namespace
    }

    /// Classifies this target's relationship to the requester within the
    /// dispatch namespace into a uniform scope tier.
    fn tier(&self, dispatch_namespace: &str, requester_session: &str) -> ScopeTier {
        if !self.reachable_at_home_tier(dispatch_namespace) {
            return ScopeTier::All;
        }
        // A relay-wide target is never `Own`: self-action via the relay-wide
        // registry path is not modeled, so it floors at `Home`.
        match self.session_id.as_deref() {
            Some(session_id) if session_id == requester_session && !self.relay_wide => {
                ScopeTier::Own
            }
            _ => ScopeTier::Home,
        }
    }
}

/// The requester's home (native) namespace, used as the dispatch namespace for
/// relationship classification.
///
/// A principal's home is its native namespace, not whichever bundle a request
/// happens to route through: a session's home is its bundle, while a relay-wide
/// principal's home is its reserved namespace (`GLOBAL` / `EXTERNAL` / `RELAY`).
/// So a relay-wide operator reaching into a bundle is cross-namespace (it needs
/// `all:all`), and `all:home` confers authority only within its own namespace.
/// A bare (already dispatch-normalized) session id carries no suffix and
/// resolves to the supplied dispatch bundle.
pub(super) fn requester_home_namespace<'a>(
    requester_session: &'a str,
    dispatch_namespace: &'a str,
) -> &'a str {
    match split_principal_id(requester_session) {
        Some((_, namespace)) => namespace,
        None => dispatch_namespace,
    }
}

/// The maximum scope tier required across all of a route's targets.
///
/// A route with no targets (a bundle-level operation that resolved to its own
/// dispatch bundle) requires the `self` floor, so an empty enumeration never
/// over-demands.
pub(super) fn required_tier(route: &ResolvedRoute) -> ScopeTier {
    route
        .targets
        .iter()
        .map(|target| target.tier(&route.dispatch_namespace, &route.requester_session))
        .max_by_key(|tier| tier.rank())
        .unwrap_or(ScopeTier::Own)
}

/// Classifies one fully-qualified target by its `@<namespace>` suffix alone — no
/// bundle configuration or catalog access. An `@<bundle>` target is a session in
/// that bundle; an `@GLOBAL` target is relay-wide (keyed by its full principal
/// id, riding `dispatch_namespace` for tier classification); a reserved
/// namespace (`@EXTERNAL`/`@RELAY`) is rejected with
/// `validation_unsupported_namespace`; a bare (unqualified) target is rejected
/// with `validation_unqualified_target`. Target *existence* is not checked here
/// — that is the handler body's delivery concern, validated after this stage.
fn resolve_target(dispatch_namespace: &str, target: &str) -> Result<ResolvedTarget, RelayError> {
    let requested = target.trim();
    match classify_principal_id(requested) {
        Some(PrincipalType::User) => Ok(ResolvedTarget {
            bundle_name: dispatch_namespace.to_string(),
            session_id: Some(requested.to_string()),
            relay_wide: true,
        }),
        Some(PrincipalType::Session) => {
            let (session_id, bundle_name) = split_principal_id(requested)
                .expect("session classification implies a parseable suffix");
            Ok(ResolvedTarget {
                bundle_name: bundle_name.to_string(),
                session_id: Some(session_id.to_string()),
                relay_wide: false,
            })
        }
        Some(PrincipalType::Application | PrincipalType::Relay) => {
            let namespace = split_principal_id(requested)
                .map(|(_, namespace)| namespace)
                .unwrap_or_default();
            Err(relay_error(
                "validation_unsupported_namespace",
                "target namespace names no routable recipient for this operation",
                Some(json!({ "target": requested, "namespace": namespace })),
            ))
        }
        None => Err(relay_error(
            "validation_unqualified_target",
            "target must be a fully-qualified principal id (id@namespace)",
            Some(json!({ "target": requested })),
        )),
    }
}

/// Resolves a Send's fully-qualified targets into a config-free [`ResolvedRoute`]
/// (the `MultiTarget` resolution stage): a fan-out of [`resolve_target`] over
/// every target.
///
/// `dispatch_namespace` is the requester's home namespace (its bundle, or
/// `GLOBAL`); see [`requester_home_namespace`].
pub(super) fn resolve_send_route(
    dispatch_namespace: &str,
    requester_session: &str,
    targets: &[String],
) -> Result<ResolvedRoute, RelayError> {
    let resolved = targets
        .iter()
        .map(|target| resolve_target(dispatch_namespace, target))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ResolvedRoute {
        dispatch_namespace: dispatch_namespace.to_string(),
        requester_session: requester_session.to_string(),
        targets: resolved,
    })
}

/// The shared `SingleTarget` resolution stage for the complementary read/write
/// operations Look (inspect) and Raww (write): one [`resolve_target`] call.
///
/// Relay-wide (`@GLOBAL`) targets resolve like any other; whether the resolved
/// target's transport supports the operation is the handlers' capability check
/// (`can_be_looked` / `can_be_written`, derived from the target's
/// `SessionType`), which replaced the blanket relay-wide rejection this stage
/// used to apply. Look and Raww resolve identically at this layer, so they
/// share this body but keep distinct entry points ([`resolve_look_route`] /
/// [`resolve_raww_route`]) documenting intent.
///
/// `dispatch_namespace` is the requester's home namespace (its bundle, or
/// `GLOBAL`); see [`requester_home_namespace`].
fn resolve_single_target_route(
    dispatch_namespace: &str,
    requester_session: &str,
    target_session: &str,
) -> Result<ResolvedRoute, RelayError> {
    let target = resolve_target(dispatch_namespace, target_session)?;
    Ok(ResolvedRoute {
        dispatch_namespace: dispatch_namespace.to_string(),
        requester_session: requester_session.to_string(),
        targets: vec![target],
    })
}

/// Resolves a Look's fully-qualified target into a config-free [`ResolvedRoute`];
/// see [`resolve_single_target_route`].
pub(super) fn resolve_look_route(
    dispatch_namespace: &str,
    requester_session: &str,
    target_session: &str,
) -> Result<ResolvedRoute, RelayError> {
    resolve_single_target_route(dispatch_namespace, requester_session, target_session)
}

/// Resolves a Raww's fully-qualified target into a config-free [`ResolvedRoute`];
/// see [`resolve_single_target_route`].
pub(super) fn resolve_raww_route(
    dispatch_namespace: &str,
    requester_session: &str,
    target_session: &str,
) -> Result<ResolvedRoute, RelayError> {
    resolve_single_target_route(dispatch_namespace, requester_session, target_session)
}

/// Builds the config-free [`ResolvedRoute`] for a List (the `BundleEnumerate`
/// resolution stage).
///
/// List names no session target — routing is purely which bundle to enumerate,
/// already resolved by the connection layer's dispatch split — so the route
/// carries a single bundle-level target (no session id, never relay-wide) and
/// the stage cannot fail. The target's `bundle_name` is the enumerated bundle; a
/// cross-namespace enumeration (a peer bundle) raises the required tier to
/// `all:all`, while a same-namespace one stays at `all:home`.
///
/// `dispatch_namespace` is the requester's home namespace (see
/// [`requester_home_namespace`]); `enumerate_bundle` is the bundle whose sessions
/// are listed.
pub(super) fn resolve_list_route(
    dispatch_namespace: &str,
    requester_session: &str,
    enumerate_bundle: &str,
) -> ResolvedRoute {
    ResolvedRoute {
        dispatch_namespace: dispatch_namespace.to_string(),
        requester_session: requester_session.to_string(),
        targets: vec![ResolvedTarget {
            bundle_name: enumerate_bundle.to_string(),
            session_id: None,
            relay_wide: false,
        }],
    }
}
