//! Relay-alias, namespace, and cross-relay principal discovery handlers.
//!
//! Discovery is relay-wide, not per-bundle: the connection layer dispatches here
//! directly, bypassing `handle_request`. Three orthogonal shapes: `list.relays`
//! enumerates the local outbound routing table; `list.namespaces` and cross-relay
//! `list.principals` discover a namespace's contents either locally or, with a
//! `relay` selector, on one configured peer.
//!
//! Foreign discovery is bounded by two trust domains. The origin authorizes the
//! requester's `list` control at the `all` tier before contacting a peer, then
//! forwards a request with the `relay` selector cleared so a peer can never
//! re-forward (no chaining). The receiving relay derives every result from its
//! own bundle catalog and `GLOBAL` registry, filtered by the authenticated peer
//! principal's registered ingress `scope`; it never trusts an origin-supplied
//! catalog, namespace, or alias.

use crate::configuration::ConfigurationRoots;
use std::collections::BTreeSet;

use serde_json::json;

use crate::configuration::load_bundle_configuration;
use crate::runtime::inscriptions::emit_inscription;
use crate::runtime::paths::tmux_socket_path_for_runtime_directory;

use super::super::authorization::{authorize_discovery_origin, requester_list_reaches_all};
use super::super::identity::{
    PrincipalType, classify_principal_id, scope_permits, split_principal_id,
};
use super::super::stream::list_namespace_sessions;
use super::super::{
    BundleCatalog, GLOBAL_NAMESPACE, ListedBundle, ListedBundleState, ListedRelay, ListedSession,
    PeerConnectionManager, RelayError, RelayRequest, RelayResponse, RequestPrincipal,
    SCHEMA_VERSION, canonical_session_id, map_config, relay_error,
};
use super::listing::build_listed_bundle;

/// Shared, connection-independent handles threaded to every discovery request.
/// Grouped so the handler signatures stay within the argument budget rather than
/// suppressing the lint.
pub(in crate::relay) struct DiscoveryContext<'a> {
    pub(in crate::relay) configuration_roots: &'a ConfigurationRoots,
    pub(in crate::relay) bundle_catalog: &'a BundleCatalog,
    pub(in crate::relay) peer_connection_manager: &'a PeerConnectionManager,
    /// This relay's configured outbound peer aliases, sorted, read from the
    /// normalized `[[peers]]` configuration (never by dialing the connection
    /// manager). The single source for `list.relays`.
    pub(in crate::relay) configured_relay_aliases: &'a [String],
}

/// Enumerates this relay's configured outbound peer aliases without dialing.
///
/// Only the local alias is exposed; addresses, `connect-as` identities, and
/// credentials are never discovery output. Configured peer aliases are relay-wide
/// cross-boundary routing information, so this requires the requester's `list`
/// control at the `all` tier.
pub(in crate::relay) fn handle_list_relays(
    context: &DiscoveryContext<'_>,
    principal: &RequestPrincipal,
) -> Result<RelayResponse, RelayError> {
    emit_inscription(
        "relay.discovery.relays.request",
        &json!({ "requester_session": principal.session_id }),
    );
    authorize_discovery_origin(context.configuration_roots, principal.session_id.as_str())?;
    let mut aliases: Vec<String> = context.configured_relay_aliases.to_vec();
    aliases.sort();
    aliases.dedup();
    let relays = aliases
        .into_iter()
        .map(|alias| ListedRelay { alias })
        .collect::<Vec<_>>();
    emit_inscription(
        "relay.discovery.relays.success",
        &json!({ "relay_count": relays.len() }),
    );
    Ok(RelayResponse::ListRelays {
        schema_version: SCHEMA_VERSION.to_string(),
        relays,
    })
}

/// Discovers namespaces locally or forwards discovery to one configured peer.
pub(in crate::relay) fn handle_discover_namespaces(
    context: &DiscoveryContext<'_>,
    principal: &RequestPrincipal,
    relay: Option<String>,
) -> Result<RelayResponse, RelayError> {
    let ingress = is_relay_principal(principal);
    match relay {
        Some(alias) => {
            reject_peer_reforward(ingress)?;
            authorize_discovery_origin(context.configuration_roots, principal.session_id.as_str())?;
            forward_discovery(
                context,
                alias.as_str(),
                &RelayRequest::DiscoverNamespaces { relay: None },
            )
        }
        None if ingress => {
            receiving_namespace_discovery(context, principal.ingress_scope.as_deref())
        }
        None => local_namespace_discovery(context, principal),
    }
}

/// Discovers principals in one concrete foreign namespace on a configured peer,
/// or serves the ingress side of a forwarded principal-discovery request.
pub(in crate::relay) fn handle_discover_principals(
    context: &DiscoveryContext<'_>,
    principal: &RequestPrincipal,
    relay: Option<String>,
    namespace: String,
) -> Result<RelayResponse, RelayError> {
    let ingress = is_relay_principal(principal);
    match relay {
        Some(alias) => {
            reject_peer_reforward(ingress)?;
            authorize_discovery_origin(context.configuration_roots, principal.session_id.as_str())?;
            forward_discovery(
                context,
                alias.as_str(),
                &RelayRequest::DiscoverPrincipals {
                    relay: None,
                    namespace,
                },
            )
        }
        None if ingress => receiving_principal_discovery(
            context,
            principal.ingress_scope.as_deref(),
            namespace.as_str(),
        ),
        None => Err(relay_error(
            "internal_unexpected_request",
            "local principal discovery is served by List, not DiscoverPrincipals",
            None,
        )),
    }
}

/// Local namespace visibility mirrors local principal visibility: a requester
/// authorized below `all` sees only its home namespace and `GLOBAL`, while an
/// `all` requester sees every configured bundle namespace plus `GLOBAL`.
fn local_namespace_discovery(
    context: &DiscoveryContext<'_>,
    principal: &RequestPrincipal,
) -> Result<RelayResponse, RelayError> {
    let requester = principal.session_id.as_str();
    let mut namespaces = BTreeSet::new();
    namespaces.insert(GLOBAL_NAMESPACE.to_string());
    if requester_list_reaches_all(context.configuration_roots, requester)? {
        for paths in context.bundle_catalog.snapshot() {
            namespaces.insert(paths.bundle_name);
        }
    } else if let Some((_, home_namespace)) = split_principal_id(requester) {
        namespaces.insert(home_namespace.to_string());
    }
    Ok(namespaces_response(namespaces, "local"))
}

/// Receiving-side namespace discovery: derive namespaces from this relay's own
/// catalog and `GLOBAL` registry, keeping only those with at least one principal
/// covered by the peer's ingress `scope`. An empty (no-principal) namespace is
/// therefore omitted even under a matching namespace scope, and an absent scope
/// denies discovery entirely.
fn receiving_namespace_discovery(
    context: &DiscoveryContext<'_>,
    scope: Option<&str>,
) -> Result<RelayResponse, RelayError> {
    if scope.is_none() {
        return Err(ingress_forbidden());
    }
    let mut namespaces = BTreeSet::new();
    for paths in context.bundle_catalog.snapshot() {
        let bundle = load_bundle_configuration(context.configuration_roots, &paths.bundle_name)
            .map_err(map_config)?;
        let covered = bundle.members.iter().any(|member| {
            scope_permits(
                scope,
                canonical_session_id(member.id.as_str(), paths.bundle_name.as_str()).as_str(),
            )
        });
        if covered {
            namespaces.insert(paths.bundle_name);
        }
    }
    let global_covered = list_namespace_sessions(GLOBAL_NAMESPACE)
        .iter()
        .any(|(principal_id, _, _)| scope_permits(scope, principal_id.as_str()));
    if global_covered {
        namespaces.insert(GLOBAL_NAMESPACE.to_string());
    }
    Ok(namespaces_response(namespaces, "ingress"))
}

/// Receiving-side principal discovery for one concrete namespace. A namespace the
/// scope does not cover — including a nonexistent one — is rejected uniformly with
/// `authorization_forbidden`, disclosing no existence. A covered namespace returns
/// the canonical listed bundle filtered to scope-covered principals, marked
/// `principals_partial` when a principal-scoped grant omitted others.
fn receiving_principal_discovery(
    context: &DiscoveryContext<'_>,
    scope: Option<&str>,
    namespace: &str,
) -> Result<RelayResponse, RelayError> {
    let Some(scope) = scope else {
        return Err(ingress_forbidden());
    };
    if !scope_covers_namespace(scope, namespace) {
        return Err(ingress_forbidden());
    }
    // `GLOBAL` is registry-backed, not a catalog bundle, so it is discovered from
    // the unified registry rather than a bundle configuration; every other
    // namespace is a hosted bundle.
    let bundle = if namespace == GLOBAL_NAMESPACE {
        build_scoped_global_bundle(scope)
    } else {
        build_scoped_namespace_bundle(context, scope, namespace)?
    };
    emit_inscription(
        "relay.discovery.principals.success",
        &json!({
            "namespace": namespace,
            "principal_count": bundle.principals.len(),
            "principals_partial": bundle.principals_partial,
        }),
    );
    Ok(RelayResponse::DiscoverPrincipals {
        schema_version: SCHEMA_VERSION.to_string(),
        bundles: vec![bundle],
    })
}

/// Builds the scope-filtered listed bundle for a covered namespace. Reuses the
/// canonical listed-bundle builder, then trims principals to those the scope
/// permits, setting `principals_partial` when a strict subset survives.
fn build_scoped_namespace_bundle(
    context: &DiscoveryContext<'_>,
    scope: &str,
    namespace: &str,
) -> Result<ListedBundle, RelayError> {
    let Some(paths) = context
        .bundle_catalog
        .snapshot()
        .into_iter()
        .find(|paths| paths.bundle_name == namespace)
    else {
        // The scope names a namespace this relay does not currently host: expose
        // an empty listing rather than disclose the mismatch, matching the
        // empty-namespace outcome.
        return Ok(empty_namespace_bundle(namespace));
    };
    let bundle_config =
        load_bundle_configuration(context.configuration_roots, namespace).map_err(map_config)?;
    let tmux_socket = tmux_socket_path_for_runtime_directory(&paths.runtime_directory);
    let mut bundle = build_listed_bundle(
        &bundle_config,
        &paths.runtime_directory,
        tmux_socket.as_path(),
    )?;
    let total = bundle.principals.len();
    bundle
        .principals
        .retain(|principal| scope_permits(Some(scope), principal.id.as_str()));
    // `principals_partial` reflects an actual omission of configured principals.
    if bundle.principals.len() < total {
        bundle.principals_partial = Some(true);
    }
    // An exact-principal grant authorizes addressing the covered principals only,
    // regardless of whether an omission happened to occur. Suppress every
    // bundle-level diagnostic — hosting/state/startup health and the
    // startup-failure history — because it describes namespace-wide state outside
    // the grant. This must not hinge on the partial marker: startup history is
    // keyed by session id independent of current membership, so a stale record
    // for a removed or out-of-scope principal would otherwise leak through a
    // grant whose sole covered principal is the only member currently configured.
    if is_exact_principal_scope(scope) {
        suppress_bundle_diagnostics(&mut bundle);
    }
    Ok(bundle)
}

/// Builds the scope-filtered `GLOBAL` listed bundle from the unified registry.
///
/// `GLOBAL` is registry-backed rather than a `BundleCatalog` bundle, so its
/// principals come from `list_namespace_sessions` and there is no bundle
/// configuration, runtime directory, or startup history to fold. Without this
/// path a foreign `GLOBAL` principal request always fell through to an empty
/// bundle even when namespace discovery had advertised `GLOBAL`.
fn build_scoped_global_bundle(scope: &str) -> ListedBundle {
    let sessions = list_namespace_sessions(GLOBAL_NAMESPACE);
    let total = sessions.len();
    let mut principals = sessions
        .into_iter()
        .filter(|(principal_id, _, _)| scope_permits(Some(scope), principal_id.as_str()))
        .map(|(principal_id, session_type, ready)| ListedSession {
            id: principal_id,
            name: None,
            transport: session_type.into(),
            ready,
        })
        .collect::<Vec<_>>();
    principals.sort_by(|left, right| left.id.cmp(&right.id));
    let partial = principals.len() < total;
    // A namespace (complete) grant mirrors canonical `GLOBAL` list state —
    // hosted/up iff a covered principal is ready (see `handle_global_list`). An
    // exact-principal grant is addressing-only, so it keeps neutral diagnostics
    // like every other subset view.
    let hosted = !is_exact_principal_scope(scope) && principals.iter().any(|session| session.ready);
    let state = if hosted {
        ListedBundleState::Up
    } else {
        ListedBundleState::Down
    };
    ListedBundle {
        id: GLOBAL_NAMESPACE.to_string(),
        hosted,
        state,
        startup_health: None,
        state_reason_code: None,
        state_reason: None,
        startup_failure_count: 0,
        recent_startup_failures: Vec::new(),
        principals,
        principals_partial: partial.then_some(true),
    }
}

/// Whether an ingress `scope` names a single principal (`id@namespace`) rather
/// than a whole namespace. An exact-principal grant is addressing-only: its
/// listing exposes only the covered principals and no bundle-level diagnostics.
fn is_exact_principal_scope(scope: &str) -> bool {
    split_principal_id(scope).is_some()
}

/// Neutralizes bundle-level diagnostics on an exact-principal (addressing-only)
/// listing so it exposes only per-principal data.
fn suppress_bundle_diagnostics(bundle: &mut ListedBundle) {
    bundle.hosted = false;
    bundle.state = ListedBundleState::Down;
    bundle.startup_health = None;
    bundle.state_reason_code = None;
    bundle.state_reason = None;
    bundle.startup_failure_count = 0;
    bundle.recent_startup_failures = Vec::new();
}

/// Forwards a discovery request to the peer named by `alias`, propagating a
/// peer-authored response — success or typed error — verbatim. Peer connection
/// failures surface as their own typed relay errors (`validation_unknown_peer`,
/// `runtime_peer_credential_missing`, `runtime_peer_unavailable`), distinct from
/// a local `relay_unavailable`, rather than being folded into a delivery outcome.
fn forward_discovery(
    context: &DiscoveryContext<'_>,
    alias: &str,
    forwarded: &RelayRequest,
) -> Result<RelayResponse, RelayError> {
    emit_inscription("relay.discovery.request", &json!({ "relay": alias }));
    let response = context
        .peer_connection_manager
        .forward(alias, forwarded)
        .inspect_err(|error| {
            emit_inscription(
                "relay.discovery.relay_error",
                &json!({ "relay": alias, "code": error.code }),
            );
        })?;
    Ok(response)
}

fn namespaces_response(namespaces: BTreeSet<String>, source: &str) -> RelayResponse {
    let namespaces = namespaces.into_iter().collect::<Vec<_>>();
    emit_inscription(
        "relay.discovery.namespaces.success",
        &json!({ "source": source, "namespace_count": namespaces.len() }),
    );
    RelayResponse::DiscoverNamespaces {
        schema_version: SCHEMA_VERSION.to_string(),
        namespaces,
    }
}

fn empty_namespace_bundle(namespace: &str) -> ListedBundle {
    ListedBundle {
        id: namespace.to_string(),
        hosted: false,
        state: ListedBundleState::Down,
        startup_health: None,
        state_reason_code: None,
        state_reason: None,
        startup_failure_count: 0,
        recent_startup_failures: Vec::new(),
        principals: Vec::new(),
        principals_partial: None,
    }
}

fn is_relay_principal(principal: &RequestPrincipal) -> bool {
    classify_principal_id(principal.session_id.as_str()) == Some(PrincipalType::Relay)
}

/// Whether an ingress `scope` covers `namespace` for principal discovery: a bare
/// bundle scope names the namespace directly, and an exact `id@namespace` scope
/// covers its own namespace. Fail-closed for any other scope.
fn scope_covers_namespace(scope: &str, namespace: &str) -> bool {
    scope == namespace
        || matches!(split_principal_id(scope), Some((_, scope_namespace)) if scope_namespace == namespace)
}

/// Rejects a peer relay attempting to re-forward discovery through this relay.
/// A forwarded request always clears its `relay` selector, so a peer principal
/// presenting one is trying to chain — refused before any peer contact.
fn reject_peer_reforward(ingress: bool) -> Result<(), RelayError> {
    if ingress {
        return Err(relay_error(
            "authorization_forbidden",
            "a peer relay ingress requester may not forward cross-relay discovery",
            Some(json!({
                "capability": "ingress",
                "reason": "cross-relay chaining is not permitted for a peer relay ingress requester",
            })),
        ));
    }
    Ok(())
}

/// Uniform ingress denial for absent-scope and out-of-scope-namespace discovery.
/// Deliberately generic so it discloses no namespace existence.
fn ingress_forbidden() -> RelayError {
    relay_error(
        "authorization_forbidden",
        "cross-relay discovery denied by peer relay ingress scope",
        Some(json!({ "capability": "ingress" })),
    )
}
