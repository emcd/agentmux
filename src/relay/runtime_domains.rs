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

use super::confined::CONFINED_DIR_MODE;
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
    /// adopt. Callers that cannot fail fall back to their own state;
    /// fallible hosts refuse instead.
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
/// Returns `validation_runtime_already_hosted` when another live domain in
/// this process owns the same store root.
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
}

/// Opens the domain lock file beneath the identity directory and takes it
/// exclusively without blocking: a held lock means another process owns the
/// store root.
fn acquire_domain_lock(
    domain_key: &Path,
) -> Result<std::fs::File, RelayError> {
    let identity_dir = domain_key.join("identity");
    // Create the identity directory owner-only when it does not exist yet,
    // matching the confined traversal's mode: a plain create_dir_all would
    // leave umask-dependent permissions on a credential-bearing directory.
    let created = !identity_dir.exists();
    std::fs::create_dir_all(&identity_dir).map_err(|source| {
        crate::relay::relay_error(
            "internal_unexpected_failure",
            "failed to ensure identity directory for domain claim",
            Some(json!({
                "path": identity_dir.display().to_string(),
                "cause": source.to_string(),
            })),
        )
    })?;
    if created {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(
            &identity_dir,
            std::fs::Permissions::from_mode(CONFINED_DIR_MODE),
        )
        .map_err(|source| {
            crate::relay::relay_error(
                "internal_unexpected_failure",
                "failed to restrict identity directory for domain claim",
                Some(json!({
                    "path": identity_dir.display().to_string(),
                    "cause": source.to_string(),
                })),
            )
        })?;
    }
    let lock_path = domain_lock_path(domain_key);
    let lock_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|source| {
            crate::relay::relay_error(
                "internal_unexpected_failure",
                "failed to open domain claim file",
                Some(json!({
                    "path": lock_path.display().to_string(),
                    "cause": source.to_string(),
                })),
            )
        })?;
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
                    "path": lock_path.display().to_string(),
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
