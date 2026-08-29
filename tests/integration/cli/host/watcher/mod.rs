//! Bundle-file watcher behavior: load new bundle, load held non-autostart bundle,
//! unload removed bundle, retain the catalog when a layer becomes unreadable,
//! reload modified bundle, preserve down-intent across edit, hold non-autostart
//! bundle on edit, `--no-watch` reconcile-disable, and `watch-bundles = false`
//! reconcile-disable.
//!
//! The cluster files partition the 8 tests by concern:
//! - [`load_unload`]: bundle-file add at runtime, non-autostart add held, and
//!   remove at runtime (3 tests).
//! - [`retain_reload`]: catalog retention when a layer becomes unreadable,
//!   modified-bundle reload, and reload on byte-identical earlier-layer
//!   appearance/removal (3 tests).
//! - [`edit_intent`]: preserve down-intent across edit, hold non-autostart
//!   bundle on edit (2 tests).
//! - [`reconcile_disable`]: `--no-watch` CLI flag and `watch-bundles = false`
//!   in `relay.toml` (2 tests; the negative-assertion budget lives here).
//!
//! Shared helpers live with their concern: [`hello_keepalive`] holds the
//! stream/reader plumbing used by tests that need to observe an eviction
//! frame on a live connection; [`inscriptions`] holds the assertion and
//! polling helpers used by every cluster.

mod edit_intent;
mod hello_keepalive;
mod inscriptions;
mod load_unload;
mod reconcile_disable;
mod retain_reload;
