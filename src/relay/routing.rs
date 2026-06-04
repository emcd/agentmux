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

use super::identity::split_principal_id;

/// Which `policies.toml` control an operation's authorization reads.
#[derive(Clone, Copy, Debug)]
pub(super) enum Capability {
    Look,
    Send,
    List,
}

impl Capability {
    /// The diagnostic capability label surfaced on an `authorization_forbidden`
    /// error for this control.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Look => "look.inspect",
            Self::Send => "send.deliver",
            Self::List => "list.read",
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
/// bundle the requester is authorized in, the requester's bundle-local id, and
/// the resolved targets.
#[derive(Clone, Debug)]
pub(super) struct ResolvedRoute {
    pub dispatch_bundle_name: String,
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
    /// Classifies this target's relationship to the requester within the
    /// dispatch bundle into a uniform scope tier.
    fn tier(&self, dispatch_bundle_name: &str, requester_session: &str) -> ScopeTier {
        // A relay-wide (`@GLOBAL`) target rides the dispatch bundle's context and
        // is delivered via the registry, not by crossing a bundle boundary, so it
        // stays at the home tier rather than demanding `all:all`.
        if self.relay_wide || self.bundle_name == dispatch_bundle_name {
            return match self.session_id.as_deref() {
                Some(session_id) if session_id == requester_session && !self.relay_wide => {
                    ScopeTier::Own
                }
                _ => ScopeTier::Home,
            };
        }
        ScopeTier::All
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
    dispatch_bundle_name: &'a str,
) -> &'a str {
    match split_principal_id(requester_session) {
        Some((_, namespace)) => namespace,
        None => dispatch_bundle_name,
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
        .map(|target| target.tier(&route.dispatch_bundle_name, &route.requester_session))
        .max_by_key(|tier| tier.rank())
        .unwrap_or(ScopeTier::Own)
}
