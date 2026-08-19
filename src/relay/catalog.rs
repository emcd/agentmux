//! The relay's live map of loaded bundles and the hosting intent it tracks for
//! each.
//!
//! Relay-wide rather than connection-scoped: the file watcher writes it as
//! bundles load, unload, and reload; the handlers and the host read it. A
//! connection never owns a catalog, it is handed one through
//! [`ConnectionServeContext`](super::ConnectionServeContext).

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use crate::runtime::paths::BundleRuntimePaths;

/// Whether the relay should keep a bundle's sessions running. Seeded from the
/// bundle's effective autostart when the bundle enters the catalog — `Run` when
/// it autostarts, `Hold` otherwise (a per-bundle `autostart = false` or a
/// relay-wide `--no-autostart` both yield `Hold`) — and then toggled by the
/// operator at runtime via `up` (`Run`) and `down` (`Hold`).
///
/// It expresses *intent*, not live status: a `Run` bundle may still have zero
/// ready sessions, and a `Hold` bundle is simply one the relay must not bring up
/// on its own. The watcher only (re)starts a bundle whose intent is `Run`; a
/// `Hold` bundle absorbs configuration edits without being started. The intent
/// is per-process: it lives only as long as the catalog entry and is not
/// persisted across a relay restart (out of scope for the file watcher).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostingIntent {
    Run,
    Hold,
}

/// A loaded bundle's runtime paths together with the host-level state the relay
/// tracks for it across reconciliation. Folding the state into the catalog entry
/// binds it structurally to the bundle's lifetime: removing the entry (an
/// unload) drops the state with it, so there is no parallel collection to keep
/// consistent by hand.
struct CatalogEntry {
    paths: BundleRuntimePaths,
    hosting_intent: HostingIntent,
}

/// Shared, mutable map from configured bundle name to its [`CatalogEntry`]
/// (resolved runtime paths plus host-level state). Cloned by reference (`Arc`)
/// across all connection workers so each accepted connection can look up its
/// bundle from the Hello frame.
///
/// The map is wrapped in an `RwLock` so the bundle file watcher can load,
/// unload, and reload bundles at runtime (the write side) while connection
/// handlers take short-lived read guards (the read side). No accessor holds a
/// guard across an `.await`: each one copies out what it needs and drops the
/// guard before returning, so the `await_holding_lock` lint is never tripped.
#[derive(Clone, Default)]
pub struct BundleCatalog {
    bundles: Arc<RwLock<HashMap<String, CatalogEntry>>>,
}

impl std::fmt::Debug for BundleCatalog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // RwLock contents are never observable without acquiring the lock, and
        // Debug-formatting while the lock is held could deadlock any thread
        // already partway through an inspect/print. Surface the structural
        // placeholder only.
        formatter
            .debug_struct("BundleCatalog")
            .finish_non_exhaustive()
    }
}

impl BundleCatalog {
    /// Builds a catalog from hosted bundle paths, defaulting every entry to
    /// `HostingIntent::Run`. Used where the entries are known to be running (the
    /// per-request ephemeral catalog) or where the intent is irrelevant (tests).
    pub fn from_paths(paths: impl IntoIterator<Item = BundleRuntimePaths>) -> Self {
        Self::from_entries(paths.into_iter().map(|paths| (paths, HostingIntent::Run)))
    }

    /// Builds a catalog from hosted bundle paths each paired with its initial
    /// hosting intent. Used by the relay host at startup to seed `Hold` for the
    /// bundles that do not autostart.
    pub fn from_entries(
        entries: impl IntoIterator<Item = (BundleRuntimePaths, HostingIntent)>,
    ) -> Self {
        let bundles = entries
            .into_iter()
            .map(|(paths, hosting_intent)| {
                (
                    paths.bundle_name.clone(),
                    CatalogEntry {
                        paths,
                        hosting_intent,
                    },
                )
            })
            .collect();
        Self {
            bundles: Arc::new(RwLock::new(bundles)),
        }
    }

    /// Returns the runtime paths for `bundle_name`, or `None` when no such
    /// bundle is currently loaded.
    pub(super) fn lookup(&self, bundle_name: &str) -> Option<BundleRuntimePaths> {
        self.read()
            .get(bundle_name)
            .map(|entry| entry.paths.clone())
    }

    /// Returns a snapshot of every currently loaded bundle's paths. Used by the
    /// relay host to derive its shutdown cleanup list from the live catalog (so
    /// bundles loaded or unloaded at runtime are reflected) and internally to
    /// replay relay-wide UI snapshots across every loaded bundle.
    pub fn snapshot(&self) -> Vec<BundleRuntimePaths> {
        self.read()
            .values()
            .map(|entry| entry.paths.clone())
            .collect()
    }

    /// Returns the set of currently loaded bundle names. Used by the watcher to
    /// diff the loaded set against the on-disk set during reconciliation.
    pub(super) fn loaded_bundle_names(&self) -> HashSet<String> {
        self.read().keys().cloned().collect()
    }

    /// Inserts or replaces a loaded bundle with an explicit hosting intent. Held
    /// by the watcher's write side when a new bundle file is detected (intent
    /// derived from the bundle's effective autostart) or a modified bundle is
    /// reloaded (always `Run` — a held bundle's reload is suppressed before it
    /// reaches here).
    pub(super) fn insert(&self, paths: BundleRuntimePaths, hosting_intent: HostingIntent) {
        let bundle_name = paths.bundle_name.clone();
        self.write().insert(
            bundle_name,
            CatalogEntry {
                paths,
                hosting_intent,
            },
        );
    }

    /// Removes a loaded bundle, returning its paths when present. Held by the
    /// watcher's write side when a bundle file disappears. Dropping the entry
    /// also drops any operator down intent recorded for it.
    pub(super) fn remove(&self, bundle_name: &str) -> Option<BundleRuntimePaths> {
        self.write().remove(bundle_name).map(|entry| entry.paths)
    }

    /// Records the operator's hosting intent on the bundle's catalog entry. Set
    /// to `Hold` by the `down` handler and `Run` by the `up` handler. A no-op
    /// when the bundle is not loaded — intent is meaningful only for a bundle
    /// that exists, and a missing entry carries no state to leak.
    pub(super) fn set_intent(&self, bundle_name: &str, hosting_intent: HostingIntent) {
        if let Some(entry) = self.write().get_mut(bundle_name) {
            entry.hosting_intent = hosting_intent;
        }
    }

    /// Returns whether `bundle_name` is currently held — i.e. the relay must not
    /// start it on its own. `false` when the bundle is not loaded.
    pub(super) fn is_held(&self, bundle_name: &str) -> bool {
        self.read()
            .get(bundle_name)
            .is_some_and(|entry| entry.hosting_intent == HostingIntent::Hold)
    }

    /// Acquires the read guard, recovering from poisoning.
    ///
    /// A poisoned lock means a writer panicked mid-update; the map itself stays
    /// internally consistent, so recovering the guard is preferable to
    /// propagating the panic to every connection handler that looks up a bundle.
    fn read(&self) -> RwLockReadGuard<'_, HashMap<String, CatalogEntry>> {
        self.bundles
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, HashMap<String, CatalogEntry>> {
        self.bundles
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
