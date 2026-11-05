//! `change psk` compensation-rollback faults through the socket path.
//!
//! A root-scoped one-shot seam faults the compensating persist mid-call, so
//! a single dispatch deterministically drives either branch: a rollback that
//! fails before publication leaves the rotated hash effective (revoke live
//! sessions, notify watchers, report the double fault), while a rollback
//! that publishes the restored record leaves the old binding effective
//! (leave sessions alone, report the original failure). Both outcomes are
//! asserted on live streams, including self-rotation — where, unlike the
//! success path, no new credential exists to preserve the requester for.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use agentmux::configuration::ConfigurationRoots;
use agentmux::relay::test_hooks::{
    IdentityRollbackFault, arm_identity_rollback_fault,
    disarm_identity_rollback_fault,
};
use agentmux::relay::{DeliveryConfiguration, DeliveryContext};
use agentmux::runtime::paths::{
    BundleRuntimePaths, session_identity_psk_path,
};
use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;

/// Removes an armed rollback fault when the test ends, so a fault for a
/// call that never ran cannot leak into another test sharing the process.
struct RollbackDisarmGuard {
    state_root: PathBuf,
}

impl Drop for RollbackDisarmGuard {
    fn drop(&mut self) {
        disarm_identity_rollback_fault(&self.state_root);
    }
}

/// Provisioned principals for rotation-fault socket tests: a registered
/// operator holding the change grant (usable store-backed for self-rotation
/// or via socket-trust when operating on others), a session victim, and a
/// watching host scoped to the victim's namespace.
struct RotationCase {
    _temporary: TempDir,
    delivery: Arc<DeliveryContext>,
    configuration_roots: ConfigurationRoots,
    state_root: PathBuf,
    bundle_paths: BundleRuntimePaths,
    operator: String,
    operator_psk: String,
    victim: String,
    victim_psk: String,
    app_principal_id: String,
    app_psk: String,
}

fn provision_rotation_case(
    bundle_name: &str,
    watcher_scope: &str,
) -> RotationCase {
    let delivery =
        Arc::new(DeliveryContext::new(DeliveryConfiguration::default()));
    let temporary = TempDir::new().expect("temporary directory");
    let configuration_roots =
        write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name)
        .expect("bundle paths");
    let operator = global_user_id(bundle_name);
    let operator_psk = register_peer(
        &delivery,
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &operator,
        None,
    );
    let victim = format!("alpha@{bundle_name}");
    let victim_psk = register_peer(
        &delivery,
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &victim,
        None,
    );
    let app_principal_id = format!("engine_{bundle_name}@EXTERNAL");
    let app_psk = register_peer(
        &delivery,
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &app_principal_id,
        Some(watcher_scope),
    );
    RotationCase {
        _temporary: temporary,
        delivery,
        configuration_roots,
        state_root,
        bundle_paths,
        operator,
        operator_psk,
        victim,
        victim_psk,
        app_principal_id,
        app_psk,
    }
}

/// A rotation case whose session victim holds the change grant itself, so
/// it can self-rotate through a relay-owned config destination: the bundle
/// session carries the operator preset's `change psk` control.
fn provision_self_rotating_session_case(
    bundle_name: &str,
    watcher_scope: &str,
) -> RotationCase {
    let delivery =
        Arc::new(DeliveryContext::new(DeliveryConfiguration::default()));
    let temporary = TempDir::new().expect("temporary directory");
    let configuration_roots =
        write_identity_configuration(&temporary, bundle_name);
    let configuration_root = configuration_roots.base_layer().to_path_buf();
    // Grant the session itself the operator preset: self-rotation must
    // authorize against the session's own bundle policy. Registration
    // does not consult bundle membership, so overwriting the bundle
    // afterwards is safe.
    std::fs::write(
        configuration_root
            .join("bundles")
            .join(format!("{bundle_name}.toml")),
        r#"
format-version = 1

[[sessions]]
id = "alpha"
name = "Alpha"
directory = "/tmp"
coder = "shell"
policy = "operator"

[[sessions]]
id = "bravo"
name = "Bravo"
directory = "/tmp"
coder = "shell"
"#,
    )
    .expect("write bundle configuration with session grant");
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name)
        .expect("bundle paths");
    let operator = global_user_id(bundle_name);
    let operator_psk = register_peer(
        &delivery,
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &operator,
        None,
    );
    let victim = format!("alpha@{bundle_name}");
    let victim_psk = register_peer(
        &delivery,
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &victim,
        None,
    );
    let app_principal_id = format!("engine_{bundle_name}@EXTERNAL");
    let app_psk = register_peer(
        &delivery,
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &app_principal_id,
        Some(watcher_scope),
    );
    RotationCase {
        _temporary: temporary,
        delivery,
        configuration_roots,
        state_root,
        bundle_paths,
        operator,
        operator_psk,
        victim,
        victim_psk,
        app_principal_id,
        app_psk,
    }
}
fn hello_live(
    case: &RotationCase,
    principal_id: &str,
    token: &str,
) -> (UnixStream, BufReader<UnixStream>, JoinHandle<()>) {
    let (mut client, join) = spawn_relay_connection(
        &case.delivery,
        &case.configuration_roots,
        &case.bundle_paths,
    );
    let mut reader = BufReader::new(client.try_clone().expect("clone"));
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": principal_id,
            "identity_token": token,
        }),
    );
    assert_eq!(
        read_json(&mut reader)["frame"],
        "hello_ack",
        "{principal_id} hello not acked"
    );
    if principal_id == case.app_principal_id {
        read_until_event_type(&mut reader, "identity.snapshot");
    }
    client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    (client, reader, join)
}

/// Reads frames until the connection closes, returning what arrived. A
/// stalled connection trips the read timeout instead of hanging the suite.
fn read_frames_until_eof(
    reader: &mut BufReader<UnixStream>,
    what: &str,
) -> Vec<Value> {
    let mut frames = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Err(source) => {
                panic!("{what}: expected frames then EOF: {source:?}")
            },
            Ok(0) => return frames,
            Ok(_) => frames.push(
                serde_json::from_str(line.trim()).expect("decode frame"),
            ),
        }
    }
}

/// A rotation whose credential publish fails and whose compensating rollback
/// fails before publication leaves the rotated hash effective: the victim is
/// revoked, watching hosts are notified, and the operator observes the double
/// fault on its own (unrevoked) connection.
#[test]
fn change_psk_double_fault_revokes_victim_and_reports_to_operator() {
    let case = provision_rotation_case(
        "ident_rotate_ddistinct",
        "ident_rotate_ddistinct",
    );
    let (victim_client, mut victim_reader, victim_join) =
        hello_live(&case, &case.victim, &case.victim_psk);
    let (app_client, mut app_reader, app_join) =
        hello_live(&case, &case.app_principal_id, &case.app_psk);
    let (mut client, mut reader, join) =
        hello_live(&case, &case.operator, "socket-trust");

    let sink_dir = case.state_root.join("sink");
    std::fs::create_dir_all(&sink_dir).expect("sink directory");
    arm_identity_rollback_fault(
        &case.state_root,
        IdentityRollbackFault::PrePublicationFailure,
    );
    let _disarm = RollbackDisarmGuard {
        state_root: case.state_root.clone(),
    };
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "rotate-double-distinct-1",
            "request": {
                "operation": "change_psk",
                "principal_id": case.victim,
                "destination": {"kind": "path", "path": sink_dir.display().to_string()},
            },
        }),
    );

    let failed = read_json_skipping_hello_ack(&mut reader);
    assert_eq!(
        failed["response"]["error"]["code"],
        "internal_credential_rollback_failed",
        "failed compensation reports the double fault: {failed:?}"
    );
    let frames = read_frames_until_eof(&mut victim_reader, "victim");
    assert!(
        frames.iter().any(|frame| frame["response"]["error"]["code"]
            == "runtime_identity_revoked"),
        "live session torn down on the effective hash: {frames:?}"
    );
    let event = read_until_event_type(&mut app_reader, "identity.revoked");
    assert_eq!(
        event["event"]["payload"]["principal_id"], case.victim,
        "watcher is told which principal was revoked: {event:?}"
    );
    let rejection = hello_first_frame(
        &case.delivery,
        &case.configuration_roots,
        &case.bundle_paths,
        &case.victim,
        &case.victim_psk,
        false,
    );
    assert_eq!(
        rejection["response"]["error"]["code"],
        "validation_unrecognized_credential",
        "superseded credential no longer verifies: {rejection:?}"
    );

    shutdown_stream(&client, "shutdown operator stream");
    join.join().expect("join operator relay thread");
    shutdown_stream(&victim_client, "shutdown victim stream");
    victim_join.join().expect("join victim relay thread");
    shutdown_stream(&app_client, "shutdown app stream");
    app_join.join().expect("join app relay thread");
}

// A self-rotation through the same double fault revokes the requester too:
// unlike the success path there is no new PSK to deliver, so the exemption
// that preserves the requester's connection does not apply. The dispatch
// error response is never constructed — revocation cancels the frame loop
// mid-request — so the requester observes revocation and close.
#[test]
fn self_rotation_double_fault_revokes_requester_and_notifies_watchers() {
    let case =
        provision_rotation_case("ident_self_rotate_double_fault", "GLOBAL");
    let (mut client, mut reader, join) =
        hello_live(&case, &case.operator, &case.operator_psk);
    let (app_client, mut app_reader, app_join) =
        hello_live(&case, &case.app_principal_id, &case.app_psk);

    let sink_dir = case.state_root.join("sink");
    std::fs::create_dir_all(&sink_dir).expect("sink directory");
    arm_identity_rollback_fault(
        &case.state_root,
        IdentityRollbackFault::PrePublicationFailure,
    );
    let _disarm = RollbackDisarmGuard {
        state_root: case.state_root.clone(),
    };
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "self-rotate-double-fault-1",
            "request": {
                "operation": "change_psk",
                "principal_id": case.operator,
                "destination": {"kind": "path", "path": sink_dir.display().to_string()},
            },
        }),
    );

    let frames = read_frames_until_eof(&mut reader, "operator");
    assert!(
        frames.iter().any(|frame| frame["response"]["error"]["code"]
            == "runtime_identity_revoked"),
        "self-rotating session must be revoked: {frames:?}"
    );
    let event = read_until_event_type(&mut app_reader, "identity.revoked");
    assert_eq!(
        event["event"]["payload"]["principal_id"], case.operator,
        "watcher is told which principal was revoked: {event:?}"
    );
    let rejection = hello_first_frame(
        &case.delivery,
        &case.configuration_roots,
        &case.bundle_paths,
        &case.operator,
        &case.operator_psk,
        false,
    );
    assert_eq!(
        rejection["response"]["error"]["code"],
        "validation_unrecognized_credential",
        "superseded credential no longer verifies: {rejection:?}"
    );

    shutdown_stream(&client, "shutdown operator stream");
    join.join().expect("join operator relay thread");
    shutdown_stream(&app_client, "shutdown app stream");
    app_join.join().expect("join app relay thread");
}

// A self-rotation whose compensating rollback publishes the restored record
// leaves the old binding effective: the requester stays connected on its
// live session, no watching host is notified, and the original failure is
// reported.
#[test]
fn self_rotation_uncertain_rollback_preserves_requester_and_silences_watchers()
{
    let case =
        provision_rotation_case("ident_self_rotate_uncertain", "GLOBAL");
    let (mut client, mut reader, join) =
        hello_live(&case, &case.operator, &case.operator_psk);
    let (app_client, mut app_reader, app_join) =
        hello_live(&case, &case.app_principal_id, &case.app_psk);
    app_client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout");

    let sink_dir = case.state_root.join("sink");
    std::fs::create_dir_all(&sink_dir).expect("sink directory");
    arm_identity_rollback_fault(
        &case.state_root,
        IdentityRollbackFault::PostPublicationUncertain,
    );
    let _disarm = RollbackDisarmGuard {
        state_root: case.state_root.clone(),
    };
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "self-rotate-uncertain-1",
            "request": {
                "operation": "change_psk",
                "principal_id": case.operator,
                "destination": {"kind": "path", "path": sink_dir.display().to_string()},
            },
        }),
    );

    let failed = read_json_skipping_hello_ack(&mut reader);
    assert_eq!(
        failed["response"]["error"]["code"], "internal_credential_write",
        "restored record reports the original failure: {failed:?}"
    );

    let mut watcher_line = String::new();
    match app_reader.read_line(&mut watcher_line) {
        Err(source)
            if source.kind() == std::io::ErrorKind::WouldBlock
                || source.kind() == std::io::ErrorKind::TimedOut =>
        {
            // Silent: the compensated rotation notifies nobody.
        },
        Err(source) => panic!("watcher must stay connected: {source:?}"),
        Ok(0) => panic!("watcher must not close on compensated rotation"),
        Ok(_) => panic!(
            "no event may arrive on a compensated rotation, got: {watcher_line:?}"
        ),
    }

    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "self-rotate-uncertain-2",
            "request": {"operation": "change_psk", "principal_id": case.operator},
        }),
    );
    let rotation = read_json_skipping_hello_ack(&mut reader);
    assert_eq!(
        rotation["response"]["kind"], "change_psk",
        "compensated rotation must leave the binding rotatable: {rotation:?}"
    );
    assert_ne!(
        rotation["response"]["psk"].as_str(),
        Some(case.operator_psk.as_str()),
        "clean retry must mint a fresh credential"
    );

    shutdown_stream(&client, "shutdown operator stream");
    join.join().expect("join operator relay thread");
    shutdown_stream(&app_client, "shutdown app stream");
    app_join.join().expect("join app relay thread");
}

// A store publish that is durability-uncertain with a compensation that
// fails before publication leaves the rotated hash effective: same wire
// outcome as the credential-publish double fault through the analogous arm.
#[test]
fn change_psk_store_double_fault_reports_to_operator_and_revokes_victim() {
    let case = provision_rotation_case(
        "ident_rotate_store_distinct",
        "ident_rotate_store_distinct",
    );
    let (victim_client, mut victim_reader, victim_join) =
        hello_live(&case, &case.victim, &case.victim_psk);
    let (app_client, mut app_reader, app_join) =
        hello_live(&case, &case.app_principal_id, &case.app_psk);
    let (mut client, mut reader, join) =
        hello_live(&case, &case.operator, "socket-trust");

    let identity_dir = case.state_root.join("identity");
    std::fs::create_dir_all(&identity_dir).expect("identity directory");
    std::fs::write(identity_dir.join(".fault-dir-sync"), b"")
        .expect("plant fault seam");
    arm_identity_rollback_fault(
        &case.state_root,
        IdentityRollbackFault::PrePublicationFailure,
    );
    let _disarm = RollbackDisarmGuard {
        state_root: case.state_root.clone(),
    };
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "rotate-store-double-distinct-1",
            "request": {"operation": "change_psk", "principal_id": case.victim},
        }),
    );

    let failed = read_json_skipping_hello_ack(&mut reader);
    assert_eq!(
        failed["response"]["error"]["code"],
        "internal_credential_rollback_failed",
        "failed compensation reports the double fault: {failed:?}"
    );
    let frames = read_frames_until_eof(&mut victim_reader, "victim");
    assert!(
        frames.iter().any(|frame| frame["response"]["error"]["code"]
            == "runtime_identity_revoked"),
        "live session torn down on the effective hash: {frames:?}"
    );
    let event = read_until_event_type(&mut app_reader, "identity.revoked");
    assert_eq!(
        event["event"]["payload"]["principal_id"], case.victim,
        "watcher is told which principal was revoked: {event:?}"
    );
    let rejection = hello_first_frame(
        &case.delivery,
        &case.configuration_roots,
        &case.bundle_paths,
        &case.victim,
        &case.victim_psk,
        false,
    );
    assert_eq!(
        rejection["response"]["error"]["code"],
        "validation_unrecognized_credential",
        "superseded credential no longer verifies: {rejection:?}"
    );

    shutdown_stream(&client, "shutdown operator stream");
    join.join().expect("join operator relay thread");
    shutdown_stream(&victim_client, "shutdown victim stream");
    victim_join.join().expect("join victim relay thread");
    shutdown_stream(&app_client, "shutdown app stream");
    app_join.join().expect("join app relay thread");
}

// The same store double fault through a self-rotation revokes the
// requester: no new credential exists to preserve the connection for.
#[test]
fn self_rotation_store_double_fault_revokes_requester() {
    let case =
        provision_rotation_case("ident_self_rotate_store_double", "GLOBAL");
    let (mut client, mut reader, join) =
        hello_live(&case, &case.operator, &case.operator_psk);
    let (app_client, mut app_reader, app_join) =
        hello_live(&case, &case.app_principal_id, &case.app_psk);

    let identity_dir = case.state_root.join("identity");
    std::fs::create_dir_all(&identity_dir).expect("identity directory");
    std::fs::write(identity_dir.join(".fault-dir-sync"), b"")
        .expect("plant fault seam");
    arm_identity_rollback_fault(
        &case.state_root,
        IdentityRollbackFault::PrePublicationFailure,
    );
    let _disarm = RollbackDisarmGuard {
        state_root: case.state_root.clone(),
    };
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "self-rotate-store-double-1",
            "request": {"operation": "change_psk", "principal_id": case.operator},
        }),
    );

    let frames = read_frames_until_eof(&mut reader, "operator");
    assert!(
        frames.iter().any(|frame| frame["response"]["error"]["code"]
            == "runtime_identity_revoked"),
        "self-rotating session must be revoked: {frames:?}"
    );
    let event = read_until_event_type(&mut app_reader, "identity.revoked");
    assert_eq!(
        event["event"]["payload"]["principal_id"], case.operator,
        "watcher is told which principal was revoked: {event:?}"
    );
    let rejection = hello_first_frame(
        &case.delivery,
        &case.configuration_roots,
        &case.bundle_paths,
        &case.operator,
        &case.operator_psk,
        false,
    );
    assert_eq!(
        rejection["response"]["error"]["code"],
        "validation_unrecognized_credential",
        "superseded credential no longer verifies: {rejection:?}"
    );

    shutdown_stream(&client, "shutdown operator stream");
    join.join().expect("join operator relay thread");
    shutdown_stream(&app_client, "shutdown app stream");
    app_join.join().expect("join app relay thread");
}

// A rotation whose credential file publishes but whose directory sync
// fails leaves both the rotated file and the rotated hash standing: the
// uncertainty (not success) is reported, the victim is revoked, and
// watching hosts are notified. The fault seam lives beside the credential
// file rather than the store, so only the credential publish is uncertain
// while the store commit lands cleanly.
#[test]
fn change_psk_credential_uncertain_reports_and_revokes_victim() {
    let bundle_name = "ident_cred_uncertain_distinct";
    let case = provision_self_rotating_session_case(bundle_name, bundle_name);
    let (victim_client, mut victim_reader, victim_join) =
        hello_live(&case, &case.victim, &case.victim_psk);
    let (app_client, mut app_reader, app_join) =
        hello_live(&case, &case.app_principal_id, &case.app_psk);
    let (mut client, mut reader, join) =
        hello_live(&case, &case.operator, "socket-trust");

    let credential_dir =
        session_identity_psk_path(&case.state_root, bundle_name, "alpha")
            .parent()
            .expect("credential parent")
            .to_path_buf();
    std::fs::create_dir_all(&credential_dir).expect("credential directory");
    std::fs::write(credential_dir.join(".fault-dir-sync"), b"")
        .expect("plant fault seam");
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "rotate-cred-uncertain-1",
            "request": {
                "operation": "change_psk",
                "principal_id": case.victim,
                "destination": {"kind": "config"},
            },
        }),
    );

    // The operator is not revoked, so the uncertainty is delivered on its
    // connection rather than lost to a teardown.
    let failed = read_json_skipping_hello_ack(&mut reader);
    assert_eq!(
        failed["response"]["error"]["code"],
        "internal_credential_durability_uncertain",
        "published file and hash report uncertainty: {failed:?}"
    );
    let frames = read_frames_until_eof(&mut victim_reader, "victim");
    assert!(
        frames.iter().any(|frame| frame["response"]["error"]["code"]
            == "runtime_identity_revoked"),
        "live session torn down on the effective hash: {frames:?}"
    );
    let event = read_until_event_type(&mut app_reader, "identity.revoked");
    assert_eq!(
        event["event"]["payload"]["principal_id"], case.victim,
        "watcher is told which principal was revoked: {event:?}"
    );
    // The published file authenticates: the store holds the rotated hash
    // and the file on disk matches it.
    let rotated_psk = std::fs::read_to_string(session_identity_psk_path(
        &case.state_root,
        bundle_name,
        "alpha",
    ))
    .expect("published credential file");
    let accepted = hello_first_frame(
        &case.delivery,
        &case.configuration_roots,
        &case.bundle_paths,
        &case.victim,
        rotated_psk.trim(),
        false,
    );
    assert_eq!(
        accepted["frame"], "hello_ack",
        "published credential must verify: {accepted:?}"
    );
    let rejection = hello_first_frame(
        &case.delivery,
        &case.configuration_roots,
        &case.bundle_paths,
        &case.victim,
        &case.victim_psk,
        false,
    );
    assert_eq!(
        rejection["response"]["error"]["code"],
        "validation_unrecognized_credential",
        "superseded credential no longer verifies: {rejection:?}"
    );

    shutdown_stream(&client, "shutdown operator stream");
    join.join().expect("join operator relay thread");
    shutdown_stream(&victim_client, "shutdown victim stream");
    victim_join.join().expect("join victim relay thread");
    shutdown_stream(&app_client, "shutdown app stream");
    app_join.join().expect("join app relay thread");
}

// The same published-credential uncertainty through a self-rotation tears
// the requester down too: the error carries no new credential, so the
// success path's requester preservation does not apply.
#[test]
fn self_rotation_credential_uncertain_revokes_requester_and_notifies_watchers()
{
    let bundle_name = "ident_cred_uncertain_self";
    let case = provision_self_rotating_session_case(bundle_name, bundle_name);
    let (mut client, mut reader, join) =
        hello_live(&case, &case.victim, &case.victim_psk);
    let (app_client, mut app_reader, app_join) =
        hello_live(&case, &case.app_principal_id, &case.app_psk);

    let credential_dir =
        session_identity_psk_path(&case.state_root, bundle_name, "alpha")
            .parent()
            .expect("credential parent")
            .to_path_buf();
    std::fs::create_dir_all(&credential_dir).expect("credential directory");
    std::fs::write(credential_dir.join(".fault-dir-sync"), b"")
        .expect("plant fault seam");
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "self-rotate-cred-uncertain-1",
            "request": {
                "operation": "change_psk",
                "principal_id": case.victim,
                "destination": {"kind": "config"},
            },
        }),
    );

    // Revocation cancels the frame loop mid-request, so the dispatch error
    // is never constructed: the requester observes revocation and close.
    let frames = read_frames_until_eof(&mut reader, "victim");
    assert!(
        frames.iter().any(|frame| frame["response"]["error"]["code"]
            == "runtime_identity_revoked"),
        "self-rotating session must be revoked: {frames:?}"
    );
    let event = read_until_event_type(&mut app_reader, "identity.revoked");
    assert_eq!(
        event["event"]["payload"]["principal_id"], case.victim,
        "watcher is told which principal was revoked: {event:?}"
    );
    let rotated_psk = std::fs::read_to_string(session_identity_psk_path(
        &case.state_root,
        bundle_name,
        "alpha",
    ))
    .expect("published credential file");
    let accepted = hello_first_frame(
        &case.delivery,
        &case.configuration_roots,
        &case.bundle_paths,
        &case.victim,
        rotated_psk.trim(),
        false,
    );
    assert_eq!(
        accepted["frame"], "hello_ack",
        "published credential must verify: {accepted:?}"
    );

    shutdown_stream(&client, "shutdown victim stream");
    join.join().expect("join victim relay thread");
    shutdown_stream(&app_client, "shutdown app stream");
    app_join.join().expect("join app relay thread");
}
