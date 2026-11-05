#![cfg(test)]

//! Last-resort coverage for the rotation credential-publish failure arm:
//! the compensating rollback persist can only be faulted mid-call, and the
//! socket path runs that call on the blocking pool, where no per-call fault
//! seam can target it. Driving `handle_change_psk` directly on this thread
//! keeps the thread-gated commit hook precise.
//!
//! A single test drives both compensation outcomes: a rollback that fails
//! before its own publication leaves the rotated hash effective (revoke the
//! live session, report the double fault), while a rollback that publishes
//! the restored record leaves the old binding effective (leave sessions
//! alone, report the original failure).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::Notify;

use crate::configuration::{ConfigurationRoots, SessionType};
use crate::relay::CredentialDestination;
use crate::relay::delivery::DeliveryContext;
use crate::relay::identity::{
    PrincipalRecord, PrincipalStore, PrincipalType, generate_psk,
    hash_token_sha256,
};
use crate::relay::stream::{RegisterStreamOutcome, register_stream};
use crate::relay::test_hooks::{
    ConfinedCommitHook, arm_confined_commit_hook, disarm_confined_commit_hook,
};

use super::identity::handle_change_psk;

struct DisarmGuard;

impl Drop for DisarmGuard {
    fn drop(&mut self) {
        disarm_confined_commit_hook();
    }
}

/// Writes operator authorization for `requester`: `change psk` at all.
fn write_operator_configuration(
    temporary: &tempfile::TempDir,
    requester: &str,
) -> ConfigurationRoots {
    let conf = temporary.path().join("conf");
    std::fs::create_dir_all(&conf).expect("create config root");
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

[policies.controls.change]
psk = "all"
"#,
    )
    .expect("write policies");
    std::fs::write(
        conf.join("users.toml"),
        format!(
            r#"
default-session = "{requester}"

[[sessions]]
id = "{requester}"
policy = "operator"

[sessions.ui]
"#
        ),
    )
    .expect("write users");
    ConfigurationRoots::single(conf)
}

/// Seeds the store with `principal_id` bound to a fresh hash, returning the
/// hash so the test can tell which binding stands afterwards.
fn seed_principal(state_root: &std::path::Path, principal_id: &str) -> String {
    let hash = hash_token_sha256(generate_psk().as_str());
    let mut store = PrincipalStore::load(state_root).expect("load store");
    store.insert(PrincipalRecord {
        principal_id: principal_id.to_string(),
        principal_type: PrincipalType::Session,
        credential_hash: hash.clone(),
        scope: None,
        expires_at: None,
        metadata: Default::default(),
    });
    store.persist().expect("persist seed");
    hash
}

/// Registers a live stream for `principal_id` on `delivery`, returning the
/// frame receiver the revocation would be written to.
fn register_live_stream(
    delivery: &Arc<DeliveryContext>,
    principal_id: &str,
) -> tokio::sync::mpsc::UnboundedReceiver<Vec<u8>> {
    let (writer, receiver) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    match register_stream(
        delivery,
        principal_id,
        SessionType::Ui,
        writer,
        Some(principal_id.to_string()),
        Arc::new(Notify::new()),
        None,
    ) {
        Ok(RegisterStreamOutcome::Registered(_)) => receiver,
        Ok(RegisterStreamOutcome::IdentityClaimConflict { .. }) => {
            panic!("no conflicting connection exists")
        },
        Err(source) => panic!("registration failed: {source:?}"),
    }
}

/// Sabotages this thread's second confined commit: the first (the rotated
/// store persist) passes through, the second (the compensating rollback
/// persist) is broken before publication when `delete_tmp` or after it when
/// planting the dir-sync seam.
fn arm_rollback_sabotage(
    identity_dir: std::path::PathBuf,
    delete_tmp: bool,
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
        if delete_tmp {
            for entry in
                std::fs::read_dir(&identity_dir).expect("read identity")
            {
                let entry = entry.expect("dir entry");
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("principals.json.")
                    && name.ends_with(".store.tmp")
                {
                    std::fs::remove_file(entry.path()).ok();
                }
            }
        } else {
            std::fs::write(identity_dir.join(".fault-dir-sync"), b"")
                .expect("plant dir-sync seam");
        }
    }));
    DisarmGuard
}

/// Runs one rotation whose credential publish fails against a directory
/// target, with the rollback sabotaged as requested. Returns the error and
/// the post-call store.
fn rotate_with_broken_rollback(
    configuration_roots: &ConfigurationRoots,
    state_root: &std::path::Path,
    delivery: &Arc<DeliveryContext>,
    requester: &str,
    principal_id: &str,
    delete_tmp: bool,
) -> (crate::relay::RelayError, PrincipalStore) {
    let sink_dir = state_root.join("sink");
    std::fs::create_dir_all(&sink_dir).expect("sink directory");
    let _guard =
        arm_rollback_sabotage(state_root.join("identity"), delete_tmp);
    let error = handle_change_psk(
        delivery,
        configuration_roots,
        state_root,
        requester,
        principal_id.to_string(),
        CredentialDestination::Path {
            path: sink_dir.display().to_string(),
        },
    )
    .expect_err("rotation must fail");
    let store = PrincipalStore::load(state_root).expect("reload store");
    (error, store)
}

#[test]
fn rotate_commit_failure_rollback_outcome_follows_effective_state() {
    let requester = "operator@GLOBAL";
    let principal_id = "victim@party";

    // A rollback that fails before its own publication leaves the rotated
    // hash effective: the live session is revoked and the double fault is
    // reported.
    let temporary = tempfile::TempDir::new().expect("temporary");
    let configuration_roots =
        write_operator_configuration(&temporary, requester);
    let state_root = temporary.path().join("state");
    let old_hash = seed_principal(&state_root, principal_id);
    let delivery = Arc::new(DeliveryContext::new(
        crate::relay::DeliveryConfiguration::default(),
    ));
    let mut frames = register_live_stream(&delivery, principal_id);
    let (error, store) = rotate_with_broken_rollback(
        &configuration_roots,
        &state_root,
        &delivery,
        requester,
        principal_id,
        true,
    );
    assert_eq!(
        error.code, "internal_credential_rollback_failed",
        "failed compensation reports the double fault: {error:?}"
    );
    let revoked = frames.try_recv().expect("revocation frame written");
    let revoked = String::from_utf8(revoked).expect("frame text");
    assert!(
        revoked.contains("runtime_identity_revoked"),
        "live session torn down on the effective hash: {revoked}"
    );
    assert!(
        store.find_by_credential_hash(old_hash.as_str()).is_none(),
        "superseded credential no longer verifies"
    );
    assert!(
        store.find_by_principal_id(principal_id).is_some(),
        "rotated record stands effective"
    );

    // A rollback that publishes the restored record leaves the old binding
    // effective: sessions stay untouched and the original failure is
    // reported.
    let temporary = tempfile::TempDir::new().expect("temporary");
    let configuration_roots =
        write_operator_configuration(&temporary, requester);
    let state_root = temporary.path().join("state");
    let old_hash = seed_principal(&state_root, principal_id);
    let delivery = Arc::new(DeliveryContext::new(
        crate::relay::DeliveryConfiguration::default(),
    ));
    let mut frames = register_live_stream(&delivery, principal_id);
    let (error, store) = rotate_with_broken_rollback(
        &configuration_roots,
        &state_root,
        &delivery,
        requester,
        principal_id,
        false,
    );
    assert_eq!(
        error.code, "internal_credential_write",
        "restored record reports the original failure: {error:?}"
    );
    assert!(
        frames.try_recv().is_err(),
        "no revocation when the old binding stands"
    );
    let record = store
        .find_by_principal_id(principal_id)
        .expect("record restored");
    assert_eq!(
        record.credential_hash, old_hash,
        "prior binding effective after compensated rotation"
    );
}
