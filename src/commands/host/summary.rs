use serde_json::{Map, Value, json};

use crate::{
    commands::{RelayHostStartupBundle, RelayHostStartupSummary, shared},
    relay::StartupFailureRecord,
    runtime::error::RuntimeError,
    runtime::inscriptions::emit_inscription,
};

pub(super) fn build_startup_summary(
    host_mode: &str,
    bundles: Vec<RelayHostStartupBundle>,
) -> RelayHostStartupSummary {
    let hosted_bundle_count = bundles
        .iter()
        .filter(|bundle| bundle.outcome == "hosted")
        .count();
    let skipped_bundle_count = bundles
        .iter()
        .filter(|bundle| bundle.outcome == "skipped")
        .count();
    let failed_bundle_count = bundles
        .iter()
        .filter(|bundle| bundle.outcome == "failed")
        .count();
    RelayHostStartupSummary {
        schema_version: 1,
        host_mode: host_mode.to_string(),
        bundles,
        hosted_bundle_count,
        skipped_bundle_count,
        failed_bundle_count,
        hosted_any: hosted_bundle_count > 0,
    }
}

pub(super) fn startup_summary_payload(summary: &RelayHostStartupSummary) -> Value {
    let mut payload = Map::<String, Value>::new();
    payload.insert("schema_version".to_string(), json!(summary.schema_version));
    payload.insert("host_mode".to_string(), json!(summary.host_mode));
    payload.insert(
        "bundles".to_string(),
        Value::Array(
            summary
                .bundles
                .iter()
                .map(|bundle| {
                    json!({
                        "bundle_name": bundle.bundle_name,
                        "outcome": bundle.outcome,
                        "reason_code": bundle.reason_code,
                        "reason": bundle.reason,
                        "details": bundle.details,
                    })
                })
                .collect::<Vec<_>>(),
        ),
    );
    payload.insert(
        "hosted_bundle_count".to_string(),
        json!(summary.hosted_bundle_count),
    );
    payload.insert(
        "skipped_bundle_count".to_string(),
        json!(summary.skipped_bundle_count),
    );
    payload.insert(
        "failed_bundle_count".to_string(),
        json!(summary.failed_bundle_count),
    );
    payload.insert("hosted_any".to_string(), json!(summary.hosted_any));
    Value::Object(payload)
}

pub(super) fn render_startup_summary(summary: &RelayHostStartupSummary) {
    match serde_json::to_string(&startup_summary_payload(summary)) {
        Ok(encoded) => println!("{encoded}"),
        Err(source) => emit_inscription(
            "relay.startup.summary.encode_failed",
            &json!({
                "error": source.to_string(),
                "host_mode": summary.host_mode,
                "bundle_count": summary.bundles.len(),
            }),
        ),
    }
}

pub(super) fn hosted_startup_bundle(bundle_name: &str) -> RelayHostStartupBundle {
    RelayHostStartupBundle {
        bundle_name: bundle_name.to_string(),
        outcome: "hosted".to_string(),
        reason_code: None,
        reason: None,
        details: None,
    }
}

pub(super) fn skipped_startup_bundle(
    bundle_name: &str,
    reason_code: &str,
    reason: String,
) -> RelayHostStartupBundle {
    RelayHostStartupBundle {
        bundle_name: bundle_name.to_string(),
        outcome: "skipped".to_string(),
        reason_code: Some(reason_code.to_string()),
        reason: Some(reason),
        details: None,
    }
}

/// Builds the summary for an autostart bundle where no session reached ready
/// state. When individual sessions failed, folds their real per-session causes
/// into the summary — the `reason` string names each failed session and its
/// cause, and `details.failed_sessions` carries the structured records — so the
/// operator sees why startup failed rather than a blanket placeholder. With no
/// recorded failures (for example a bundle that configures no sessions) it keeps
/// the plain "nothing became ready" message.
pub(super) fn failed_autostart_bundle(
    bundle_name: &str,
    failed_startups: &[StartupFailureRecord],
) -> RelayHostStartupBundle {
    if failed_startups.is_empty() {
        return RelayHostStartupBundle {
            bundle_name: bundle_name.to_string(),
            outcome: "failed".to_string(),
            reason_code: Some("runtime_startup_failed".to_string()),
            reason: Some("no configured session reached ready state".to_string()),
            details: None,
        };
    }
    let joined = failed_startups
        .iter()
        .map(|failure| format!("{}: {}", failure.session_id, failure.reason))
        .collect::<Vec<_>>()
        .join("; ");
    let failed_sessions = failed_startups
        .iter()
        .map(|failure| {
            json!({
                "session_id": failure.session_id,
                "transport": failure.transport,
                "code": failure.code,
                "reason": failure.reason,
                "details": failure.details,
            })
        })
        .collect::<Vec<_>>();
    RelayHostStartupBundle {
        bundle_name: bundle_name.to_string(),
        outcome: "failed".to_string(),
        reason_code: Some("runtime_startup_failed".to_string()),
        reason: Some(format!(
            "no configured session reached ready state ({} failed) -- {joined}",
            failed_startups.len()
        )),
        details: Some(json!({ "failed_sessions": failed_sessions })),
    }
}

pub(super) fn failed_startup_bundle(
    bundle_name: &str,
    source: RuntimeError,
) -> RelayHostStartupBundle {
    let (reason_code, reason) = shared::runtime_error_reason(&source);
    RelayHostStartupBundle {
        bundle_name: bundle_name.to_string(),
        outcome: "failed".to_string(),
        reason_code: Some(reason_code),
        reason: Some(reason),
        details: None,
    }
}

/// Preserves the structured details a relay-layer startup failure carries
/// (for example the offending policy control and value), which the
/// `RuntimeError` mapping would otherwise flatten to a message string.
pub(super) fn failed_startup_bundle_from_relay_error(
    bundle_name: &str,
    source: crate::relay::RelayError,
) -> RelayHostStartupBundle {
    RelayHostStartupBundle {
        bundle_name: bundle_name.to_string(),
        outcome: "failed".to_string(),
        reason_code: Some(source.code),
        reason: Some(source.message),
        details: source.details,
    }
}
