//! Operator-facing rendering of structured relay errors.
//!
//! Exercises [`RelayError::operator_message`], the seam `agentmux up` (and the
//! other relay-backed CLI surfaces) route through so a configuration parse /
//! unknown-field failure names the offending file and field instead of a bare
//! summary.

use agentmux::relay::RelayError;
use serde_json::json;

#[test]
fn operator_message_folds_config_path_and_cause() {
    let error = RelayError {
        code: "validation_invalid_arguments".to_string(),
        message: "failed to parse authorization policy artifact".to_string(),
        details: Some(json!({
            "path": "/home/op/.config/agentmux/policies.toml",
            "cause": "unknown field `new`, expected one of `find`, `list`",
        })),
    };

    assert_eq!(
        error.operator_message(),
        "failed to parse authorization policy artifact \
         (/home/op/.config/agentmux/policies.toml: unknown field `new`, \
         expected one of `find`, `list`)",
    );
}

#[test]
fn operator_message_returns_plain_message_without_details() {
    let error = RelayError {
        code: "internal_unexpected_failure".to_string(),
        message: "bundle configuration is invalid".to_string(),
        details: None,
    };

    assert_eq!(error.operator_message(), "bundle configuration is invalid");
}

#[test]
fn operator_message_ignores_unrelated_detail_shapes() {
    // A capability-gate rejection carries `target_session` / `session_type`, not
    // the config `path` + `cause` pair, so the message is left untouched rather
    // than appending noise.
    let error = RelayError {
        code: "validation_unsupported_operation".to_string(),
        message: "target transport does not support this operation".to_string(),
        details: Some(json!({
            "target_session": "alpha@party",
            "session_type": "ui",
            "can_be_written": false,
        })),
    };

    assert_eq!(
        error.operator_message(),
        "target transport does not support this operation",
    );
}
