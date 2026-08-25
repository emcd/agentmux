//! Stream-registry eviction helpers extracted from `stream/mod.rs`.
//!
//! Three `pub(crate)` entry points (`evict_streams_for_bundle`,
//! `notify_trusted_hosts_of_revocation`, `revoke_streams_for_identity`)
//! share the single private `evict_streams` selector-based core. The cluster
//! holds no internal state; it pulls registry snapshots under lock, drains
//! each target's writer, and signals the per-connection teardown.
//!
//! `EvictionScope` decides whether a matched entry is fully removed (used
//! on bundle unload/reload, when the configuration itself is gone) or only
//! detached from its dynamic stream state (used on credential revocation,
//! where the static bundle-runtime shell must persist for a reconnect).

use crate::relay::RelayResponse;
use crate::relay::identity::scope_permits;

use super::{
    OutgoingFrame, RegistrationSource, RegistryEntry, RelayStreamEvent, is_relay_wide,
    stream_registry, write_stream_frame_to_writer,
};

/// Whether to remove a matched entry entirely during eviction, or only detach
/// its dynamic stream state and keep any static bundle-runtime shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EvictionScope {
    /// Remove only purely dynamic (`Stream`-source) entries; keep bundle-runtime
    /// shells so the configured session can reconnect. Used by credential
    /// revocation.
    DropStreamSourceOnly,
    /// Remove every matched entry, static or dynamic. Used by bundle
    /// unload/reload, where the configuration itself is gone.
    DropEntry,
}

/// Tears down every live stream entry the `selector` matches, writing `response`
/// to each before signalling its read loop to close, then applies `removal` to
/// decide whether the entry is removed or only detached.
///
/// This is the single session-eviction mechanism shared by credential
/// revocation (matched by verified `authenticated_identity`) and bundle
/// unload/reload (matched by namespace). The dynamic stream state is detached
/// before the teardown signal fires so a reconnect is not wedged into an
/// identity-claim conflict against the dying connection. The connection's writer
/// task drains its queue (flushing the `response` frame) before exiting, so the
/// client observes the typed error frame ahead of EOF. Writers and teardown
/// signals are collected under the registry lock and acted on after it is
/// released. Returns the number of connections torn down.
fn evict_streams<F>(selector: F, response: &RelayResponse, removal: EvictionScope) -> usize
where
    F: Fn(&RegistryEntry) -> bool,
{
    let registry = stream_registry();
    let targets = {
        let Ok(mut entries) = registry.entries.lock() else {
            return 0;
        };
        let mut targets = Vec::new();
        let mut to_remove = Vec::new();
        for (key, entry) in entries.iter_mut() {
            if !selector(entry) {
                continue;
            }
            if let (Some(writer), Some(revoke)) = (entry.writer.clone(), entry.revoke.clone()) {
                entry.clear_dynamic_state();
                targets.push((writer, revoke));
            }
            let remove = match removal {
                EvictionScope::DropEntry => true,
                EvictionScope::DropStreamSourceOnly => entry.source == RegistrationSource::Stream,
            };
            if remove {
                to_remove.push(key.clone());
            }
        }
        for key in to_remove {
            entries.remove(&key);
        }
        targets
    };
    let count = targets.len();
    for (writer, revoke) in targets {
        let _ = write_stream_frame_to_writer(
            &writer,
            OutgoingFrame::Response {
                request_id: None,
                response,
            },
        );
        revoke.notify_one();
    }
    count
}

/// Tears down every live stream whose verified `authenticated_identity` matches
/// `principal_id`. Used by `change psk` to revoke a rotated credential and by
/// `drop peer` to revoke a deleted one: a connection holding a credential the
/// store no longer honors is force-disconnected.
/// Socket-trust connections carry no `authenticated_identity` and are never
/// matched: they hold no credential to revoke. A bundle-runtime entry keeps its
/// static shell and a relay-wide entry is removed, in both cases.
///
/// The retained shell is configuration, not permission to return. After a
/// rotation the principal still exists, so it reconnects by presenting the new
/// credential. After a drop the record is gone, so nothing authenticates into
/// that shell: the removed credential fails Hello as unrecognized, and the shell
/// stays only because the bundle still declares the member. Returns the number
/// torn down.
pub(crate) fn revoke_streams_for_identity(principal_id: &str, response: &RelayResponse) -> usize {
    evict_streams(
        |entry| entry.authenticated_identity.as_deref() == Some(principal_id),
        response,
        EvictionScope::DropStreamSourceOnly,
    )
}

/// Evicts every registry entry in `namespace`. Used by the bundle file watcher
/// when a bundle is unloaded (file removed) or reloaded (file modified): each
/// connected session receives the supplied typed error frame
/// (`runtime_bundle_unloaded` / `runtime_bundle_reloaded`) ahead of EOF, and
/// both static and dynamic entries for the namespace are removed so a reload
/// recreates them from the current configuration. Relay-wide principals
/// (`@GLOBAL`/`@EXTERNAL`/`@RELAY`) live in their own reserved namespaces and are
/// never matched. Returns the number of connections torn down.
pub(crate) fn evict_streams_for_bundle(namespace: &str, response: &RelayResponse) -> usize {
    evict_streams(
        |entry| entry.namespace == namespace,
        response,
        EvictionScope::DropEntry,
    )
}

/// Fans an `identity.revoked` event out to every live trusted-host (application
/// principal) connection whose registered scope covers `revoked_principal_id`.
///
/// The revoked principal's own session is torn down separately by
/// [`revoke_streams_for_identity`]; this is the notification to *watching*
/// hosts so they can drop any cached view of the revoked identity. Only entries
/// carrying a scope are considered, and `scope` is set only for application
/// principals, so non-host connections are skipped (a `None` scope is
/// fail-closed in [`scope_permits`]). The per-recipient event is cloned with
/// `target_session` rewritten to the recipient host's principal id, matching the
/// relay-wide event convention used by the snapshot. Writers are collected under
/// the registry lock and written after it is released. Returns the number of
/// hosts notified.
pub(crate) fn notify_trusted_hosts_of_revocation(
    revoked_principal_id: &str,
    template: &RelayStreamEvent,
) -> usize {
    let registry = stream_registry();
    let targets = {
        let Ok(entries) = registry.entries.lock() else {
            return 0;
        };
        let mut targets = Vec::new();
        for entry in entries.values() {
            let Some(writer) = entry.writer.clone() else {
                continue;
            };
            if !scope_permits(entry.scope.as_deref(), revoked_principal_id) {
                continue;
            };
            if !is_relay_wide(entry.principal_class) {
                continue;
            };
            targets.push((writer, entry.principal_id.clone()));
        }
        targets
    };
    let count = targets.len();
    for (writer, host_principal_id) in targets {
        let mut event = template.clone();
        event.target_session = host_principal_id;
        let _ = write_stream_frame_to_writer(&writer, OutgoingFrame::Event { event: &event });
    }
    count
}
