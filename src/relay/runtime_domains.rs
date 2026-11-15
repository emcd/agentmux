//! Single-revocation-domain ownership registry for relay state roots.
//!
//! One store root hosts one runtime per process: [`claim_domain_slot`]
//! refuses a second live claim over the same root, and the socket
//! identity-admin dispatch resolves the owning delivery state for a root
//! instead of assuming the serving connection's own. Entries are weak so a
//! dropped [`RelayRuntime`](super::runtime::RelayRuntime) releases its root
//! only once no holder of the domain remains; cross-process sharing is
//! refused through an exclusive lock file held for the delivery state's
//! lifetime, so live streams keep the claim alive past the handle's drop.

use std::collections::HashMap;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use serde_json::json;

#[cfg(feature = "experimental-public-send")]
use super::catalog::BundleCatalog;
use super::confined::{ConfineError, confined_claim_lock};
use super::identity::invalid_credential_path;
#[cfg(feature = "experimental-public-send")]
use super::peer_connection::PeerConnectionManager;
use super::{DeliveryContext, RelayError};

/// Leaf holding the exclusive cross-process domain claim, beneath the
/// identity directory that already anchors the principal store.
const DOMAIN_LOCK_FILE: &str = ".domain.lock";

/// Process-wide registry of hosted store roots: each canonical state root
/// maps to the delivery state of the domain that owns it.
static RUNTIME_DOMAINS: OnceLock<
    Mutex<HashMap<PathBuf, Weak<DeliveryContext>>>,
> = OnceLock::new();

/// Process-wide registry of relay-scope admin locks, keyed like the domain
/// registry: contexts adopted into one domain share one lock, preserving
/// the single-context serialization the serve path had before adoption.
static DOMAIN_ADMIN_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> =
    OnceLock::new();

/// The domain's live dispatch pair: the shared catalog and peer manager
/// a domain's dispatch resolves through. Stored inside the owning delivery
/// state itself (see `DeliveryContext::wiring_pair`), never in a side
/// registry: the pair's lifetime is exactly the domain's lifetime, a
/// dropped delivery takes its pair with it, and a reclaimed root gets a
/// fresh delivery with a fresh empty slot by construction. No stale
/// entries can linger for a later generation to trip over — and no
/// strong back-edge beyond the delivery the pair already belongs to.
/// Experimental-gated with the wiring surface.
#[cfg(feature = "experimental-public-send")]
pub(in crate::relay) struct InstalledDomainPair {
    pub(in crate::relay) catalog: BundleCatalog,
    pub(in crate::relay) peers: Arc<PeerConnectionManager>,
    /// Whether the pair is readable through live/mirrored reads.
    /// Tentative installs stay unreadable until the installer publishes
    /// them, so initialization ownership never grants operative send
    /// authority by itself.
    pub(in crate::relay) published: bool,
}

/// Readers go through the owning delivery state looked up in the domain
/// map, so no side registry exists whose lifetime could outlive the
/// domain it was installed for.
fn runtime_domains() -> &'static Mutex<HashMap<PathBuf, Weak<DeliveryContext>>>
{
    RUNTIME_DOMAINS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn domain_admin_locks() -> &'static Mutex<HashMap<PathBuf, Arc<Mutex<()>>>> {
    DOMAIN_ADMIN_LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Returns the relay-scope admin lock for the canonical `domain_key`,
/// creating it on first use. Shared by every serve context adopted into
/// the domain.
pub(in crate::relay) fn admin_lock_for(domain_key: &Path) -> Arc<Mutex<()>> {
    domain_admin_locks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry(domain_key.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Path of the exclusive cross-process domain claim file for a state root.
pub(crate) fn domain_lock_path(state_root: &Path) -> PathBuf {
    state_root.join("identity").join(DOMAIN_LOCK_FILE)
}

/// Canonical registry key for a store root: the canonicalized path when it
/// resolves, so two handles naming the same store through different
/// spellings still coordinate.
pub(in crate::relay) fn canonical_domain_key(state_root: &Path) -> PathBuf {
    std::fs::canonicalize(state_root)
        .unwrap_or_else(|_| state_root.to_path_buf())
}

/// Returns the live delivery state owning `state_root`, if a domain hosts
/// it in this process.
pub(in crate::relay) fn owning_delivery_context(
    state_root: &Path,
) -> Option<Arc<DeliveryContext>> {
    let key = canonical_domain_key(state_root);
    runtime_domains()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
        .and_then(Weak::upgrade)
}

/// Outcome of claiming a store root for a delivery state: either the
/// caller adopts a live domain, establishes its own, or finds the root held
/// by another process (which has no locally adoptable delivery state).
pub(crate) enum DomainClaim {
    /// A live domain already owns the root: adopt its delivery state.
    Adopted(Arc<DeliveryContext>),
    /// No live domain owned the root: the passed delivery state now does.
    Claimed,
    /// Another process holds the root: there is no local delivery state to
    /// adopt, and no claim to establish. Both the serving constructor and
    /// fallible hosts refuse rather than serving an unowned domain.
    ForeignHeld,
}

/// Claims the store root for `delivery`, or adopts the live owner: one
/// critical section decides between adopting, claiming, and reporting a
/// foreign holder, so concurrent first constructions converge on one domain
/// instead of interleaving a lookup with a claim. A successful claim
/// installs the cross-process lock file in the delivery state for its
/// lifetime.
///
/// # Errors
///
/// The claim's I/O failures are returned verbatim. A root held by another
/// process is reported as `Ok(DomainClaim::ForeignHeld)` — not an error at
/// this layer — so each caller applies its own refusal: the serving
/// constructor and runtime init map it to
/// `validation_runtime_already_hosted`, while host preparation takes the
/// graceful all-skipped exit.
pub(crate) fn claim_or_adopt(
    state_root: &Path,
    delivery: &Arc<DeliveryContext>,
) -> Result<DomainClaim, RelayError> {
    let domain_key = canonical_domain_key(state_root);
    let domain_key = domain_key.as_path();
    // One critical section covers the lookup, the lock acquisition, and the
    // publication: a concurrent construction can never observe a pending
    // claim, adopt it, and diverge from its outcome. The mutex is never held
    // across blocking work — the flock below is non-blocking — so parking
    // here is bounded by another claim's syscalls.
    let mut domains = runtime_domains()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(owner) = domains.get(domain_key).and_then(Weak::upgrade) {
        return Ok(DomainClaim::Adopted(owner));
    }
    domains.insert(domain_key.to_path_buf(), Arc::downgrade(delivery));
    match acquire_domain_lock(domain_key) {
        Ok(lock) => {
            if !delivery.install_domain_lock(lock) {
                // This delivery state already hosts another root's domain.
                // One delivery owns one root: roll the new slot back rather
                // than dropping the earlier root's lock out from under its
                // live streams and registry entry. The registry guard is
                // still held, so no second lock is taken here.
                domains.remove(domain_key);
                return Err(already_hosted(domain_key));
            }
            Ok(DomainClaim::Claimed)
        },
        Err(error) if error.code == "validation_runtime_already_hosted" => {
            // Another process holds the root. Roll the slot back: adoption
            // must stay contingent on successful ownership, so a later
            // fallible host re-checks the lock instead of adopting a
            // fallback that never held the root and skipping refusal.
            domains.remove(domain_key);
            Ok(DomainClaim::ForeignHeld)
        },
        Err(error) => {
            // A real claim I/O failure owns nothing: roll the slot back so
            // a later construction retries instead of adopting a domain
            // that never held the root.
            domains.remove(domain_key);
            Err(error)
        },
    }
}

/// Claims single-revocation-domain ownership of the canonical `domain_key`
/// for `delivery`, returning the weak handle to store for release on drop.
///
/// The in-process registry refuses a root that already hosts a live domain;
/// the exclusive lock file then refuses a root held by another process. The
/// lock lives in the delivery state, so every `Arc` holder of the domain —
/// live streams, serve contexts, the embedding handle — keeps the
/// cross-process claim alive with it, and the last drop releases it.
///
/// # Errors
///
/// Returns `validation_runtime_already_hosted` when another live domain in
/// this process or another process already owns the same store root.
pub(in crate::relay) fn claim_domain_slot(
    domain_key: &Path,
    delivery: &Arc<DeliveryContext>,
) -> Result<Weak<DeliveryContext>, RelayError> {
    match claim_or_adopt(domain_key, delivery)? {
        DomainClaim::Adopted(_) => Err(already_hosted(domain_key)),
        DomainClaim::Claimed => Ok(Arc::downgrade(delivery)),
        DomainClaim::ForeignHeld => Err(already_hosted(domain_key)),
    }
}

/// Releases `domain_key`, but only when no holder of the domain remains: a
/// live `Weak` means streams, serve contexts, or an embedder handle still
/// reference the delivery state, and ownership is retained until they cease.
/// A dead entry is removed as stale cleanup.
pub(in crate::relay) fn release_domain_slot(
    domain_key: &Path,
    domain_weak: &Weak<DeliveryContext>,
) {
    let mut domains = runtime_domains()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let retained = domains
        .get(domain_key)
        .is_some_and(|entry| Weak::ptr_eq(entry, domain_weak));
    if retained && domain_weak.upgrade().is_none() {
        domains.remove(domain_key);
    }
    // No pair cleanup here: the installed pair lives inside the delivery
    // state itself, so it is released exactly when the delivery drops —
    // never earlier, never later, and with no cross-generation residue.
}

/// Installs the domain's live dispatch pair into the owning delivery
/// state, published and immediately readable, once per domain
/// generation. Experimental-gated with the wiring surface: default
/// builds have no pair slot and no production write path to it.
///
/// Only the embedded mirrored constructor calls this; serve
/// construction itself never installs, so a foreign pair handed to a
/// second serve constructor on an already-owned root is never registered.
/// First installation wins: re-registering the identical pair (same shared
/// identities) is a no-op success, while a different pair over a committed
/// one is refused so a peer-set change forces domain regeneration instead
/// of silently rebinding live dispatch — and a different pair over a
/// still-tentative one fails with an initializing error so the loser
/// retries after the winner commits or aborts. `install_layers` must
/// agree with the configuration identity bound to the owning delivery
/// at domain-ownership acquisition; a mismatch is refused before
/// anything installs. The caller's delivery must be the domain
/// owner or registration is refused — holding the pair types
/// alone confers no authority.
///
/// Lock order is domains-then-pair-slot, matching nothing else in reverse:
/// the pair mutex is never held while acquiring the domain map.
#[cfg(feature = "experimental-public-send")]
pub(crate) fn register_domain_pair(
    delivery: &Arc<DeliveryContext>,
    state_root: &Path,
    catalog: BundleCatalog,
    peers: Arc<PeerConnectionManager>,
    install_layers: &[PathBuf],
) -> Result<bool, RelayError> {
    let domain_key = canonical_domain_key(state_root);
    let owner = Arc::downgrade(delivery);
    // Both locks are held across the check-and-install so a release and
    // reclaim racing this call cannot interleave between them.
    let domains = runtime_domains()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !domains
        .get(domain_key.as_path())
        .is_some_and(|entry| Weak::ptr_eq(entry, &owner))
    {
        return Err(crate::relay::relay_error(
            "validation_wiring_not_owned",
            "only the domain owner can install its live dispatch pair",
            None,
        ));
    }
    delivery.install_live_pair(catalog, peers, install_layers)
}

/// Installs the domain's dispatch pair into the owning delivery state as
/// tentative: reserved but unreadable until
/// [`publish_domain_pair`] runs. Only the host prepare path calls this;
/// the reservation is the preflight (a domain that already hosts a pair
/// fails here, before workers spawn or listeners bind) without granting
/// operative send authority. `install_layers` must agree with the
/// configuration identity bound to the owning delivery: a mismatched
/// adopted prepare is refused before installing or starting anything
/// foreign. Same ownership and first-installation-wins rules as
/// [`register_domain_pair`].
/// Experimental-gated with the wiring surface.
#[cfg(feature = "experimental-public-send")]
pub(crate) fn register_tentative_domain_pair(
    delivery: &Arc<DeliveryContext>,
    state_root: &Path,
    catalog: BundleCatalog,
    peers: Arc<PeerConnectionManager>,
    install_layers: &[PathBuf],
) -> Result<bool, RelayError> {
    let domain_key = canonical_domain_key(state_root);
    let owner = Arc::downgrade(delivery);
    let domains = runtime_domains()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !domains
        .get(domain_key.as_path())
        .is_some_and(|entry| Weak::ptr_eq(entry, &owner))
    {
        return Err(crate::relay::relay_error(
            "validation_wiring_not_owned",
            "only the domain owner can install its live dispatch pair",
            None,
        ));
    }
    delivery.install_tentative_live_pair(catalog, peers, install_layers)
}

/// Publishes the domain's tentatively installed pair, making it readable
/// through live/mirrored reads. Only the host prepare path calls this,
/// once, at its commit boundary after every fallible step: dispatchability
/// starts here and not earlier. Experimental-gated with the wiring
/// surface.
#[cfg(feature = "experimental-public-send")]
pub(crate) fn publish_domain_pair(
    delivery: &Arc<DeliveryContext>,
    state_root: &Path,
) -> Result<(), RelayError> {
    let domain_key = canonical_domain_key(state_root);
    let owner = Arc::downgrade(delivery);
    let owned = runtime_domains()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(domain_key.as_path())
        .is_some_and(|entry| Weak::ptr_eq(entry, &owner));
    if !owned {
        return Err(crate::relay::relay_error(
            "validation_wiring_not_owned",
            "only the domain owner can publish its live dispatch pair",
            None,
        ));
    }
    delivery.publish_live_pair()
}

/// Reports whether the domain currently holds a tentative (installed but
/// unpublished) pair. Readers use this to fail closed with an
/// initializing error rather than treating the domain as merely
/// unregistered. Experimental-gated with the wiring surface.
#[cfg(feature = "experimental-public-send")]
pub(in crate::relay) fn domain_pair_pending(state_root: &Path) -> bool {
    let domain_key = canonical_domain_key(state_root);
    let delivery = runtime_domains()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(domain_key.as_path())
        .and_then(|entry| entry.upgrade());
    delivery
        .as_ref()
        .is_some_and(|delivery| delivery.pair_pending())
}

/// Removes the domain's installed dispatch pair from the owning
/// delivery state, if any. The host prepare path calls this when
/// preparation fails after the early tentative install, so a failed
/// preparation never leaves a retained tentative pair behind: the next
/// preparation over the same live domain starts from an empty slot
/// instead of conflicting with its predecessor's residue. Only the
/// domain owner can clear; a non-owner call is a no-op success so
/// cleanup on a failing error path cannot itself fail.
/// Experimental-gated with the wiring surface.
#[cfg(feature = "experimental-public-send")]
pub(crate) fn unregister_domain_pair(
    delivery: &Arc<DeliveryContext>,
    state_root: &Path,
) {
    let domain_key = canonical_domain_key(state_root);
    let owner = Arc::downgrade(delivery);
    let owned = runtime_domains()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(domain_key.as_path())
        .is_some_and(|entry| Weak::ptr_eq(entry, &owner));
    if owned {
        delivery.clear_live_pair();
    }
}

/// Returns the domain's installed dispatch pair with its owning delivery,
/// or `None` when no pair was installed, the installed pair is still
/// tentative, or the owning domain lapsed. Callers treat absence as
/// fail-closed: there is no fallback pair, and tentative pairs are never
/// exposed. The pair is read from the owning delivery state itself, so a
/// reclaimed root can never surface a previous generation's pair.
/// Experimental-gated with the wiring surface, its only reader.
#[cfg(feature = "experimental-public-send")]
pub(in crate::relay) fn domain_live_pair(
    state_root: &Path,
) -> Option<(
    Arc<DeliveryContext>,
    BundleCatalog,
    Arc<PeerConnectionManager>,
)> {
    let domain_key = canonical_domain_key(state_root);
    let delivery = runtime_domains()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(domain_key.as_path())?
        .upgrade()?;
    let (catalog, peers) = delivery.read_live_pair()?;
    Some((delivery, catalog, peers))
}

/// Opens the domain lock file beneath the identity directory through
/// retained-handle no-follow traversal, then takes it exclusively without
/// blocking: a held lock means another process owns the store root.
///
/// A symlinked identity directory or lock leaf is refused instead of
/// followed, and a non-regular leaf is refused rather than flocked, so the
/// flock always pins the root's own regular file: an atomic replacement of
/// the principal store beside it cannot strand a second owner on a new
/// inode. The returned descriptor is never re-resolved, so path exchanges
/// after this point cannot redirect the held claim. Routine release only
/// closes the descriptor; the claim inode is never unlinked or recreated.
fn acquire_domain_lock(
    domain_key: &Path,
) -> Result<std::fs::File, RelayError> {
    let relative = Path::new("identity").join(DOMAIN_LOCK_FILE);
    let lock_file = confined_claim_lock(domain_key, &relative).map_err(
        |error| match error {
            ConfineError::Symlink { component } => invalid_credential_path(
                component.as_str(),
                "domain claim ancestor is a symlink",
            ),
            ConfineError::Exchanged { component } => invalid_credential_path(
                component.as_str(),
                "domain claim ancestor changed during claim",
            ),
            ConfineError::DirSync { .. } => crate::relay::relay_error(
                "internal_unexpected_failure",
                "failed to sync domain claim directory",
                Some(json!({
                    "path": domain_key.display().to_string(),
                })),
            ),
            ConfineError::Io { source } => crate::relay::relay_error(
                "internal_unexpected_failure",
                "failed to open domain claim file",
                Some(json!({
                    "path": domain_lock_path(domain_key).display().to_string(),
                    "cause": source.to_string(),
                })),
            ),
        },
    )?;
    // SAFETY: `flock` takes an owned open file descriptor and no other
    // memory; the flags request an exclusive, non-blocking take.
    let taken = unsafe {
        libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB)
    };
    if taken != 0 {
        let source = std::io::Error::last_os_error();
        return Err(if source.kind() == std::io::ErrorKind::WouldBlock {
            already_hosted(domain_key)
        } else {
            crate::relay::relay_error(
                "internal_unexpected_failure",
                "failed to claim domain lock file",
                Some(json!({
                    "path": domain_lock_path(domain_key).display().to_string(),
                    "cause": source.to_string(),
                })),
            )
        });
    }
    Ok(lock_file)
}

/// Refusal for a store root that already hosts a live domain, in this
/// process or another.
pub(in crate::relay) fn already_hosted(domain_key: &Path) -> RelayError {
    crate::relay::relay_error(
        "validation_runtime_already_hosted",
        "a relay domain already hosts this state root; sharing one store across runtimes is refused",
        Some(json!({
            "path": domain_key.display().to_string(),
        })),
    )
}
