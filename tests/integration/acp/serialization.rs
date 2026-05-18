use agentmux::relay::RelayResponse;
use serde_json::Value;
use tempfile::TempDir;

use super::helpers::*;

#[test]
fn acp_result_serialization_reflects_queued_async_outcome() {
    let temporary = TempDir::new().expect("temporary");
    let options = AcpStubOptions {
        stop_reason: "cancelled".to_string(),
        ..AcpStubOptions::default()
    };
    let (config_root, _log_path) = write_configuration(temporary.path(), &options);
    let response = dispatch_send(
        &config_root,
        &temporary.path().join("tmux.sock"),
        Some(1_000),
    );
    let RelayResponse::Chat { results, .. } = response else {
        panic!("expected chat response");
    };
    let encoded = serde_json::to_value(results).expect("serialize results");
    let Value::Array(results) = encoded else {
        panic!("expected array");
    };
    assert_eq!(results.len(), 1);
    // Async dispatch records a queued per-target result with no terminal
    // reason or phase details; those are observed later via worker state.
    assert_eq!(results[0]["outcome"], Value::String("queued".to_string()));
    assert_eq!(results[0]["reason_code"], Value::Null);
    assert_eq!(results[0]["reason"], Value::Null);
    assert_eq!(results[0]["details"], Value::Null);
}
