use agentmux::relay::RelayResponse;
use serde_json::Value;
use tempfile::TempDir;

use super::helpers::*;

#[test]
fn acp_result_serialization_preserves_dispatch_phase_details() {
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
    assert_eq!(results[0]["reason_code"], Value::Null);
    assert_eq!(results[0]["reason"], Value::Null);
    assert_eq!(
        results[0]["details"]["delivery_phase"],
        Value::String("accepted_in_progress".to_string())
    );
}
