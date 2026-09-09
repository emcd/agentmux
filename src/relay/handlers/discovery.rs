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
    PrincipalType, classify_principal_id, live_peer_ingress, peer_scope_covers_namespace,
    peer_scope_covers_target, split_principal_id,
};
use super::super::stream::list_namespace_sessions;
use super::super::{
    BundleCatalog, GLOBAL_NAMESPACE, ListedBundle, ListedBundleState, ListedRelay, ListedSession,
    PeerConnectionManager, PeerIngressAuthority, RelayError, RelayRequest, RelayResponse,
    RequestPrincipal, SCHEMA_VERSION, map_config, relay_error,
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
    /// Live peer-ingress authority (`None` off the stream path). Peer ingress
    /// resolves its current grant under this serialization and holds it across
    /// the scope-filtered decision.
    pub(in crate::relay) ingress_authority: Option<PeerIngressAuthority<'a>>,
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
            // Two phases around the shared identity-admin serialization. Phase
            // 1 collects the full candidate snapshot with no lock held, so a
            // stalled bundle probe cannot block scope administration or other
            // ingress decisions. Phase 2 resolves the current grant under the
            // lock and fixes the filtered result there, so a scope update
            // cannot interleave between the grant check and the decision.
            let snapshot = collect_namespace_snapshot(context)?;
            let live = live_peer_ingress(
                context.ingress_authority,
                principal.session_id.as_str(),
                principal.credential_hash.as_deref(),
            )?;
            Ok(filter_namespace_snapshot(
                snapshot,
                live.scope.as_deref(),
                principal.session_id.as_str(),
            )?)
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
        None if ingress => {
            // Two phases, as in `handle_discover_namespaces`: the bundle
            // listing (including tmux readiness probes) is collected with no
            // lock held; the grant is resolved and the filtered result fixed
            // under the shared serialization.
            let snapshot = collect_principal_snapshot(context, namespace.as_str())?;
            let live = live_peer_ingress(
                context.ingress_authority,
                principal.session_id.as_str(),
                principal.credential_hash.as_deref(),
            )?;
            Ok(decide_principal_snapshot(
                snapshot,
                live.scope.as_deref(),
                namespace.as_str(),
            ))
        }
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

/// Phase 1 of receiving-side namespace discovery: collect every namespace on
/// this relay holding at least one principal, from the bundle catalog and the
/// `GLOBAL` registry. Runs with no lock held; bundle configuration loads and
/// registry reads here never block scope administration.
fn collect_namespace_snapshot(
    context: &DiscoveryContext<'_>,
) -> Result<BTreeSet<String>, RelayError> {
    let mut namespaces = BTreeSet::new();
    for paths in context.bundle_catalog.snapshot() {
        let bundle = load_bundle_configuration(context.configuration_roots, &paths.bundle_name)
            .map_err(map_config)?;
        if !bundle.members.is_empty() {
            namespaces.insert(paths.bundle_name);
        }
    }
    if !list_namespace_sessions(GLOBAL_NAMESPACE).is_empty() {
        namespaces.insert(GLOBAL_NAMESPACE.to_string());
    }
    Ok(namespaces)
}

/// Phase 2 of receiving-side namespace discovery: filter a collected snapshot
/// by the peer's current grant under the shared serialization and fix the
/// result there. An empty (no-principal) namespace is omitted even under a
/// matching scope, and an absent scope denies discovery entirely.
fn filter_namespace_snapshot(
    snapshot: BTreeSet<String>,
    scope: Option<&str>,
    requester: &str,
) -> Result<RelayResponse, RelayError> {
    let Some(scope) = scope else {
        return Err(ingress_forbidden());
    };
    let namespaces = snapshot
        .into_iter()
        .filter(|namespace| peer_scope_covers_namespace(Some(scope), namespace))
        .collect::<BTreeSet<_>>();
    if namespaces.is_empty() {
        emit_scope_unmatched(requester, scope);
    }
    Ok(namespaces_response(namespaces, "ingress"))
}

/// Records an ingress scope that covered no namespace on this relay.
///
/// The wire result for this case is an ordinary empty success and must stay one:
/// a namespace-scoped grant covering no principals is required to be omitted from
/// discovery, producing the same result as a namespace that does not exist, so the
/// peer cannot learn which of the two it hit. That rule binds what this relay
/// tells the peer, not what it records for itself.
///
/// Without the scope and the asking principal the surviving record is
/// `namespace_count: 0` alone, which says a peer saw nothing but not which peer or
/// under what grant — enough to notice a misconfiguration, never enough to fix it.
/// Both fields name things the receiving operator issued, so writing them locally
/// discloses nothing the peer did not already present.
fn emit_scope_unmatched(requester: &str, scope: &str) {
    emit_inscription(
        "relay.discovery.namespaces.scope_unmatched",
        &json!({ "requester_session": requester, "scope": scope }),
    );
}

/// Phase 1 of receiving-side principal discovery: build the complete canonical
/// listing for one concrete namespace, including tmux readiness probes. Runs
/// with no lock held so a stalled probe cannot block scope administration or
/// other ingress decisions. Returns `None` when this relay does not currently
/// host the namespace.
fn collect_principal_snapshot(
    context: &DiscoveryContext<'_>,
    namespace: &str,
) -> Result<Option<ListedBundle>, RelayError> {
    if namespace == GLOBAL_NAMESPACE {
        // `GLOBAL` is registry-backed, not a catalog bundle; its snapshot is a
        // fast in-process registry read with no probes, collected here for
        // shape symmetry with hosted bundles.
        return Ok(Some(build_scoped_global_bundle("*")));
    }
    let Some(paths) = context
        .bundle_catalog
        .snapshot()
        .into_iter()
        .find(|paths| paths.bundle_name == namespace)
    else {
        return Ok(None);
    };
    let bundle_config =
        load_bundle_configuration(context.configuration_roots, namespace).map_err(map_config)?;
    let tmux_socket = tmux_socket_path_for_runtime_directory(&paths.runtime_directory);
    let bundle = build_listed_bundle(
        &bundle_config,
        &paths.runtime_directory,
        tmux_socket.as_path(),
    )?;
    Ok(Some(bundle))
}

/// Phase 2 of receiving-side principal discovery: authorize one concrete
/// namespace against the peer's current grant under the shared serialization
/// and fix the collected listing there. A namespace the scope does not cover —
/// including a nonexistent one — is rejected uniformly with
/// `authorization_forbidden`, disclosing no existence. A covered namespace
/// returns its complete collected listing with normal diagnostics and no
/// scope-induced `principals_partial` marker.
fn decide_principal_snapshot(
    snapshot: Option<ListedBundle>,
    scope: Option<&str>,
    namespace: &str,
) -> RelayResponse {
    let Some(scope) = scope else {
        return RelayResponse::Error {
            error: ingress_forbidden(),
        };
    };
    if !scope_covers_namespace(scope, namespace) {
        return RelayResponse::Error {
            error: ingress_forbidden(),
        };
    }
    let bundle = snapshot.unwrap_or_else(|| empty_namespace_bundle(namespace));
    debug_assert!(
        bundle
            .principals
            .iter()
            .all(|principal| peer_scope_covers_target(Some(scope), principal.id.as_str())),
        "a covered namespace must expose only scope-covered principals"
    );
    emit_inscription(
        "relay.discovery.principals.success",
        &json!({
            "namespace": namespace,
            "principal_count": bundle.principals.len(),
            "principals_partial": bundle.principals_partial,
        }),
    );
    RelayResponse::DiscoverPrincipals {
        schema_version: SCHEMA_VERSION.to_string(),
        bundles: vec![bundle],
    }
}

/// Builds the `GLOBAL` listed bundle from the unified registry.
///
/// `GLOBAL` is registry-backed rather than a `BundleCatalog` bundle, so its
/// principals come from `list_namespace_sessions` and there is no bundle
/// configuration, runtime directory, or startup history to fold. Without this
/// path a foreign `GLOBAL` principal request always fell through to an empty
/// bundle even when namespace discovery had advertised `GLOBAL`. A covered
/// `GLOBAL` returns every registry principal with canonical list state —
/// hosted/up iff a principal is ready (see `handle_global_list`) — and no
/// scope-induced `principals_partial` marker.
fn build_scoped_global_bundle(scope: &str) -> ListedBundle {
    let sessions = list_namespace_sessions(GLOBAL_NAMESPACE);
    let mut principals = sessions
        .into_iter()
        .filter(|(principal_id, _, _)| peer_scope_covers_target(Some(scope), principal_id.as_str()))
        .map(|(principal_id, session_type, ready)| ListedSession {
            id: principal_id,
            name: None,
            transport: session_type.into(),
            ready,
        })
        .collect::<Vec<_>>();
    principals.sort_by(|left, right| left.id.cmp(&right.id));
    let hosted = principals.iter().any(|session| session.ready);
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
        principals_partial: None,
    }
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

/// Whether an ingress `scope` covers `namespace` for principal discovery: `*`
/// covers every addressable namespace and a set covers its named namespaces.
/// Fail-closed for any other scope.
fn scope_covers_namespace(scope: &str, namespace: &str) -> bool {
    peer_scope_covers_namespace(Some(scope), namespace)
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
