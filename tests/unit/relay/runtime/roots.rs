//! Public runtime roots and per-instance layout.
//!
//! The embedding host supplies unrelated configuration and state roots; the
//! runtime owns its layout beneath them and honors relay configuration
//! without assuming fixed parents.

use agentmux::relay::{
    CredentialSource, DeliveryConfiguration, IdentityDescriptor, RelayRuntime,
    check_context, verify_identity,
};

use super::{provision_alpha, test_runtime};
use tempfile::TempDir;

/// Separate, unrelated roots initialize cleanly with Agentmux-owned layout
/// beneath the state root and relay configuration honored from the config
/// root.
#[test]
fn init_accepts_unrelated_roots_and_honors_relay_configuration() {
    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path().join("elsewhere-conf");
    let state_root = temporary.path().join("other-state");
    std::fs::create_dir_all(&configuration_root).expect("create config root");
    std::fs::write(
        configuration_root.join("relay.toml"),
        "require-session-credentials = true\n",
    )
    .expect("write relay.toml");

    let runtime = RelayRuntime::init(
        configuration_root.clone(),
        state_root.clone(),
        DeliveryConfiguration::default(),
    )
    .expect("runtime init");

    assert_eq!(
        runtime.configuration_roots().base_layer(),
        configuration_root.as_path(),
        "the supplied configuration root is used as given"
    );
    assert_eq!(
        runtime.state_root(),
        state_root.as_path(),
        "the supplied state root is used as given"
    );
    assert!(
        runtime.require_session_credentials(),
        "relay.toml require_session_credentials is honored"
    );
    assert!(
        state_root.is_dir(),
        "Agentmux-owned layout is ensured beneath the state root"
    );
}

/// The explicit delivery table wins over any relay.toml `[delivery]` table:
///
/// `init` owns its configuration explicitly rather than re-reading the file
/// the standalone host resolves before constructing its own handle.
#[test]
fn explicit_delivery_configuration_wins_over_relay_toml() {
    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path().join("conf");
    let state_root = temporary.path().join("state");
    std::fs::create_dir_all(&configuration_root).expect("create config root");
    std::fs::write(
        configuration_root.join("relay.toml"),
        "[delivery]\nqueued-envelopes-max = 2000\nqueued-envelopes-per-target-max = 5\n",
    )
    .expect("write relay.toml");

    let runtime = RelayRuntime::init(
        configuration_root,
        state_root,
        DeliveryConfiguration {
            queued_envelopes_max: 7,
            ..DeliveryConfiguration::default()
        },
    )
    .expect("runtime init");

    assert_eq!(
        runtime
            .delivery_context()
            .delivery_configuration()
            .queued_envelopes_max,
        7,
        "the explicit table wins, not the relay.toml one"
    );
}

/// Two runtimes hold fully independent delivery state and shutdown gates.
///
/// Gate independence is asserted through the public check boundary: after A
/// begins shutdown, A's outstanding contexts stop authorizing while B's are
/// unaffected.
#[test]
fn two_runtimes_hold_independent_delivery_state() {
    let temporary_a = TempDir::new().expect("temporary");
    let temporary_b = TempDir::new().expect("temporary");
    let runtime_a = RelayRuntime::init(
        temporary_a.path().join("conf"),
        temporary_a.path().join("state"),
        DeliveryConfiguration {
            queued_envelopes_max: 1,
            ..DeliveryConfiguration::default()
        },
    )
    .expect("runtime A init");
    let runtime_b = RelayRuntime::init(
        temporary_b.path().join("conf"),
        temporary_b.path().join("state"),
        DeliveryConfiguration::default(),
    )
    .expect("runtime B init");

    assert_ne!(
        runtime_a
            .delivery_context()
            .delivery_configuration()
            .queued_envelopes_max,
        runtime_b
            .delivery_context()
            .delivery_configuration()
            .queued_envelopes_max,
        "each handle resolves its own delivery table"
    );
    let psk_a = provision_alpha(&runtime_a);
    let psk_b = provision_alpha(&runtime_b);
    let context_a = verify_identity(
        &runtime_a,
        &IdentityDescriptor::new(
            "alpha@party".to_string(),
            CredentialSource::DirectPsk(psk_a),
        ),
    )
    .expect("context mints in A");
    let context_b = verify_identity(
        &runtime_b,
        &IdentityDescriptor::new(
            "alpha@party".to_string(),
            CredentialSource::DirectPsk(psk_b),
        ),
    )
    .expect("context mints in B");

    runtime_a.begin_shutdown();
    assert_eq!(
        check_context(&runtime_a, &context_a)
            .expect_err("A rejects dispatch after its own shutdown")
            .code,
        "runtime_shutting_down"
    );
    check_context(&runtime_b, &context_b)
        .expect("signalling A must not shut B down");
}

/// A second live runtime on the same state root is refused: one store root
/// hosts one revocation domain per process.
#[test]
fn second_init_on_same_state_root_is_refused() {
    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path().join("conf");
    let state_root = temporary.path().join("state");
    std::fs::create_dir_all(&configuration_root).expect("create config root");

    let _first = RelayRuntime::init(
        configuration_root.clone(),
        state_root.clone(),
        DeliveryConfiguration::default(),
    )
    .expect("first runtime init");
    let failed = match RelayRuntime::init(
        configuration_root,
        state_root,
        DeliveryConfiguration::default(),
    ) {
        Ok(_) => panic!("second init on the same root must fail"),
        Err(error) => error,
    };
    assert_eq!(
        failed.code, "validation_runtime_already_hosted",
        "the refusal names the ownership violation: {failed:?}"
    );
}

/// Dropping the owner releases its root: a later runtime may host it.
#[test]
fn dropped_runtime_releases_its_state_root() {
    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path().join("conf");
    let state_root = temporary.path().join("state");
    std::fs::create_dir_all(&configuration_root).expect("create config root");

    let first = RelayRuntime::init(
        configuration_root.clone(),
        state_root.clone(),
        DeliveryConfiguration::default(),
    )
    .expect("first runtime init");
    drop(first);
    RelayRuntime::init(
        configuration_root,
        state_root,
        DeliveryConfiguration::default(),
    )
    .expect("a released root hosts a new runtime");
}

/// A runtime that never configured session credentials leaves them
/// unenforced by default.
#[test]
fn session_credentials_default_to_unenforced() {
    let (_temporary, runtime) = test_runtime();
    assert!(
        !runtime.require_session_credentials(),
        "absent relay.toml leaves the documented default"
    );
}

/// Descriptors carry routing defaults and metadata without affecting
/// verification inputs.
#[test]
fn descriptor_carries_routing_defaults_and_metadata() {
    let mut descriptor = IdentityDescriptor::new(
        "alpha@party".to_string(),
        CredentialSource::DirectPsk("psk".to_string()),
    );
    descriptor.operation_bundle = Some("party".to_string());
    descriptor
        .metadata
        .insert("team".to_string(), "red".to_string());
    assert_eq!(descriptor.operation_bundle.as_deref(), Some("party"));
    assert_eq!(
        descriptor.metadata.get("team").map(String::as_str),
        Some("red")
    );
}
