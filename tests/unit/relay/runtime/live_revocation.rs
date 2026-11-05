//! Live revocation through the public runtime boundary.
//!
//! Dropping a principal tears down its live streams and notifies watching
//! hosts through the owning runtime's delivery state — observable on the
//! wire, not just in the store. A pre-publication failure, by contrast,
//! leaves live sessions untouched. Both directions are asserted through real
//! stream connections served on the runtime's own delivery context.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use agentmux::relay::test_hooks::{
    ConfinedCommitHook, arm_confined_commit_hook, disarm_confined_commit_hook,
};
use agentmux::relay::{
    BundleCatalog, ConnectionDrainCoordinator, ConnectionServeContext,
    CredentialDestination, CredentialSource, DeliveryConfiguration,
    DeliveryContext, IdentityDescriptor, PeerConnectionManager, RelayRuntime,
    drop_principal, introspect_principal, provision_principal,
    rotate_principal_credential, verify_identity,
};
use agentmux::runtime::paths::BundleRuntimePaths;
use serde_json::{Value, json};
use tempfile::TempDir;

use super::test_runtime;

/// Writes a bundle with a single UI session: no tmux server needed for a
/// connection to register against it.
fn write_ui_bundle(temporary: &TempDir, bundle_name: &str) {
    let conf = temporary.path().join("conf");
    let bundles = conf.join("bundles");
    std::fs::create_dir_all(&bundles).expect("create bundles directory");
    std::fs::write(
        conf.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "shell"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders file");
    std::fs::write(
        conf.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "home"
look = "self"
send = "home"
"#,
    )
    .expect("write policies file");
    std::fs::write(
        bundles.join(format!("{bundle_name}.toml")),
        r#"
format-version = 1

[[sessions]]
id = "victim"
name = "Victim"
directory = "/tmp"

[sessions.ui]
"#,
    )
    .expect("write bundle configuration");
}

/// Serves one stream connection against the given delivery state. Reads/writes
/// happen on the returned client; the server half runs on a background thread
/// until the test's TempDir (and runtime) is dropped. Serve construction
/// over a hosted root adopts the owning delivery state, so passing a foreign
/// state exercises the adoption path, not a second domain.
fn serve_with(
    delivery: &Arc<agentmux::relay::DeliveryContext>,
    configuration_roots: &agentmux::configuration::ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
) -> (UnixStream, std::thread::JoinHandle<()>) {
    let (server_stream, client_stream) =
        UnixStream::pair().expect("unix stream pair");
    server_stream
        .set_nonblocking(true)
        .expect("non-blocking server stream");
    let context = Arc::new(ConnectionServeContext::new(
        configuration_roots.clone(),
        bundle_paths.state_root.clone(),
        BundleCatalog::from_paths([bundle_paths.clone()]),
        Arc::new(PeerConnectionManager::from_configuration(
            &bundle_paths.state_root,
            &[],
        )),
        Vec::new(),
        false,
        Duration::from_secs(2),
        Arc::clone(delivery),
    ));
    let coordinator = ConnectionDrainCoordinator::new();
    let slot = coordinator.register_worker();
    // Leak the coordinator: dropping its shutdown sender would read as a
    // shutdown signal to the worker, closing the connection immediately.
    std::mem::forget(coordinator);
    let handle = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build test runtime");
        runtime.block_on(async move {
            let stream = tokio::net::UnixStream::from_std(server_stream)
                .expect("async stream");
            agentmux::relay::serve_connection(stream, &context, slot)
                .await
                .ok()
        });
    });
    (client_stream, handle)
}

/// Serves one stream connection against the runtime's own delivery state.
fn serve_on(
    runtime: &RelayRuntime,
    configuration_roots: &agentmux::configuration::ConfigurationRoots,
    bundle_paths: &BundleRuntimePaths,
) -> (UnixStream, std::thread::JoinHandle<()>) {
    serve_with(
        runtime.delivery_context(),
        configuration_roots,
        bundle_paths,
    )
}

/// Sends one JSON frame.
fn send_json(client: &mut UnixStream, payload: Value) {
    let encoded = serde_json::to_string(&payload).expect("encode frame");
    client
        .write_all(format!("{encoded}\n").as_bytes())
        .expect("write frame");
    client.flush().expect("flush frame");
}

/// Reads one JSON frame.
fn read_json(reader: &mut BufReader<UnixStream>) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).expect("read frame");
    serde_json::from_str(line.trim()).expect("decode frame")
}

/// Hellos as `principal_id` with `token` and returns the reader once the
/// ack lands.
fn hello(
    client: &mut UnixStream,
    principal_id: &str,
    token: &str,
) -> BufReader<UnixStream> {
    send_json(
        client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": principal_id,
            "identity_token": token,
        }),
    );
    let mut reader = BufReader::new(client.try_clone().expect("clone stream"));
    let ack = read_json(&mut reader);
    assert_eq!(
        ack["frame"], "hello_ack",
        "hello not acked for {principal_id}: {ack}"
    );
    reader
}

/// Discards hello-triggered frames (identity snapshots, registration
/// choices) still buffered after [`hello`], so a later silence assertion
/// observes only post-hello effects. Read quiescence ends the drain; a
/// revocation frame or a close during drain fails loudly.
fn drain_pending(reader: &mut BufReader<UnixStream>, principal_id: &str) {
    use std::io::ErrorKind;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Err(error)
                if error.kind() == ErrorKind::WouldBlock
                    || error.kind() == ErrorKind::TimedOut =>
            {
                return;
            },
            Err(error) => panic!(
                "{principal_id} must stay connected while draining, got error: {error:?}"
            ),
            Ok(0) => panic!("{principal_id} must not close while draining"),
            Ok(_) => {
                let frame: Value =
                    serde_json::from_str(line.trim()).expect("decode frame");
                assert_ne!(
                    frame["response"]["error"]["code"],
                    "runtime_identity_revoked",
                    "{principal_id} revoked before the test acted: {frame}"
                );
            },
        }
    }
}

/// Reads frames until `predicate` matches, or panics after the budget.
fn await_frame(
    reader: &mut BufReader<UnixStream>,
    what: &str,
    budget: Duration,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + budget;
    loop {
        let frame = read_json(reader);
        if predicate(&frame) {
            return frame;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: last frame {frame}"
        );
    }
}

/// A post-publication drop tears down the victim's live session and notifies
/// a watching host: both effects are observable on the wire.
#[test]
fn dropping_a_principal_revokes_its_live_session_and_notifies_watchers() {
    let (temporary, runtime) = test_runtime();
    let bundle_name = "party";
    write_ui_bundle(&temporary, bundle_name);
    let configuration_roots =
        agentmux::configuration::ConfigurationRoots::single(
            temporary.path().join("conf"),
        );
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name)
        .expect("bundle paths");

    let victim_psk = provision_principal(
        &runtime,
        agentmux::relay::ProvisionParams::new("victim@party".to_string()),
    )
    .expect("provision victim")
    .psk
    .expect("one-time PSK");
    let mut watcher_params =
        agentmux::relay::ProvisionParams::new("watcher@EXTERNAL".to_string());
    watcher_params.scope = Some(bundle_name.to_string());
    let watcher_psk = provision_principal(&runtime, watcher_params)
        .expect("provision watching host")
        .psk
        .expect("one-time PSK");

    let (mut victim_client, victim_handle) =
        serve_on(&runtime, &configuration_roots, &bundle_paths);
    let mut victim_reader =
        hello(&mut victim_client, "victim@party", &victim_psk);
    let (mut watcher_client, watcher_handle) =
        serve_on(&runtime, &configuration_roots, &bundle_paths);
    let mut watcher_reader =
        hello(&mut watcher_client, "watcher@EXTERNAL", &watcher_psk);
    victim_client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    watcher_client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");

    drop_principal(&runtime, "victim@party").expect("drop victim");

    let revoked = await_frame(
        &mut victim_reader,
        "revocation frame",
        Duration::from_secs(5),
        |frame| {
            frame["frame"] == "response"
                && frame["response"]["error"]["code"]
                    == "runtime_identity_revoked"
        },
    );
    assert_eq!(
        revoked["response"]["error"]["details"]["principal_id"],
        "victim@party",
        "revocation names the dropped principal: {revoked}"
    );
    let mut trailing = String::new();
    assert_eq!(
        victim_reader
            .read_line(&mut trailing)
            .expect("read after revoke"),
        0,
        "victim connection closes after revocation: {trailing:?}"
    );

    let event = await_frame(
        &mut watcher_reader,
        "identity.revoked event",
        Duration::from_secs(5),
        |frame| {
            frame["frame"] == "event"
                && frame["event"]["event_type"] == "identity.revoked"
        },
    );
    assert_eq!(
        event["event"]["payload"]["principal_id"], "victim@party",
        "watcher is told which principal was revoked: {event}"
    );
    assert_eq!(
        event["event"]["target_session"], "watcher@EXTERNAL",
        "event is addressed to the watching host: {event}"
    );

    victim_client.shutdown(std::net::Shutdown::Both).ok();
    watcher_client.shutdown(std::net::Shutdown::Both).ok();
    victim_handle.join().expect("join victim connection");
    watcher_handle.join().expect("join watcher connection");
}

/// A pre-publication drop failure leaves live sessions untouched: no
/// revocation frame, no close, no broadcast to watching hosts, and the
/// binding still verifies.
#[test]
fn pre_publication_drop_failure_leaves_live_sessions_untouched() {
    let (temporary, runtime) = test_runtime();
    let bundle_name = "party";
    write_ui_bundle(&temporary, bundle_name);
    let configuration_roots =
        agentmux::configuration::ConfigurationRoots::single(
            temporary.path().join("conf"),
        );
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name)
        .expect("bundle paths");

    let victim_psk = provision_principal(
        &runtime,
        agentmux::relay::ProvisionParams::new("victim@party".to_string()),
    )
    .expect("provision victim")
    .psk
    .expect("one-time PSK");
    let mut watcher_params =
        agentmux::relay::ProvisionParams::new("watcher@EXTERNAL".to_string());
    watcher_params.scope = Some(bundle_name.to_string());
    let watcher_psk = provision_principal(&runtime, watcher_params)
        .expect("provision watching host")
        .psk
        .expect("one-time PSK");
    let (mut victim_client, victim_handle) =
        serve_on(&runtime, &configuration_roots, &bundle_paths);
    let mut victim_reader =
        hello(&mut victim_client, "victim@party", &victim_psk);
    let (mut watcher_client, watcher_handle) =
        serve_on(&runtime, &configuration_roots, &bundle_paths);
    let mut watcher_reader =
        hello(&mut watcher_client, "watcher@EXTERNAL", &watcher_psk);
    victim_client
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("read timeout");
    watcher_client
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("read timeout");
    drain_pending(&mut victim_reader, "victim@party");
    drain_pending(&mut watcher_reader, "watcher@EXTERNAL");

    let identity_dir = state_root.join("identity");
    std::fs::create_dir_all(&identity_dir).expect("identity directory");
    std::fs::write(identity_dir.join(".fault-pre-rename"), b"")
        .expect("plant fault seam");
    let failed = drop_principal(&runtime, "victim@party")
        .expect_err("pre-rename fault must fail the drop");
    assert_ne!(
        failed.code, "internal_store_durability_uncertain",
        "a pre-publication failure is not uncertainty"
    );
    std::fs::remove_file(identity_dir.join(".fault-pre-rename")).ok();

    let mut line = String::new();
    match victim_reader.read_line(&mut line) {
        Err(error)
            if error.kind() == std::io::ErrorKind::WouldBlock
                || error.kind() == std::io::ErrorKind::TimedOut =>
        {
            // Still connected and silent: no revocation frame arrived.
        },
        Err(error) => {
            panic!("live session must stay connected, got error: {error:?}")
        },
        Ok(0) => panic!("live session must not close on failed drop"),
        Ok(_) => panic!("no frame may arrive on a failed drop, got: {line:?}"),
    }
    // A failed drop notifies nobody: the watching host hears nothing.
    let mut watcher_line = String::new();
    match watcher_reader.read_line(&mut watcher_line) {
        Err(error)
            if error.kind() == std::io::ErrorKind::WouldBlock
                || error.kind() == std::io::ErrorKind::TimedOut =>
        {
            // Silent: no revocation is broadcast on a failed drop.
        },
        Err(error) => {
            panic!("watcher must stay connected, got error: {error:?}")
        },
        Ok(0) => panic!("watcher must not close on failed drop"),
        Ok(_) => panic!(
            "no event may arrive on a failed drop, got: {watcher_line:?}"
        ),
    }
    verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "victim@party".to_string(),
            CredentialSource::DirectPsk(victim_psk),
        ),
    )
    .expect("binding intact after failed drop");

    victim_client.shutdown(std::net::Shutdown::Both).ok();
    watcher_client.shutdown(std::net::Shutdown::Both).ok();
    victim_handle.join().expect("join victim connection");
    watcher_handle.join().expect("join watcher connection");
}

/// A post-publication drop failure still revokes: the removal is effective,
/// so the victim's live session is torn down and watching hosts are
/// notified — only the reported outcome is the durability uncertainty
/// rather than success.
#[test]
fn post_publication_drop_uncertainty_still_revokes_live_sessions() {
    let (temporary, runtime) = test_runtime();
    let bundle_name = "party";
    write_ui_bundle(&temporary, bundle_name);
    let configuration_roots =
        agentmux::configuration::ConfigurationRoots::single(
            temporary.path().join("conf"),
        );
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name)
        .expect("bundle paths");

    let victim_psk = provision_principal(
        &runtime,
        agentmux::relay::ProvisionParams::new("victim@party".to_string()),
    )
    .expect("provision victim")
    .psk
    .expect("one-time PSK");
    let mut watcher_params =
        agentmux::relay::ProvisionParams::new("watcher@EXTERNAL".to_string());
    watcher_params.scope = Some(bundle_name.to_string());
    let watcher_psk = provision_principal(&runtime, watcher_params)
        .expect("provision watching host")
        .psk
        .expect("one-time PSK");
    let (mut victim_client, victim_handle) =
        serve_on(&runtime, &configuration_roots, &bundle_paths);
    let mut victim_reader =
        hello(&mut victim_client, "victim@party", &victim_psk);
    let (mut watcher_client, watcher_handle) =
        serve_on(&runtime, &configuration_roots, &bundle_paths);
    let mut watcher_reader =
        hello(&mut watcher_client, "watcher@EXTERNAL", &watcher_psk);
    victim_client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    watcher_client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");

    let identity_dir = state_root.join("identity");
    std::fs::create_dir_all(&identity_dir).expect("identity directory");
    std::fs::write(identity_dir.join(".fault-dir-sync"), b"")
        .expect("plant fault seam");
    let failed = drop_principal(&runtime, "victim@party")
        .expect_err("post-rename fault must surface uncertainty");
    assert_eq!(
        failed.code, "internal_store_durability_uncertain",
        "publication happened: uncertainty, not silent success: {failed:?}"
    );
    std::fs::remove_file(identity_dir.join(".fault-dir-sync")).ok();

    let revoked = await_frame(
        &mut victim_reader,
        "revocation frame",
        Duration::from_secs(5),
        |frame| {
            frame["frame"] == "response"
                && frame["response"]["error"]["code"]
                    == "runtime_identity_revoked"
        },
    );
    assert_eq!(
        revoked["response"]["error"]["details"]["principal_id"],
        "victim@party",
        "revocation names the dropped principal: {revoked}"
    );
    let mut trailing = String::new();
    assert_eq!(
        victim_reader
            .read_line(&mut trailing)
            .expect("read after revoke"),
        0,
        "victim connection closes after revocation: {trailing:?}"
    );
    let event = await_frame(
        &mut watcher_reader,
        "identity.revoked event",
        Duration::from_secs(5),
        |frame| {
            frame["frame"] == "event"
                && frame["event"]["event_type"] == "identity.revoked"
        },
    );
    assert_eq!(
        event["event"]["payload"]["principal_id"], "victim@party",
        "watcher is told which principal was revoked: {event}"
    );
    assert!(
        introspect_principal(&runtime, "victim@party")
            .expect("introspect")
            .is_none(),
        "the published removal is effective despite the uncertainty"
    );

    victim_client.shutdown(std::net::Shutdown::Both).ok();
    watcher_client.shutdown(std::net::Shutdown::Both).ok();
    victim_handle.join().expect("join victim connection");
    watcher_handle.join().expect("join watcher connection");
}

/// Operator policy preset plus a TUI session binding the operator id to it,
/// so a `socket-trust` hello over a session-credential-unenforced serve
/// context can issue relay-admin requests.
fn write_operator_configuration(temporary: &TempDir, operator_id: &str) {
    let conf = temporary.path().join("conf");
    std::fs::write(
        conf.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "home"
look = "self"
send = "home"

[[policies]]
id = "operator"

[policies.controls]
list = "home"
look = "home"
send = "home"

[policies.controls.new]
peer = "all"

[policies.controls.change]
psk = "all"

[policies.controls.drop]
peer = "all"
"#,
    )
    .expect("write operator policies configuration");
    std::fs::write(
        conf.join("users.toml"),
        format!(
            r#"
default-session = "{operator_id}"

[[sessions]]
id = "{operator_id}"
policy = "operator"

[sessions.ui]
"#
        ),
    )
    .expect("write users configuration");
}

/// Submits one relay-admin request and returns its response frame, skipping
/// any interleaved events or snapshots.
fn request_admin(
    client: &mut UnixStream,
    reader: &mut BufReader<UnixStream>,
    request: Value,
) -> Value {
    send_json(
        client,
        json!({
            "frame": "request",
            "request_id": "admin-1",
            "request": request,
        }),
    );
    loop {
        let frame = read_json(reader);
        if frame["frame"] == "response" {
            return frame;
        }
    }
}

/// A drop issued over a socket served on a foreign delivery state still tears
/// down the owning runtime's live sessions: serve construction over a hosted
/// root adopts the owning domain's delivery state, so the admin connection,
/// the victim, and the watching host all register in the one domain and the
/// revocation lands where they live.
#[test]
fn socket_admin_routes_through_the_owning_runtime_domain() {
    let (temporary, runtime) = test_runtime();
    let bundle_name = "party";
    write_ui_bundle(&temporary, bundle_name);
    let operator_id = "boundary-op@GLOBAL";
    write_operator_configuration(&temporary, operator_id);
    let configuration_roots =
        agentmux::configuration::ConfigurationRoots::single(
            temporary.path().join("conf"),
        );
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name)
        .expect("bundle paths");

    let victim_psk = provision_principal(
        &runtime,
        agentmux::relay::ProvisionParams::new("victim@party".to_string()),
    )
    .expect("provision victim")
    .psk
    .expect("one-time PSK");
    let (mut victim_client, victim_handle) =
        serve_on(&runtime, &configuration_roots, &bundle_paths);
    let mut victim_reader =
        hello(&mut victim_client, "victim@party", &victim_psk);

    // Same roots, but a fresh delivery state: serve construction over the
    // hosted root adopts the owning domain, so the admin connection
    // registers where the victim lives. Without adoption the revocation
    // below would land in a foreign registry and leave the victim untouched.
    let foreign_delivery =
        Arc::new(DeliveryContext::new(DeliveryConfiguration::default()));
    let (mut operator_client, operator_handle) =
        serve_with(&foreign_delivery, &configuration_roots, &bundle_paths);
    let mut operator_reader =
        hello(&mut operator_client, operator_id, "socket-trust");
    victim_client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");

    let response = request_admin(
        &mut operator_client,
        &mut operator_reader,
        json!({"operation": "drop_peer", "principal_id": "victim@party"}),
    );
    assert_eq!(
        response["response"]["kind"], "drop_peer",
        "drop rejected: {response:?}"
    );

    let revoked = await_frame(
        &mut victim_reader,
        "revocation frame",
        Duration::from_secs(5),
        |frame| {
            frame["frame"] == "response"
                && frame["response"]["error"]["code"]
                    == "runtime_identity_revoked"
        },
    );
    assert_eq!(
        revoked["response"]["error"]["details"]["principal_id"],
        "victim@party",
        "revocation names the dropped principal: {revoked}"
    );
    let mut trailing = String::new();
    assert_eq!(
        victim_reader
            .read_line(&mut trailing)
            .expect("read after revoke"),
        0,
        "victim connection closes after revocation: {trailing:?}"
    );

    victim_client.shutdown(std::net::Shutdown::Both).ok();
    operator_client.shutdown(std::net::Shutdown::Both).ok();
    victim_handle.join().expect("join victim connection");
    operator_handle.join().expect("join operator connection");
}

/// Guard disarming the global confined-commit hook when the test ends, so a
/// sabotage armed for one test never leaks into parallel tests.
struct DisarmGuard;

impl Drop for DisarmGuard {
    fn drop(&mut self) {
        disarm_confined_commit_hook();
    }
}

/// How the compensating rollback persist is broken: either before its own
/// publication (the rotated hash stands effective) or after it (the
/// restored prior record stands effective but crash-durability is
/// uncertain).
enum RollbackSabotage {
    /// Deletes the rollback's staged store temp inside the commit, so the
    /// publish rename fails before publication.
    DeleteStagedStoreTemp,
    /// Plants the dir-sync fault seam inside the commit, so the publish
    /// rename succeeds and only the durability sync fails.
    PlantDirSyncSeam,
}

/// Arms a one-shot sabotage of the calling thread's second confined commit:
/// the first commit (the rotated store persist) passes through, while the
/// second (the compensating rollback persist) is broken the requested way.
/// Thread-gating keeps parallel tests' commits from consuming the count.
fn arm_rollback_sabotage(
    identity_dir: std::path::PathBuf,
    sabotage: RollbackSabotage,
) -> DisarmGuard {
    let owner = std::thread::current().id();
    let fires = Arc::new(AtomicUsize::new(0));
    arm_confined_commit_hook(ConfinedCommitHook::arm(move || {
        if std::thread::current().id() != owner {
            return;
        }
        if fires.fetch_add(1, Ordering::SeqCst) != 1 {
            return;
        }
        match sabotage {
            RollbackSabotage::DeleteStagedStoreTemp => {
                let entries =
                    std::fs::read_dir(&identity_dir).expect("read identity");
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if name.starts_with("principals.json.")
                        && name.ends_with(".store.tmp")
                    {
                        std::fs::remove_file(entry.path()).ok();
                    }
                }
            },
            RollbackSabotage::PlantDirSyncSeam => {
                std::fs::write(identity_dir.join(".fault-dir-sync"), b"")
                    .expect("plant dir-sync seam");
            },
        }
    }));
    DisarmGuard
}

/// A rotation whose credential publish fails and whose compensating rollback
/// persist fails before publication leaves the rotated hash effective: live
/// sessions holding the superseded credential are torn down, watching hosts
/// are notified, and the double fault (not the single write failure) is
/// reported.
#[test]
fn rotate_commit_failure_with_failed_rollback_revokes_live_sessions() {
    let (temporary, runtime) = test_runtime();
    let bundle_name = "party";
    write_ui_bundle(&temporary, bundle_name);
    let configuration_roots =
        agentmux::configuration::ConfigurationRoots::single(
            temporary.path().join("conf"),
        );
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name)
        .expect("bundle paths");

    let victim_psk = provision_principal(
        &runtime,
        agentmux::relay::ProvisionParams::new("victim@party".to_string()),
    )
    .expect("provision victim")
    .psk
    .expect("one-time PSK");
    let mut watcher_params =
        agentmux::relay::ProvisionParams::new("watcher@EXTERNAL".to_string());
    watcher_params.scope = Some(bundle_name.to_string());
    let watcher_psk = provision_principal(&runtime, watcher_params)
        .expect("provision watching host")
        .psk
        .expect("one-time PSK");
    let (mut victim_client, victim_handle) =
        serve_on(&runtime, &configuration_roots, &bundle_paths);
    let mut victim_reader =
        hello(&mut victim_client, "victim@party", &victim_psk);
    let (mut watcher_client, watcher_handle) =
        serve_on(&runtime, &configuration_roots, &bundle_paths);
    let mut watcher_reader =
        hello(&mut watcher_client, "watcher@EXTERNAL", &watcher_psk);
    victim_client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    watcher_client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");

    // A directory as the credential target fails the publish rename after
    // the rotated store persist already committed; deleting the rollback's
    // staged temp then fails the compensation before its own publication.
    let sink_dir = temporary.path().join("sink");
    std::fs::create_dir_all(&sink_dir).expect("sink directory");
    let _guard = arm_rollback_sabotage(
        state_root.join("identity"),
        RollbackSabotage::DeleteStagedStoreTemp,
    );
    let failed = rotate_principal_credential(
        &runtime,
        "victim@party",
        CredentialDestination::Path {
            path: sink_dir.display().to_string(),
        },
    )
    .expect_err("double fault must surface");
    assert_eq!(
        failed.code, "internal_credential_rollback_failed",
        "the failed compensation reports the double fault: {failed:?}"
    );

    let revoked = await_frame(
        &mut victim_reader,
        "revocation frame",
        Duration::from_secs(5),
        |frame| {
            frame["frame"] == "response"
                && frame["response"]["error"]["code"]
                    == "runtime_identity_revoked"
        },
    );
    assert_eq!(
        revoked["response"]["error"]["details"]["principal_id"],
        "victim@party",
        "revocation names the rotated principal: {revoked}"
    );
    let mut trailing = String::new();
    assert_eq!(
        victim_reader
            .read_line(&mut trailing)
            .expect("read after revoke"),
        0,
        "victim connection closes after revocation: {trailing:?}"
    );
    let event = await_frame(
        &mut watcher_reader,
        "identity.revoked event",
        Duration::from_secs(5),
        |frame| {
            frame["frame"] == "event"
                && frame["event"]["event_type"] == "identity.revoked"
        },
    );
    assert_eq!(
        event["event"]["payload"]["principal_id"], "victim@party",
        "watcher is told which principal was revoked: {event}"
    );
    let stale = verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "victim@party".to_string(),
            CredentialSource::DirectPsk(victim_psk),
        ),
    )
    .expect_err("superseded credential must not verify");
    assert_eq!(
        stale.code, "validation_unrecognized_credential",
        "the rotated hash stands effective: {stale:?}"
    );

    victim_client.shutdown(std::net::Shutdown::Both).ok();
    watcher_client.shutdown(std::net::Shutdown::Both).ok();
    victim_handle.join().expect("join victim connection");
    watcher_handle.join().expect("join watcher connection");
}

/// A rotation whose credential publish fails but whose compensating rollback
/// persist still publishes the restored record leaves the old binding
/// effective: live sessions stay connected, the old credential verifies, and
/// the original write failure (not a double fault, not uncertainty) is
/// reported.
#[test]
fn rotate_commit_failure_with_uncertain_rollback_preserves_live_sessions() {
    let (temporary, runtime) = test_runtime();
    let bundle_name = "party";
    write_ui_bundle(&temporary, bundle_name);
    let configuration_roots =
        agentmux::configuration::ConfigurationRoots::single(
            temporary.path().join("conf"),
        );
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name)
        .expect("bundle paths");

    let victim_psk = provision_principal(
        &runtime,
        agentmux::relay::ProvisionParams::new("victim@party".to_string()),
    )
    .expect("provision victim")
    .psk
    .expect("one-time PSK");
    let (mut victim_client, victim_handle) =
        serve_on(&runtime, &configuration_roots, &bundle_paths);
    let mut victim_reader =
        hello(&mut victim_client, "victim@party", &victim_psk);
    victim_client
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("read timeout");
    drain_pending(&mut victim_reader, "victim@party");

    let sink_dir = temporary.path().join("sink");
    std::fs::create_dir_all(&sink_dir).expect("sink directory");
    let _guard = arm_rollback_sabotage(
        state_root.join("identity"),
        RollbackSabotage::PlantDirSyncSeam,
    );
    let failed = rotate_principal_credential(
        &runtime,
        "victim@party",
        CredentialDestination::Path {
            path: sink_dir.display().to_string(),
        },
    )
    .expect_err("credential publish must fail");
    assert_eq!(
        failed.code, "internal_credential_write",
        "the restored prior record reports the original failure: {failed:?}"
    );
    std::fs::remove_file(state_root.join("identity").join(".fault-dir-sync"))
        .ok();

    let mut line = String::new();
    match victim_reader.read_line(&mut line) {
        Err(error)
            if error.kind() == std::io::ErrorKind::WouldBlock
                || error.kind() == std::io::ErrorKind::TimedOut =>
        {
            // Still connected and silent: no revocation frame arrived.
        },
        Err(error) => {
            panic!("live session must stay connected, got error: {error:?}")
        },
        Ok(0) => panic!("live session must not close on failed rotate"),
        Ok(_) => {
            panic!("no frame may arrive on a failed rotate, got: {line:?}")
        },
    }
    verify_identity(
        &runtime,
        &IdentityDescriptor::new(
            "victim@party".to_string(),
            CredentialSource::DirectPsk(victim_psk),
        ),
    )
    .expect("prior binding intact after compensated rotate");

    victim_client.shutdown(std::net::Shutdown::Both).ok();
    victim_handle.join().expect("join victim connection");
}
