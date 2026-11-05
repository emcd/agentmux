//! Principal-store expiry-pruning tests (identity-federation task 1.11).
//!
//! Exercise `relay::prune_principal_store` — the public startup-prune surface —
//! over a hand-written `principals.json`, asserting the fail-closed expiry
//! semantics and the persist-only-when-records-were-pruned contract.

use std::sync::Arc;
use std::time::Duration;

use agentmux::relay::{
    DeliveryConfiguration, DeliveryContext, RelayRuntime,
    prune_principal_store,
};
use agentmux::runtime::paths::principal_store_path;
use tempfile::TempDir;

/// Writes a principal-store file with one record per supplied
/// `(principal_id, expires_at)` pair and returns the resolved state root. A
/// `None` expiry is serialized as an absent field (never-expires).
fn write_store(
    temporary: &TempDir,
    records: &[(&str, Option<&str>)],
) -> std::path::PathBuf {
    let state_root = temporary.path().join("state");
    let store_path = principal_store_path(&state_root);
    std::fs::create_dir_all(store_path.parent().expect("store parent"))
        .expect("create identity directory");
    let principals: Vec<String> = records
        .iter()
        .enumerate()
        .map(|(index, (principal_id, expires_at))| {
            let expiry_field = match expires_at {
                Some(value) => format!(",\n      \"expires_at\": \"{value}\""),
                None => String::new(),
            };
            format!(
                "    {{\n      \"principal_id\": \"{principal_id}\",\n      \"principal_type\": \"session\",\n      \"credential_hash\": \"{index:064x}\"{expiry_field}\n    }}"
            )
        })
        .collect();
    let body = format!(
        "{{\n  \"format_version\": 1,\n  \"principals\": [\n{}\n  ]\n}}",
        principals.join(",\n")
    );
    std::fs::write(&store_path, body).expect("write principal store");
    state_root
}

fn registered_ids(state_root: &std::path::Path) -> Vec<String> {
    let raw = std::fs::read_to_string(principal_store_path(state_root))
        .expect("read store");
    let value: serde_json::Value =
        serde_json::from_str(&raw).expect("decode store");
    let mut ids: Vec<String> = value["principals"]
        .as_array()
        .expect("principals array")
        .iter()
        .map(|record| {
            record["principal_id"]
                .as_str()
                .expect("principal_id")
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

/// Claims the domain for `state_root` through a live runtime: the only way
/// the public prune surface may mutate the store.
fn owned_runtime(
    temporary: &TempDir,
    state_root: &std::path::Path,
) -> RelayRuntime {
    let configuration_root = temporary.path().join("conf");
    std::fs::create_dir_all(&configuration_root).expect("create config root");
    RelayRuntime::init(
        configuration_root,
        state_root.to_path_buf(),
        DeliveryConfiguration::default(),
    )
    .expect("runtime claims the domain")
}

#[test]
fn prune_drops_past_and_unparseable_expiries_and_keeps_future_and_absent() {
    let temporary = TempDir::new().expect("temporary directory");
    let state_root = write_store(
        &temporary,
        &[
            ("past@alpha", Some("2000-01-01T00:00:00Z")),
            ("garbage@alpha", Some("not-a-timestamp")),
            ("future@alpha", Some("2999-01-01T00:00:00Z")),
            ("forever@alpha", None),
        ],
    );

    let runtime = owned_runtime(&temporary, &state_root);
    let pruned =
        prune_principal_store(&state_root, runtime.delivery_context())
            .expect("prune principal store");

    assert_eq!(pruned, 2, "past + unparseable records must be pruned");
    assert_eq!(
        registered_ids(&state_root),
        vec!["forever@alpha".to_string(), "future@alpha".to_string()],
        "only the future-dated and never-expiring records survive"
    );
}

#[test]
fn prune_leaves_file_untouched_when_nothing_expired() {
    let temporary = TempDir::new().expect("temporary directory");
    let state_root = write_store(
        &temporary,
        &[
            ("future@alpha", Some("2999-01-01T00:00:00Z")),
            ("forever@alpha", None),
        ],
    );
    let store_path = principal_store_path(&state_root);
    let before = std::fs::read(&store_path).expect("read store before prune");

    let runtime = owned_runtime(&temporary, &state_root);
    let pruned =
        prune_principal_store(&state_root, runtime.delivery_context())
            .expect("prune principal store");

    assert_eq!(pruned, 0, "no records are expired");
    let after = std::fs::read(&store_path).expect("read store after prune");
    assert_eq!(
        before, after,
        "the store must not be rewritten when nothing was pruned"
    );
}

#[test]
fn prune_on_missing_store_is_a_noop() {
    let temporary = TempDir::new().expect("temporary directory");
    let state_root = temporary.path().join("state");

    let runtime = owned_runtime(&temporary, &state_root);
    let pruned =
        prune_principal_store(&state_root, runtime.delivery_context())
            .expect("prune missing store");

    assert_eq!(pruned, 0, "a missing store prunes nothing");
    assert!(
        !principal_store_path(&state_root).exists(),
        "pruning a missing store must not create the file"
    );
}

/// A delivery state that does not own the root cannot prune: the ownership
/// proof is the live owner's handle, not merely a delivery state.
#[test]
fn prune_with_non_owning_delivery_is_refused() {
    let temporary = TempDir::new().expect("temporary directory");
    let state_root = write_store(
        &temporary,
        &[("past@alpha", Some("2000-01-01T00:00:00Z"))],
    );
    let _runtime = owned_runtime(&temporary, &state_root);

    let foreign =
        Arc::new(DeliveryContext::new(DeliveryConfiguration::default()));
    let failed = match prune_principal_store(&state_root, &foreign) {
        Ok(_) => panic!("prune without domain ownership must fail"),
        Err(error) => error,
    };
    assert_eq!(
        failed.code, "validation_runtime_already_hosted",
        "prune without ownership is refused: {failed:?}"
    );
}

/// A second process cannot prune while the domain is held: the public prune
/// surface refuses without ownership even though the store file is readable.
/// The child runs this same test binary filtered to this test with a hold
/// directive.
#[test]
fn prune_from_another_process_while_owned_is_refused() {
    // Child branch: claim the domain named by the environment and hold it
    // until stdin closes, signalling readiness through the ready file.
    if let Ok(hold_root) = std::env::var("AGENTMUX_TEST_HOLD_PRUNE_DOMAIN") {
        let hold_conf = std::env::var("AGENTMUX_TEST_HOLD_PRUNE_CONF")
            .expect("hold configuration root");
        let hold_ready = std::env::var("AGENTMUX_TEST_HOLD_PRUNE_READY")
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
    let temporary = TempDir::new().expect("temporary directory");
    let configuration_root = temporary.path().join("conf");
    let state_root = write_store(
        &temporary,
        &[("past@alpha", Some("2000-01-01T00:00:00Z"))],
    );
    let ready_file = temporary.path().join("child.ready");
    std::fs::create_dir_all(&configuration_root).expect("create config root");

    let test_binary = std::env::current_exe().expect("test binary");
    let mut child = std::process::Command::new(test_binary)
        .arg("relay::principal_store::prune_from_another_process_while_owned_is_refused")
        .arg("--exact")
        .env("AGENTMUX_TEST_HOLD_PRUNE_DOMAIN", &state_root)
        .env("AGENTMUX_TEST_HOLD_PRUNE_CONF", &configuration_root)
        .env("AGENTMUX_TEST_HOLD_PRUNE_READY", &ready_file)
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
    let foreign =
        Arc::new(DeliveryContext::new(DeliveryConfiguration::default()));
    let failed = match prune_principal_store(&state_root, &foreign) {
        Ok(_) => panic!("cross-process prune over a held root must fail"),
        Err(error) => error,
    };
    assert_eq!(
        failed.code, "validation_runtime_already_hosted",
        "cross-process prune is refused: {failed:?}"
    );
    drop(child.stdin.take());
    let status = child.wait().expect("reap holding child");
    assert!(status.success(), "holding child exits cleanly");
}
