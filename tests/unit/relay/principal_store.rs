//! Principal-store expiry-pruning tests (identity-federation task 1.11).
//!
//! Exercise `relay::prune_principal_store` — the public startup-prune surface —
//! over a hand-written `principals.json`, asserting the fail-closed expiry
//! semantics and the persist-only-when-records-were-pruned contract.

use agentmux::relay::prune_principal_store;
use agentmux::runtime::paths::principal_store_path;
use tempfile::TempDir;

/// Writes a principal-store file with one record per supplied
/// `(principal_id, expires_at)` pair and returns the resolved state root. A
/// `None` expiry is serialized as an absent field (never-expires).
fn write_store(temporary: &TempDir, records: &[(&str, Option<&str>)]) -> std::path::PathBuf {
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
    let raw = std::fs::read_to_string(principal_store_path(state_root)).expect("read store");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("decode store");
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

    let pruned = prune_principal_store(&state_root).expect("prune principal store");

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

    let pruned = prune_principal_store(&state_root).expect("prune principal store");

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

    let pruned = prune_principal_store(&state_root).expect("prune missing store");

    assert_eq!(pruned, 0, "a missing store prunes nothing");
    assert!(
        !principal_store_path(&state_root).exists(),
        "pruning a missing store must not create the file"
    );
}
