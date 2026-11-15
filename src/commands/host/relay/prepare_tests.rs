// Host domain-claim decision coverage: reachable only through host
// preparation (no public interface drives these paths). Extracted
// from `prepare.rs` so that file stays under the line-count limit.

// Last-resort coverage for the host's domain-claim decisions: they are only
// reachable through host preparation (no public interface drives them).
// One test drives both phases in order: a foreign-held root is refused
// before any hosting, then, with the hold released and the same root owned
// by a live same-process runtime, preparation adopts the owner and startup
// registrations land in the owning domain rather than a discarded fresh one.
#[cfg(test)]
mod domain_claim_tests {
    use std::os::unix::io::AsRawFd as _;

    use tempfile::TempDir;

    use crate::configuration::ConfigurationRoots;
    use crate::relay::domain_lock_path;
    use crate::runtime::paths::{ConfigurationRootSource, RuntimeRoots};
    use crate::runtime::starter::ensure_starter_configuration_layout;

    use super::super::RelayHostPreparation;
    use super::super::prepare::prepare_relay_host;

    fn write_party_bundle(configuration_root: &std::path::Path) {
        std::fs::write(
            configuration_root.join("bundles").join("party.toml"),
            r#"
format-version = 1

[[sessions]]
id = "victim"
name = "Victim"
directory = "/tmp"

[sessions.ui]
"#,
        )
        .expect("write bundle");
    }

    fn test_roots(
        temporary: &TempDir,
    ) -> (RuntimeRoots, std::path::PathBuf, std::path::PathBuf) {
        let configuration_root = temporary.path().join("conf");
        let state_root = temporary.path().join("state");
        let roots = RuntimeRoots {
            configuration_roots: ConfigurationRoots::single(
                &configuration_root,
            ),
            state_root: state_root.clone(),
            // Shared with the other in-crate inscription tests: the
            // configured path is first-wins per process, so per-test
            // temporary paths would fail configuration depending on run
            // order. relay_inscriptions_path appends relay.log, matching
            // the shared test path exactly.
            inscriptions_root:
                crate::runtime::inscriptions::shared_test_inscriptions_path()
                    .parent()
                    .expect("inscription file has a parent")
                    .to_path_buf(),
            configuration_root_source: ConfigurationRootSource::Default,
        };
        ensure_starter_configuration_layout(&roots)
            .expect("starter configuration");
        write_party_bundle(&configuration_root);
        (roots, configuration_root, state_root)
    }

    #[test]
    fn host_domain_claim_decisions_route_preparation() {
        // Phase 1 — a foreign-held root: refusal before any hosting, the
        // serving constructor refuses the same root, and preparing again
        // still refuses. The hold is a raw flock on a separate open
        // description, the way another process would hold it.
        let temporary = TempDir::new().expect("temporary");
        let (roots, configuration_root, state_root) = test_roots(&temporary);

        // Hold the domain claim the way another process would: an open file
        // description with an exclusive, non-blocking flock, kept alive
        // across the prepare call. Same-process flock conflicts on a
        // separate open description, so no child process is needed.
        let lock_path = domain_lock_path(&state_root);
        std::fs::create_dir_all(lock_path.parent().expect("lock parent"))
            .expect("identity directory");
        let lock_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .expect("open domain lock file");
        let taken = unsafe {
            libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB)
        };
        assert_eq!(taken, 0, "test holds the domain lock");

        match prepare_relay_host(&roots, true, None, None)
            .expect("prepare runs")
        {
            RelayHostPreparation::NoHostedBundles => {},
            RelayHostPreparation::Serve(_) => {
                panic!("a foreign-held domain must not yield a served host")
            },
        }
        // The refusal lands before any bundle is hosted: no per-bundle
        // runtime directory may exist. (A bypassed check would proceed into
        // hosting and create it, so this pins the early return, not just
        // the preparation variant.)
        assert!(
            !roots.state_root.join("bundles").exists(),
            "refused preparation must not host bundles"
        );
        // The serving constructor refuses the same root itself: no usable
        // serving context can be built over a domain this process does
        // not own, so there is no fallback for a later prepare to adopt.
        let foreign = std::sync::Arc::new(crate::relay::DeliveryContext::new(
            crate::relay::DeliveryConfiguration::default(),
        ));
        let failed = match crate::relay::ConnectionServeContext::new(
            roots.configuration_roots.clone(),
            roots.state_root.clone(),
            crate::relay::BundleCatalog::from_paths([]),
            std::sync::Arc::new(
                crate::relay::PeerConnectionManager::from_configuration(
                    &roots.state_root,
                    &[],
                ),
            ),
            Vec::new(),
            false,
            std::time::Duration::from_secs(2),
            foreign,
        ) {
            Ok(_) => panic!("serve construction over a held root must fail"),
            Err(error) => error,
        };
        assert_eq!(
            failed.code, "validation_runtime_already_hosted",
            "serving-path refusal names the ownership violation: {failed:?}"
        );
        // Preparing again must still refuse: no local entry was published
        // for the failed construction to adopt.
        match prepare_relay_host(&roots, true, None, None)
            .expect("prepare runs")
        {
            RelayHostPreparation::NoHostedBundles => {},
            RelayHostPreparation::Serve(_) => {
                panic!("a refused constructor must not yield a served host")
            },
        }
        assert!(
            !roots.state_root.join("bundles").exists(),
            "refused preparation must not host bundles"
        );
        drop(lock_file);

        // Phase 2 — the same root owned by a same-process runtime:
        // preparation adopts its delivery state instead of refusing, and
        // startup registrations land in the owner's domain rather than a
        // discarded fresh one.
        let runtime = crate::relay::RelayRuntime::init(
            configuration_root,
            state_root,
            crate::relay::DeliveryConfiguration::default(),
        )
        .expect("runtime init");
        match prepare_relay_host(&roots, true, None, None)
            .expect("prepare runs")
        {
            RelayHostPreparation::Serve(_) => {},
            RelayHostPreparation::NoHostedBundles => {
                panic!("an adopted domain must still be served")
            },
        }
        let principals = crate::relay::registered_principal_ids(
            runtime.delivery_context(),
            "party",
        );
        assert!(
            principals
                .iter()
                .any(|principal| principal == "victim@party"),
            "startup registrations land in the owning domain: {principals:?}"
        );
        // The early-installed pair is what the serve context hosts: a
        // live wiring reads back the same generation with the configured
        // membership. If the install were still late (or missing), this
        // read would fail closed or diverge from the served catalog.
        #[cfg(feature = "experimental-public-send")]
        {
            let wiring = crate::relay::DispatchWiring::live(&runtime)
                .expect("early-installed pair reads back");
            let mut names: Vec<String> = wiring
                .catalog()
                .snapshot()
                .iter()
                .map(|paths| paths.bundle_name.clone())
                .collect();
            names.sort();
            assert_eq!(
                names,
                vec!["party".to_string()],
                "serve reuses the early-installed catalog identity"
            );
        }
    }

    /// A second, valid configuration with distinct memberships over the
    /// SAME state root, sharing the inscriptions root: the adopted
    /// prepare over these roots must refuse rather than host foreign
    /// configuration on the adopted owner's delivery. Experimental-gated
    /// with the wiring tests that use it.
    #[cfg(feature = "experimental-public-send")]
    fn other_roots_with_lone_bundle(
        temporary: &TempDir,
        roots: &RuntimeRoots,
        state_root: std::path::PathBuf,
    ) -> RuntimeRoots {
        let other_configuration = temporary.path().join("other-conf");
        let other_roots = RuntimeRoots {
            configuration_roots: ConfigurationRoots::single(
                &other_configuration,
            ),
            state_root,
            inscriptions_root: roots.inscriptions_root.clone(),
            configuration_root_source: ConfigurationRootSource::Default,
        };
        ensure_starter_configuration_layout(&other_roots)
            .expect("starter configuration");
        std::fs::write(
            other_configuration.join("bundles").join("lone.toml"),
            r#"
format-version = 1

[[sessions]]
id = "castaway"
name = "Castaway"
directory = "/tmp"

[sessions.ui]
"#,
        )
        .expect("write lone bundle");
        other_roots
    }

    /// Preparing over a domain that already hosts a live dispatch pair
    /// is refused before any startup side effect: the atomic early
    /// registration right after the domain claim is itself the
    /// preflight, so no runtime directory is created and no worker
    /// spawns for the rejected preparation.
    #[cfg(feature = "experimental-public-send")]
    #[test]
    fn prepare_refuses_installed_domain_before_side_effects() {
        use crate::relay::DispatchWiring;
        use crate::runtime::error::RuntimeError;

        let temporary = TempDir::new().expect("temporary directory");
        let (roots, configuration_root, state_root) = test_roots(&temporary);
        let runtime = crate::relay::RelayRuntime::init(
            configuration_root,
            state_root.clone(),
            crate::relay::DeliveryConfiguration::default(),
        )
        .expect("runtime claims the domain");
        DispatchWiring::mirrored(&runtime, &[], true)
            .expect("install domain pair");
        match prepare_relay_host(&roots, true, None, None) {
            Ok(_) => {
                panic!("prepare over an installed pair must fail")
            },
            Err(error) => match error {
                RuntimeError::Validation { code, .. } => {
                    assert_eq!(
                        code, "validation_wiring_conflict",
                        "conflict names the installed pair: {code}"
                    );
                },
                error => {
                    panic!("conflict must surface as validation: {error:?}")
                },
            },
        }
        assert!(
            !state_root.join("bundles").exists(),
            "refused preparation must not host bundles"
        );
    }

    /// A paused prepare issues no operative wiring: with the installed
    /// gate armed, preparation parks after the tentative install and
    /// before any bundle startup side effect, and concurrent live and
    /// mirrored reads must fail closed with an initializing error --
    /// never a wiring for a pair that may yet abort. Releasing the gate
    /// lets preparation proceed to `Serve`, after which live reads back
    /// the committed pair. Neutering the fail-closed reads (or the
    /// tentative install) fails this test at the paused reads, which is
    /// the timing proof a post-success readback alone cannot give.
    #[cfg(feature = "experimental-public-send")]
    #[test]
    fn paused_prepare_issues_no_operative_wiring() {
        use std::sync::{Arc as StdArc, Barrier};
        use std::time::Duration;

        use crate::relay::DispatchWiring;
        use crate::relay::test_hooks::{
            PrepareInstalledGate, arm_prepare_installed_gate,
            disarm_prepare_installed_gate,
        };

        let temporary = TempDir::new().expect("temporary directory");
        let (roots, configuration_root, state_root) = test_roots(&temporary);
        let runtime = crate::relay::RelayRuntime::init(
            configuration_root,
            state_root.clone(),
            crate::relay::DeliveryConfiguration::default(),
        )
        .expect("runtime claims the domain");
        let (gate, reached_rx, proceed_tx) =
            PrepareInstalledGate::arm(&roots.state_root);
        arm_prepare_installed_gate(gate);
        let roots_owned = roots.clone();
        let handle = std::thread::spawn(move || {
            prepare_relay_host(&roots_owned, true, None, None)
        });
        reached_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("prepare reaches the installed gate");
        // The unpublished window: live fails closed, and concurrent
        // autostart-requesting mirrored calls fail closed too instead of
        // adopting the tentative pair -- no operative wiring is issuable
        // before the commit boundary, so none can survive an abort.
        match DispatchWiring::live(&runtime) {
            Ok(_) => {
                panic!("live must not read a tentative pair")
            },
            Err(error) => {
                assert_eq!(
                    error.code, "validation_wiring_initializing",
                    "paused live names initialization: {error:?}"
                );
            },
        }
        let runtime = StdArc::new(runtime);
        let barrier = StdArc::new(Barrier::new(8));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let runtime = StdArc::clone(&runtime);
            let barrier = StdArc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                DispatchWiring::mirrored(&runtime, &[], true)
            }));
        }
        for handle in handles {
            match handle.join().expect("mirrored thread joins") {
                Ok(_) => {
                    panic!("mirrored must not adopt a tentative pair")
                },
                Err(error) => {
                    assert_eq!(
                        error.code, "validation_wiring_initializing",
                        "paused mirrored names initialization: {error:?}"
                    );
                },
            }
        }
        proceed_tx.send(()).expect("release preparation");
        disarm_prepare_installed_gate();
        match handle
            .join()
            .expect("prepare thread joins")
            .expect("released prepare runs")
        {
            RelayHostPreparation::Serve(_) => {},
            RelayHostPreparation::NoHostedBundles => {
                panic!("a released prepare must serve")
            },
        }
        // After the commit boundary the pair reads back with its final
        // intents (process-only here, so still `Hold`).
        let wiring = DispatchWiring::live(&runtime)
            .expect("committed pair reads back after release");
        assert!(
            wiring.catalog().is_held("party"),
            "the committed pair carries the final intents"
        );
    }

    /// A preparation that fails after the tentative install leaves no
    /// retained pair: corrupting `users.toml` fails relay-wide principal
    /// registration (after the install, before `Serve`) on an adopted
    /// domain, and the abort guard must clear the owner's slot so a
    /// retry with valid configuration serves instead of conflicting.
    #[cfg(feature = "experimental-public-send")]
    #[test]
    fn failed_adopted_prepare_leaves_no_tentative_pair() {
        use crate::relay::DispatchWiring;
        use crate::runtime::error::RuntimeError;

        let temporary = TempDir::new().expect("temporary directory");
        let (roots, configuration_root, state_root) = test_roots(&temporary);
        let runtime = crate::relay::RelayRuntime::init(
            configuration_root,
            state_root.clone(),
            crate::relay::DeliveryConfiguration::default(),
        )
        .expect("runtime claims the domain");
        std::fs::write(
            temporary.path().join("conf").join("users.toml"),
            "not = [valid",
        )
        .expect("write invalid users configuration");
        match prepare_relay_host(&roots, true, None, None) {
            Ok(_) => {
                panic!("invalid users.toml must fail preparation")
            },
            Err(error) => match error {
                RuntimeError::Validation { code, .. } => {
                    assert_ne!(
                        code, "validation_wiring_conflict",
                        "the failure is the invalid file, not the pair: {code}"
                    );
                },
                error => {
                    panic!("configuration failure is validation: {error:?}")
                },
            },
        }
        assert!(
            DispatchWiring::live(&runtime).is_err(),
            "a failed prepare must not leave a live pair behind"
        );
        std::fs::remove_file(temporary.path().join("conf").join("users.toml"))
            .expect("remove invalid users configuration");
        match prepare_relay_host(&roots, true, None, None).expect("retry runs")
        {
            RelayHostPreparation::Serve(_) => {},
            RelayHostPreparation::NoHostedBundles => {
                panic!("a retry with valid configuration must serve")
            },
        }
        let wiring = DispatchWiring::live(&runtime)
            .expect("the retry installs the committed pair");
        let mut names: Vec<String> = wiring
            .catalog()
            .snapshot()
            .iter()
            .map(|paths| paths.bundle_name.clone())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec!["party".to_string()],
            "the retry serves the committed catalog"
        );
    }

    /// An adopted prepare with a different configuration is refused
    /// before installing or starting anything foreign: the first
    /// installation must agree with the configuration identity bound to
    /// the owning runtime at ownership acquisition, so confB memberships
    /// can never ride deliveryA's passing brand check. live(runtimeA)
    /// keeps failing closed (unregistered: nothing was ever installed),
    /// and no bundle runtime directory is created for the foreign
    /// configuration.
    #[cfg(feature = "experimental-public-send")]
    #[test]
    fn adopted_prepare_with_different_configuration_is_refused() {
        use crate::relay::DispatchWiring;
        use crate::runtime::error::RuntimeError;

        let temporary = TempDir::new().expect("temporary directory");
        let (roots, configuration_root, state_root) = test_roots(&temporary);
        let runtime = crate::relay::RelayRuntime::init(
            configuration_root,
            state_root.clone(),
            crate::relay::DeliveryConfiguration::default(),
        )
        .expect("runtime claims the domain");
        // A second, valid configuration with distinct memberships over
        // the SAME state root: the adopted prepare must refuse this
        // rather than host it on the adopted owner's delivery.
        let other_roots = other_roots_with_lone_bundle(
            &temporary,
            &roots,
            state_root.clone(),
        );
        match prepare_relay_host(&other_roots, true, None, None) {
            Ok(_) => {
                panic!("a foreign configuration must not prepare")
            },
            Err(error) => match error {
                RuntimeError::Validation { code, .. } => {
                    assert_eq!(
                        code, "validation_wiring_config_mismatch",
                        "the failure names the configuration binding: {code}"
                    );
                },
                error => {
                    panic!("binding refusal is validation: {error:?}")
                },
            },
        }
        assert!(
            !state_root.join("bundles").exists(),
            "a refused foreign prepare must not host bundles"
        );
        assert!(
            DispatchWiring::live(&runtime).is_err(),
            "live must never yield the foreign pair under the owner's brand"
        );
    }

    /// Concurrent domain claimants resolve to exactly one bound winner
    /// with no mixture: barrier-aligned runtime construction (confA)
    /// racing adopted preparation (confB, distinct memberships) over a
    /// fresh state root must end in exactly one consistent outcome.
    /// Either init wins the fresh claim and the prepare is refused for
    /// its foreign layers (live yields confA only, never the foreign
    /// pair), or prepare wins the fresh claim and serves confB while
    /// init is refused as already-hosted. Any other combination —
    /// including a live wiring for the foreign bundle under the
    /// winner's brand — fails the round. Binding precedes publication
    /// at every claim site, so no interleaving can install foreign
    /// configuration on an unbound owner.
    #[cfg(feature = "experimental-public-send")]
    #[test]
    fn concurrent_claimants_resolve_to_one_bound_winner_without_mixture() {
        use std::sync::{Arc as StdArc, Barrier};

        use crate::relay::DispatchWiring;
        use crate::runtime::error::RuntimeError;

        for _ in 0..20 {
            let temporary = TempDir::new().expect("temporary directory");
            let (roots, configuration_root, state_root) =
                test_roots(&temporary);
            let other_roots = other_roots_with_lone_bundle(
                &temporary,
                &roots,
                state_root.clone(),
            );
            let barrier = StdArc::new(Barrier::new(2));
            let barrier_a = StdArc::clone(&barrier);
            let configuration_root_a = configuration_root.clone();
            let state_root_a = state_root.clone();
            let init_handle = std::thread::spawn(move || {
                barrier_a.wait();
                crate::relay::RelayRuntime::init(
                    configuration_root_a,
                    state_root_a,
                    crate::relay::DeliveryConfiguration::default(),
                )
            });
            let barrier_b = StdArc::clone(&barrier);
            let prepare_handle = std::thread::spawn(move || {
                barrier_b.wait();
                prepare_relay_host(&other_roots, true, None, None)
            });
            let init_outcome = init_handle.join().expect("init thread joins");
            let prepare_outcome =
                prepare_handle.join().expect("prepare thread joins");
            match (init_outcome, prepare_outcome) {
                (Ok(runtime), Err(RuntimeError::Validation { code, .. })) => {
                    assert_eq!(
                        code, "validation_wiring_config_mismatch",
                        "the losing prepare names the binding"
                    );
                    if let Ok(wiring) = DispatchWiring::live(&runtime) {
                        let mut names: Vec<String> = wiring
                            .catalog()
                            .snapshot()
                            .iter()
                            .map(|paths| paths.bundle_name.clone())
                            .collect();
                        names.sort();
                        assert_eq!(
                            names,
                            vec!["party".to_string()],
                            "live never yields the foreign pair"
                        );
                    }
                },
                (Err(error), Ok(RelayHostPreparation::Serve(_))) => {
                    assert_eq!(
                        error.code, "validation_runtime_already_hosted",
                        "the losing init names the claim: {error:?}"
                    );
                },
                _ => {
                    panic!("exactly one claimant wins each round")
                },
            }
        }
    }
}
