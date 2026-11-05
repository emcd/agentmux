//! Public runtime roots and per-instance layout.
//!
//! The embedding host supplies unrelated configuration and state roots; the
//! runtime owns its layout beneath them and honors relay configuration
//! without assuming fixed parents.

use std::sync::Arc;
use std::time::Duration;

use agentmux::relay::{
    BundleCatalog, ConnectionServeContext, CredentialSource,
    DeliveryConfiguration, DeliveryContext, IdentityDescriptor,
    PeerConnectionManager, RelayRuntime, check_context, verify_identity,
};
use agentmux::runtime::paths::BundleRuntimePaths;

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

/// Builds a standalone-style serve context over `state_root` with its own
/// delivery state: the shape a socket host constructs before any runtime
/// exists.
fn serve_context_over(
    configuration_root: &std::path::Path,
    state_root: &std::path::Path,
    delivery: &Arc<DeliveryContext>,
) -> ConnectionServeContext {
    let configuration_roots =
        agentmux::configuration::ConfigurationRoots::single(
            configuration_root,
        );
    let bundle_paths = BundleRuntimePaths::resolve(state_root, "party")
        .expect("bundle paths");
    ConnectionServeContext::new(
        configuration_roots,
        state_root.to_path_buf(),
        BundleCatalog::from_paths([bundle_paths]),
        Arc::new(PeerConnectionManager::from_configuration(state_root, &[])),
        Vec::new(),
        false,
        Duration::from_secs(2),
        Arc::clone(delivery),
    )
    .expect("serve context claims its domain")
}

/// A runtime init over a root already claimed by serve contexts is refused:
/// the first claimer owns the domain and there is no migration of existing
/// registrations. Dropping the claiming contexts releases the root.
#[test]
fn runtime_init_over_serve_claimed_root_is_refused() {
    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path().join("conf");
    let state_root = temporary.path().join("state");
    std::fs::create_dir_all(&configuration_root).expect("create config root");

    let context = {
        let foreign =
            Arc::new(DeliveryContext::new(DeliveryConfiguration::default()));
        let context =
            serve_context_over(&configuration_root, &state_root, &foreign);
        assert!(
            Arc::ptr_eq(context.delivery_context(), &foreign),
            "first claimer keeps its own delivery state"
        );
        context
    };
    let failed = match RelayRuntime::init(
        configuration_root.clone(),
        state_root.clone(),
        DeliveryConfiguration::default(),
    ) {
        Ok(_) => panic!("init over a serve-claimed root must fail"),
        Err(error) => error,
    };
    assert_eq!(
        failed.code, "validation_runtime_already_hosted",
        "the refusal names the ownership violation: {failed:?}"
    );
    drop(context);
    RelayRuntime::init(
        configuration_root,
        state_root,
        DeliveryConfiguration::default(),
    )
    .expect("a released root hosts a new runtime");
}

/// Dropping the runtime while a serve context still references its delivery
/// state does not release the root: ownership is retained until every
/// effect-capable holder ceases. A second init is refused for as long as
/// the domain is alive.
#[test]
fn dropped_runtime_with_live_users_retains_ownership() {
    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path().join("conf");
    let state_root = temporary.path().join("state");
    std::fs::create_dir_all(&configuration_root).expect("create config root");

    let runtime = RelayRuntime::init(
        configuration_root.clone(),
        state_root.clone(),
        DeliveryConfiguration::default(),
    )
    .expect("first runtime init");
    // A foreign delivery state is set aside: the context adopts the owning
    // one, so dropping the runtime afterwards still leaves the domain
    // referenced through the context.
    let foreign =
        Arc::new(DeliveryContext::new(DeliveryConfiguration::default()));
    let context =
        serve_context_over(&configuration_root, &state_root, &foreign);
    assert!(
        Arc::ptr_eq(context.delivery_context(), runtime.delivery_context()),
        "a context over a hosted root adopts the owning delivery state"
    );
    assert!(
        !Arc::ptr_eq(context.delivery_context(), &foreign),
        "the passed foreign delivery must not survive adoption"
    );
    drop(runtime);
    let failed = match RelayRuntime::init(
        configuration_root.clone(),
        state_root.clone(),
        DeliveryConfiguration::default(),
    ) {
        Ok(_) => panic!("init must stay refused while the domain is alive"),
        Err(error) => error,
    };
    assert_eq!(
        failed.code, "validation_runtime_already_hosted",
        "a live domain retains its root past the handle's drop: {failed:?}"
    );
    drop(context);
    RelayRuntime::init(
        configuration_root,
        state_root,
        DeliveryConfiguration::default(),
    )
    .expect("a fully released root hosts a new runtime");
}

/// Concurrent first constructions converge on one domain: every context
/// shares the winner's delivery state rather than each keeping its own.
#[test]
fn concurrent_first_constructions_share_one_domain() {
    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path().join("conf");
    let state_root = temporary.path().join("state");
    std::fs::create_dir_all(&configuration_root).expect("create config root");

    let barrier = Arc::new(std::sync::Barrier::new(4));
    let contexts = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(4);
        for _ in 0..4 {
            let barrier = Arc::clone(&barrier);
            let (configuration_root, state_root) =
                (configuration_root.clone(), state_root.clone());
            handles.push(scope.spawn(move || {
                barrier.wait();
                let foreign = Arc::new(DeliveryContext::new(
                    DeliveryConfiguration::default(),
                ));
                serve_context_over(&configuration_root, &state_root, &foreign)
            }));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("serve construction"))
            .collect::<Vec<_>>()
    });
    {
        let first = contexts[0].delivery_context().clone();
        for context in &contexts[1..] {
            assert!(
                Arc::ptr_eq(context.delivery_context(), &first),
                "concurrent constructions must converge on one domain"
            );
        }
    }
    drop(contexts);
    RelayRuntime::init(
        configuration_root,
        state_root,
        DeliveryConfiguration::default(),
    )
    .expect("a fully released root hosts a new runtime");
}

/// A second process holding the domain claim refuses an init here: the
/// cross-process lock is the teeth of the single-domain rule. The child
/// runs this same test binary filtered to this test with a hold directive.
/// Readiness is a file (not stdout — the harness captures child output).
#[test]
fn cross_process_second_init_is_refused() {
    // Child branch: claim the domain named by the environment and hold it
    // until stdin closes, signalling readiness through the ready file.
    if let Ok(hold_root) = std::env::var("AGENTMUX_TEST_HOLD_DOMAIN") {
        let hold_conf = std::env::var("AGENTMUX_TEST_HOLD_CONF")
            .expect("hold configuration root");
        let hold_ready = std::env::var("AGENTMUX_TEST_HOLD_READY")
            .expect("hold readiness file");
        std::fs::create_dir_all(&hold_conf).expect("hold config root");
        let _held = RelayRuntime::init(
            hold_conf.into(),
            hold_root.into(),
            DeliveryConfiguration::default(),
        )
        .expect("child claims the domain");
        std::fs::write(hold_ready, b"READY").expect("signal readiness");
        let mut drain = Vec::new();
        std::io::Read::read_to_end(&mut std::io::stdin(), &mut drain).ok();
        return;
    }
    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path().join("conf");
    let state_root = temporary.path().join("state");
    let ready_file = temporary.path().join("child.ready");
    std::fs::create_dir_all(&configuration_root).expect("create config root");

    let test_binary = std::env::current_exe().expect("test binary");
    let mut child = std::process::Command::new(test_binary)
        .arg("relay::runtime::roots::cross_process_second_init_is_refused")
        .arg("--exact")
        .env("AGENTMUX_TEST_HOLD_DOMAIN", &state_root)
        .env("AGENTMUX_TEST_HOLD_CONF", &configuration_root)
        .env("AGENTMUX_TEST_HOLD_READY", &ready_file)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("spawn holding child");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !ready_file.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "child must claim the domain before the parent probes"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let failed = match RelayRuntime::init(
        configuration_root.clone(),
        state_root.clone(),
        DeliveryConfiguration::default(),
    ) {
        Ok(_) => panic!("init over a foreign-held root must fail"),
        Err(error) => error,
    };
    assert_eq!(
        failed.code, "validation_runtime_already_hosted",
        "cross-process ownership is refused: {failed:?}"
    );

    // Serve construction on the foreign-held root has no local owner to
    // adopt: the constructor refuses rather than returning an unowned
    // usable context.
    for _ in 0..2 {
        let foreign =
            Arc::new(DeliveryContext::new(DeliveryConfiguration::default()));
        let failed = match ConnectionServeContext::new(
            agentmux::configuration::ConfigurationRoots::single(
                &configuration_root,
            ),
            state_root.clone(),
            BundleCatalog::from_paths([BundleRuntimePaths::resolve(
                &state_root,
                "party",
            )
            .expect("bundle paths")]),
            Arc::new(PeerConnectionManager::from_configuration(
                &state_root,
                &[],
            )),
            Vec::new(),
            false,
            Duration::from_secs(2),
            foreign,
        ) {
            Ok(_) => {
                panic!("serve construction over a held root must fail")
            },
            Err(error) => error,
        };
        assert_eq!(
            failed.code, "validation_runtime_already_hosted",
            "serving-path refusal names the ownership violation: {failed:?}"
        );
    }
    drop(child.stdin.take());
    let status = child.wait().expect("reap holding child");
    assert!(status.success(), "holding child exits cleanly");
}

/// A delivery state that already owns one root cannot claim a second: the
/// second claim is refused rather than dropping the first root's lock out
/// from under its live serve context and registry entry. A raw separate
/// open of the first root's claim file still fails while that context
/// lives, proving the claim was retained and not silently replaced.
#[test]
fn shared_delivery_must_not_release_an_earlier_root_claim() {
    use std::os::unix::io::AsRawFd as _;

    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path().join("conf");
    std::fs::create_dir_all(&configuration_root).expect("create config root");
    let delivery =
        Arc::new(DeliveryContext::new(DeliveryConfiguration::default()));
    let state_a = temporary.path().join("state-a");
    let state_b = temporary.path().join("state-b");
    let _context_a =
        serve_context_over(&configuration_root, &state_a, &delivery);

    let failed = match ConnectionServeContext::new(
        agentmux::configuration::ConfigurationRoots::single(
            &configuration_root,
        ),
        state_b.clone(),
        BundleCatalog::from_paths([BundleRuntimePaths::resolve(
            &state_b, "party",
        )
        .expect("bundle paths")]),
        Arc::new(PeerConnectionManager::from_configuration(&state_b, &[])),
        Vec::new(),
        false,
        Duration::from_secs(2),
        Arc::clone(&delivery),
    ) {
        Ok(_) => panic!("a second root on one delivery must fail"),
        Err(error) => error,
    };
    assert_eq!(
        failed.code, "validation_runtime_already_hosted",
        "one delivery owns one root: {failed:?}"
    );
    let lock_path = state_a.join("identity").join(".domain.lock");
    let probe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open first root's claim file");
    let taken = unsafe {
        libc::flock(probe.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB)
    };
    assert_ne!(
        taken, 0,
        "the first root's claim must survive the refused second claim"
    );
}

/// A claim I/O failure must not yield a usable serving context either: an
/// occupied claim path (here a directory where the lock file belongs) makes
/// the domain claim fail before any ownership exists, and construction
/// reports the failure instead of serving unowned.
#[test]
fn claim_io_failure_must_not_return_a_usable_context() {
    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path().join("conf");
    let state_root = temporary.path().join("state");
    std::fs::create_dir_all(&configuration_root).expect("create config root");
    std::fs::create_dir_all(state_root.join("identity").join(".domain.lock"))
        .expect("directory at lock path");

    let delivery =
        Arc::new(DeliveryContext::new(DeliveryConfiguration::default()));
    let failed = match ConnectionServeContext::new(
        agentmux::configuration::ConfigurationRoots::single(
            &configuration_root,
        ),
        state_root.clone(),
        BundleCatalog::from_paths([BundleRuntimePaths::resolve(
            &state_root,
            "party",
        )
        .expect("bundle paths")]),
        Arc::new(PeerConnectionManager::from_configuration(&state_root, &[])),
        Vec::new(),
        false,
        Duration::from_secs(2),
        delivery,
    ) {
        Ok(_) => panic!("an unclaimable root must not yield a context"),
        Err(error) => error,
    };
    assert_eq!(
        failed.code, "internal_unexpected_failure",
        "claim I/O failures propagate rather than serving unowned: {failed:?}"
    );
    assert_ne!(
        failed.code, "validation_runtime_already_hosted",
        "an I/O failure is not an ownership refusal"
    );
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
