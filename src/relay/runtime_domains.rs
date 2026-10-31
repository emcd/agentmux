//! Single-revocation-domain ownership registry for relay state roots.
//!
//! One store root hosts one runtime per process: [`claim_domain_slot`]
//! refuses a second live runtime on the same root, and the socket
//! identity-admin dispatch resolves the owning delivery state for a root
//! instead of assuming the serving connection's own. Entries are weak so a
//! dropped runtime releases its root; cross-process sharing is outside this
//! registry and remains operator error.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use serde_json::json;

use super::{DeliveryContext, RelayError};

/// Process-wide registry of hosted store roots: each canonical state root
/// maps to the delivery state of the runtime that owns it.
static RUNTIME_DOMAINS: OnceLock<
    Mutex<HashMap<PathBuf, Weak<DeliveryContext>>>,
> = OnceLock::new();

fn runtime_domains() -> &'static Mutex<HashMap<PathBuf, Weak<DeliveryContext>>>
{
    RUNTIME_DOMAINS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Canonical registry key for a store root: the canonicalized path when it
/// resolves, so two handles naming the same store through different
/// spellings still coordinate.
pub(in crate::relay) fn canonical_domain_key(state_root: &Path) -> PathBuf {
    std::fs::canonicalize(state_root)
        .unwrap_or_else(|_| state_root.to_path_buf())
}

/// Returns the live delivery state owning `state_root`, if a runtime hosts
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

/// Claims single-revocation-domain ownership of the canonical `domain_key`
/// for `delivery`, returning the weak handle to store for release on drop.
///
/// # Errors
///
/// Returns `validation_runtime_already_hosted` when another live runtime in
/// this process already hosts the same state root.
pub(in crate::relay) fn claim_domain_slot(
    domain_key: &Path,
    delivery: &Arc<DeliveryContext>,
) -> Result<Weak<DeliveryContext>, RelayError> {
    let mut domains = runtime_domains()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if domains.get(domain_key).and_then(Weak::upgrade).is_some() {
        return Err(crate::relay::relay_error(
            "validation_runtime_already_hosted",
            "a relay runtime already hosts this state root in this process; sharing one store across runtimes is refused",
            Some(json!({
                "path": domain_key.display().to_string(),
            })),
        ));
    }
    let domain_weak = Arc::downgrade(delivery);
    domains.insert(domain_key.to_path_buf(), Weak::clone(&domain_weak));
    Ok(domain_weak)
}

/// Releases `domain_key` when the registry still points at `domain_weak`:
/// a stale entry is left for the owner that replaced it.
pub(in crate::relay) fn release_domain_slot(
    domain_key: &Path,
    domain_weak: &Weak<DeliveryContext>,
) {
    let mut domains = runtime_domains()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let still_owner = domains
        .get(domain_key)
        .is_some_and(|entry| Weak::ptr_eq(entry, domain_weak));
    if still_owner {
        domains.remove(domain_key);
    }
}
