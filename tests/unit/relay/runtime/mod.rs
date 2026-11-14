//! Public runtime tests: handle, roots, provisioning, verified context.
//!
//! Every test drives the public `agentmux::relay` runtime surface only:
//! runtime initialization, provisioning, verification, context checks, and
//! introspection. Dispatch adjacency is asserted through `check_context`,
//! the single boundary where the binding epoch, instance binding, and
//! shutdown gate are enforced.

use agentmux::relay::{
    DeliveryConfiguration, ProvisionParams, RelayRuntime, provision_principal,
};
use tempfile::TempDir;

/// In-process public-send dispatch tests. Compiled only under the
/// experimental cutover feature, matching the gated surface they
/// exercise: without it `dispatch_send`/`SendOperation` do not exist.
#[cfg(feature = "experimental-public-send")]
mod dispatch_send;
mod live_revocation;
mod provisioning;
mod roots;
mod transactions;
mod verified_context;

/// Initializes a runtime on fresh, unrelated config and state roots.
pub(super) fn test_runtime() -> (TempDir, RelayRuntime) {
    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path().join("conf");
    let state_root = temporary.path().join("state");
    std::fs::create_dir_all(&configuration_root).expect("create config root");
    let runtime = RelayRuntime::init(
        configuration_root,
        state_root,
        DeliveryConfiguration::default(),
    )
    .expect("runtime init");
    (temporary, runtime)
}

/// Shares one runtime across threads for barrier-driven interleavings.
pub(super) fn shared_runtime() -> (TempDir, std::sync::Arc<RelayRuntime>) {
    let (temporary, runtime) = test_runtime();
    (temporary, std::sync::Arc::new(runtime))
}

/// Provisions a session principal and returns its one-time PSK.
pub(super) fn provision_alpha(runtime: &RelayRuntime) -> String {
    provision_principal(
        runtime,
        ProvisionParams::new("alpha@party".to_string()),
    )
    .expect("provision alpha")
    .psk
    .expect("one-time raw PSK")
}
