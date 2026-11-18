//! Domain-bound dispatch wiring: the registry slot, the live/mirrored
//! constructors, the brand check, and the catalog-driven refusal rules.
//! These tests run under the experimental cutover feature, like the
//! surface they cover.

use std::sync::Arc;
use std::time::Duration;

use super::{provisioned_psk, send_fixture, verified_context};
use agentmux::relay::{
    BundleCatalog, ConnectionServeContext, DispatchWiring,
    PeerConnectionManager, RelayResponse, RelayRuntime, SendOperation,
    dispatch_send,
};
use agentmux::runtime::paths::BundleRuntimePaths;

/// A bundle file present on disk but carrying no sessions is not a
/// configured member: membership loading skips it, so a mirrored catalog
/// built against the real membership set must refuse targets naming it.
/// This pins the catalog as the resolution authority — file existence
/// alone never admits a bundle.
#[test]
fn unconfigured_bundle_is_refused_despite_file_presence() {
    let fixture = send_fixture("home");
    std::fs::write(
        fixture
            ._temporary
            .path()
            .join("conf")
            .join("bundles")
            .join("third.toml"),
        "format-version = 1\n",
    )
    .expect("write session-less bundle file");
    let wiring = DispatchWiring::mirrored(&fixture.runtime, &[], true)
        .expect("mirror domain wiring");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);
    let response = dispatch_send(
        &fixture.runtime,
        &context,
        &wiring,
        SendOperation {
            targets: vec!["alpha@third".to_string()],
            message: "hello".to_string(),
            broadcast: false,
            operation_bundle: None,
        },
    );
    let RelayResponse::Error { error } = response else {
        panic!("unconfigured bundle must fail, got {response:?}");
    };
    assert_eq!(
        error.code, "validation_unknown_target",
        "on-disk file without membership resolves nowhere: {error:?}"
    );
}

/// A bundle whose membership disables autostart enters the mirrored
/// catalog as held, and the shared spine refuses delivery to it with
/// the held-bundle error rather than spawning a worker behind the
/// operator's back.
#[test]
fn held_bundle_refuses_delivery() {
    let fixture = send_fixture("home");
    let wiring = DispatchWiring::mirrored(&fixture.runtime, &[], false)
        .expect("mirror domain wiring");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);
    let response = dispatch_send(
        &fixture.runtime,
        &context,
        &wiring,
        SendOperation {
            targets: vec!["bravo@party".to_string()],
            message: "hello".to_string(),
            broadcast: false,
            operation_bundle: None,
        },
    );
    let RelayResponse::Error { error } = response else {
        panic!("held bundle must fail, got {response:?}");
    };
    assert_eq!(
        error.code, "runtime_bundle_held",
        "held intent denies delivery: {error:?}"
    );
}

/// V2-1 counterexample, part one: a serve constructor handed a foreign
/// catalog on an already-owned domain keeps it locally (adoption does not
/// install), so reading the domain wiring must fail closed — never yield
/// the foreign pair.
#[test]
fn foreign_serve_pair_cannot_be_wrapped() {
    let fixture = send_fixture("home");
    let state_root = fixture.runtime.state_root().to_path_buf();
    let catalog_b = BundleCatalog::from_paths([BundleRuntimePaths::resolve(
        &state_root,
        "zzz",
    )
    .expect("bundle paths")]);
    let peers_b =
        Arc::new(PeerConnectionManager::from_configuration(&state_root, &[]));
    let _foreign_serve = ConnectionServeContext::new(
        fixture.runtime.configuration_roots().clone(),
        state_root.clone(),
        catalog_b,
        peers_b,
        Vec::new(),
        false,
        Duration::from_secs(2),
        Arc::clone(fixture.runtime.delivery_context()),
    )
    .expect("adopted serve keeps its caller pair locally");
    let error = DispatchWiring::live(&fixture.runtime)
        .expect_err("unregistered domain must fail closed");
    assert_eq!(
        error.code, "validation_wiring_unregistered",
        "no pair was ever installed: {error:?}"
    );
}

/// V2-1 counterexample, part two: once the host pair is installed, a
/// later foreign serve constructor cannot replace it — the registered
/// pair still resolves, observed through its shared catalog identity.
#[test]
fn registered_pair_survives_foreign_serve() {
    let fixture = send_fixture("home");
    let wiring = DispatchWiring::mirrored(&fixture.runtime, &[], true)
        .expect("install host pair");
    let state_root = fixture.runtime.state_root().to_path_buf();
    let catalog_c = BundleCatalog::from_paths([BundleRuntimePaths::resolve(
        &state_root,
        "zzz",
    )
    .expect("bundle paths")]);
    let _foreign_serve = ConnectionServeContext::new(
        fixture.runtime.configuration_roots().clone(),
        state_root,
        catalog_c,
        Arc::new(PeerConnectionManager::from_configuration(
            fixture.runtime.state_root(),
            &[],
        )),
        Vec::new(),
        false,
        Duration::from_secs(2),
        Arc::clone(fixture.runtime.delivery_context()),
    )
    .expect("adopted serve keeps its caller pair locally");
    let live = DispatchWiring::live(&fixture.runtime)
        .expect("installed pair reads back");
    let mut names: Vec<String> = live
        .catalog()
        .snapshot()
        .iter()
        .map(|paths| paths.bundle_name.clone())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["other".to_string(), "party".to_string()],
        "registered pair is unchanged by the foreign serve"
    );
    let _ = wiring;
}

/// A wiring built for one runtime is refused by another: the brand check
/// compares the wiring's delivery against the dispatching runtime's, so
/// cross-domain reuse fails closed instead of delivering under foreign
/// quota and authorization.
#[test]
fn cross_runtime_wiring_is_refused() {
    let fixture = send_fixture("home");
    let other = send_fixture("home");
    let other_wiring = DispatchWiring::mirrored(&other.runtime, &[], true)
        .expect("mirror other domain wiring");
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);
    let response = dispatch_send(
        &fixture.runtime,
        &context,
        &other_wiring,
        SendOperation {
            targets: vec!["bravo@party".to_string()],
            message: "hello".to_string(),
            broadcast: false,
            operation_bundle: None,
        },
    );
    let RelayResponse::Error { error } = response else {
        panic!("foreign wiring must fail, got {response:?}");
    };
    assert_eq!(
        error.code, "validation_wiring_runtime_mismatch",
        "brand check fails closed: {error:?}"
    );
}
/// A fully valid bundle file that membership loading never saw is still
/// refused when it is absent from the installed catalog: the alpha
/// session exists on disk with valid configuration, but the wiring was
/// mirrored before the file arrived, so resolution fails at catalog
/// lookup — never at member resolution — proving the installed catalog,
/// not the filesystem, is the authority.
#[test]
fn valid_bundle_outside_installed_catalog_is_refused() {
    let fixture = send_fixture("home");
    let wiring = DispatchWiring::mirrored(&fixture.runtime, &[], true)
        .expect("mirror domain wiring");
    // Positive control first: the installed catalog serves its members.
    let psk = provisioned_psk(&fixture.runtime, "alpha@party");
    let context = verified_context(&fixture.runtime, "alpha@party", &psk);
    let response = dispatch_send(
        &fixture.runtime,
        &context,
        &wiring,
        SendOperation {
            targets: vec!["bravo@party".to_string()],
            message: "hello".to_string(),
            broadcast: false,
            operation_bundle: None,
        },
    );
    assert!(
        matches!(response, RelayResponse::Send { .. }),
        "installed catalog serves its members: {response:?}"
    );
    // A new, fully valid bundle file lands after installation.
    std::fs::write(
        fixture
            ._temporary
            .path()
            .join("conf")
            .join("bundles")
            .join("third.toml"),
        r#"format-version = 1
autostart = true

[[sessions]]
id = "alpha"
name = "Alpha"
directory = "/tmp"
coder = "shell"
"#,
    )
    .expect("write valid bundle file after install");
    let mut names: Vec<String> = wiring
        .catalog()
        .snapshot()
        .iter()
        .map(|paths| paths.bundle_name.clone())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["other".to_string(), "party".to_string()],
        "the installed catalog does not see the late file"
    );
    let refused = dispatch_send(
        &fixture.runtime,
        &context,
        &wiring,
        SendOperation {
            targets: vec!["alpha@third".to_string()],
            message: "hello".to_string(),
            broadcast: false,
            operation_bundle: None,
        },
    );
    let RelayResponse::Error { error } = refused else {
        panic!("uncatalogued bundle must fail, got {refused:?}");
    };
    assert_eq!(
        error.code, "validation_unknown_target",
        "absence from the catalog refuses before any member check: {error:?}"
    );
}

/// Dropping a domain releases its pair with it: re-initializing the same
/// root starts from an empty slot, a fresh mirrored install succeeds, and
/// dispatch works. No stale registration survives the generation.
#[test]
fn dropped_domain_releases_pair_for_clean_reclaim() {
    use super::SendFixture;
    use agentmux::relay::DeliveryConfiguration;

    let SendFixture {
        _temporary: _keep_alive,
        runtime,
    } = send_fixture("home");
    let state_root = runtime.state_root().to_path_buf();
    let configuration_root = _keep_alive.path().join("conf").to_path_buf();
    let wiring =
        DispatchWiring::mirrored(&runtime, &[], true).expect("install pair");
    drop(wiring);
    drop(runtime);
    let runtime = RelayRuntime::init(
        configuration_root,
        state_root,
        DeliveryConfiguration::default(),
    )
    .expect("reclaim after full drop succeeds");
    let rewired = DispatchWiring::mirrored(&runtime, &[], true)
        .expect("fresh generation installs cleanly");
    let psk = provisioned_psk(&runtime, "alpha@party");
    let context = verified_context(&runtime, "alpha@party", &psk);
    let response = dispatch_send(
        &runtime,
        &context,
        &rewired,
        SendOperation {
            targets: vec!["bravo@party".to_string()],
            message: "hello".to_string(),
            broadcast: false,
            operation_bundle: None,
        },
    );
    assert!(
        matches!(response, RelayResponse::Send { .. }),
        "reclaimed domain dispatches: {response:?}"
    );
}

/// A held delivery keeps its domain alive: re-initializing the same root
/// while an old handle still lives is refused, and only after the last
/// holder drops does reclaiming succeed with a fresh pair.
#[test]
fn held_delivery_blocks_reclaim_until_release() {
    use super::SendFixture;
    use agentmux::relay::DeliveryConfiguration;

    let SendFixture {
        _temporary: _keep_alive,
        runtime,
    } = send_fixture("home");
    let state_root = runtime.state_root().to_path_buf();
    let configuration_root = _keep_alive.path().join("conf").to_path_buf();
    let held = std::sync::Arc::clone(runtime.delivery_context());
    let wiring =
        DispatchWiring::mirrored(&runtime, &[], true).expect("install pair");
    drop(wiring);
    drop(runtime);
    assert!(
        RelayRuntime::init(
            configuration_root.clone(),
            state_root.clone(),
            DeliveryConfiguration::default(),
        )
        .is_err(),
        "a live held delivery keeps the domain owned"
    );
    drop(held);
    let runtime = RelayRuntime::init(
        configuration_root,
        state_root,
        DeliveryConfiguration::default(),
    )
    .expect("release clears the domain for reclaim");
    let rewired = DispatchWiring::mirrored(&runtime, &[], true)
        .expect("fresh generation installs cleanly");
    let psk = provisioned_psk(&runtime, "alpha@party");
    let context = verified_context(&runtime, "alpha@party", &psk);
    let response = dispatch_send(
        &runtime,
        &context,
        &rewired,
        SendOperation {
            targets: vec!["bravo@party".to_string()],
            message: "hello".to_string(),
            broadcast: false,
            operation_bundle: None,
        },
    );
    assert!(
        matches!(response, RelayResponse::Send { .. }),
        "reclaimed domain dispatches: {response:?}"
    );
}

/// Concurrent mirrored calls on one domain converge: every caller gets a
/// working wiring over the same installed pair, never a conflict and
/// never divergent content. A barrier aligns thread starts so the calls
/// overlap in practice; this is a concurrent exercise rather than a
/// deterministic install-race proof (threads may still serialize before
/// the empty-slot check). Shared identity is covered in-crate
/// (`concurrent_mirrored_wirings_share_catalog_identity`: shared catalog
/// map plus a mutation visible through every returned wiring). Here,
/// identity is pinned by shared peer Arc and identical catalog content,
/// and every returned wiring — not a freshly acquired one — is
/// exercised through dispatch.
#[test]
fn concurrent_mirrored_calls_converge_on_one_pair() {
    use std::sync::{Arc as StdArc, Barrier};

    let fixture = send_fixture("home");
    let runtime = StdArc::new(fixture.runtime);
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
    let mut wirings = Vec::new();
    for handle in handles {
        wirings.push(
            handle
                .join()
                .expect("mirrored thread joins")
                .expect("concurrent mirrored install converges"),
        );
    }
    let mut namesets = Vec::new();
    for wiring in &wirings {
        let mut names: Vec<String> = wiring
            .catalog()
            .snapshot()
            .iter()
            .map(|paths| paths.bundle_name.clone())
            .collect();
        names.sort();
        namesets.push(names);
    }
    assert!(
        namesets.windows(2).all(|pair| pair[0] == pair[1]),
        "all concurrent wirings resolve the identical catalog"
    );
    for pair in wirings.windows(2) {
        assert!(
            StdArc::ptr_eq(pair[0].peers(), pair[1].peers()),
            "concurrent wirings share the installed peer manager"
        );
    }
    let psk = provisioned_psk(&runtime, "alpha@party");
    let context = verified_context(&runtime, "alpha@party", &psk);
    for wiring in &wirings {
        let response = dispatch_send(
            &runtime,
            &context,
            wiring,
            SendOperation {
                targets: vec!["bravo@party".to_string()],
                message: "hello".to_string(),
                broadcast: false,
                operation_bundle: None,
            },
        );
        assert!(
            matches!(response, RelayResponse::Send { .. }),
            "every converged wiring dispatches: {response:?}"
        );
    }
}

/// A `Hold` pair is not usable by concurrent autostart callers: the
/// first install wins, so later `mirrored` calls with `autostart = true`
/// adopt the `Hold` pair and dispatch is refused with
/// `runtime_bundle_held` until a `Run` pair replaces it. This pins the
/// Hold-refuses-dispatch mechanism at the public boundary using only
/// the public constructors. (The unpublished prepare window is covered
/// separately: tentative pairs refuse reads entirely, so no wiring is
/// issuable before the commit boundary.)
#[test]
fn hold_installed_pair_refuses_concurrent_autostart_dispatch() {
    use std::sync::{Arc as StdArc, Barrier};

    let fixture = send_fixture("home");
    // The tentative install a paused prepare would publish: everything
    // `Hold`, before any bundle has started.
    let _tentative = DispatchWiring::mirrored(&fixture.runtime, &[], false)
        .expect("tentative Hold install");
    let runtime = StdArc::new(fixture.runtime);
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
    let psk = provisioned_psk(&runtime, "alpha@party");
    let context = verified_context(&runtime, "alpha@party", &psk);
    for handle in handles {
        let wiring = handle
            .join()
            .expect("mirrored thread joins")
            .expect("autostart callers adopt the installed Hold pair");
        let response = dispatch_send(
            &runtime,
            &context,
            &wiring,
            SendOperation {
                targets: vec!["bravo@party".to_string()],
                message: "hello".to_string(),
                broadcast: false,
                operation_bundle: None,
            },
        );
        let RelayResponse::Error { error } = response else {
            panic!("tentative Hold entry must refuse dispatch: {response:?}");
        };
        assert_eq!(
            error.code, "runtime_bundle_held",
            "tentative entries refuse until promotion: {error:?}"
        );
    }
}
