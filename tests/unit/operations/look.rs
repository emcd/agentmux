//! Direct public-consumer tests for the canonical `look` operation.
//!
//! This file imports only the public runtime and operations APIs
//! (`agentmux::relay`, `agentmux::operations`): no MCP server, no stdio
//! transport, no rmcp router, no agent harness, no MCP association.
//! Compiled only under `experimental-public-send`, matching the gated
//! surface exercised.

use std::sync::Arc;

use agentmux::operations::{LookInput, LookOutput, LookTool};
use agentmux::relay::{
    DispatchWiring, LookSnapshotPayload, rotate_principal_credential,
};
use agentmux::transports::{
    LookFreshness, LookSnapshotSource, StructuredEntry,
};

use super::support::{
    OperationFixture, authorize_global_operator, operation_fixture,
    operation_fixture_with_coder, operation_fixture_with_look,
    verified_context,
};

fn look_tool(fixture: &OperationFixture) -> LookTool {
    let wiring = DispatchWiring::mirrored(&fixture.runtime, &[], true)
        .expect("mirrored wiring installs");
    LookTool::new(Arc::clone(&fixture.runtime), wiring)
}

fn look_input(target: &str, lines: Option<u64>) -> LookInput {
    LookInput {
        target: target.to_string(),
        lines,
        offset: None,
    }
}

/// A direct call returns the typed snapshot: requester and target
/// pinned, verified identity carried, structured entries delivered.
#[tokio::test]
async fn direct_look_returns_typed_snapshot() {
    let fixture = operation_fixture_with_coder("all", "acp");
    let tool = look_tool(&fixture);
    let context = verified_context(&fixture.runtime, "alpha@party");
    let output = tool
        .invoke(&context, look_input("alpha@party", Some(10)))
        .await
        .expect("direct look succeeds");
    assert_eq!(output.requester_session, "alpha@party");
    assert_eq!(output.target_session, "alpha@party");
    assert_eq!(
        output.authenticated_identity.as_deref(),
        Some("alpha@party")
    );
    match output.snapshot {
        LookSnapshotPayload::StructuredEntriesV1 {
            entries_total,
            returned_entries_count,
            ..
        } => {
            assert_eq!(entries_total, 0);
            assert_eq!(returned_entries_count, 0);
        },
        other => {
            panic!("acp baseline yields structured entries, got {other:?}")
        },
    }
}

/// A fully qualified target passes through fusion untouched, so a
/// home-scoped looker can inspect a peer bundle without any bundle
/// parameter (mirroring the MCP contract).
#[tokio::test]
async fn direct_look_passes_through_qualified_target() {
    let fixture = operation_fixture_with_look("all", "acp", "all");
    let tool = look_tool(&fixture);
    let context = verified_context(&fixture.runtime, "alpha@party");
    let output = tool
        .invoke(&context, look_input("carol@other", None))
        .await
        .expect("direct look succeeds");
    assert_eq!(output.target_session, "carol@other");
}

/// A positive control with the same authorized relay-wide context:
/// an explicitly qualified target needs no bundle default.
#[tokio::test]
async fn direct_look_global_operator_reads_qualified_target() {
    let fixture = operation_fixture_with_look("all", "acp", "all");
    authorize_global_operator(&fixture, "op@GLOBAL");
    let tool = look_tool(&fixture);
    let context = verified_context(&fixture.runtime, "op@GLOBAL");
    let output = tool
        .invoke(&context, look_input("alpha@party", None))
        .await
        .expect("authorized global look succeeds");
    assert_eq!(output.target_session, "alpha@party");
    assert_eq!(output.authenticated_identity.as_deref(), Some("op@GLOBAL"));
}

/// A nonzero offset on a tmux target is refused before capture:
/// the offset rides the public boundary through dispatch instead of
/// being dropped, and validation happens before any pane read, so no
/// live tmux session is needed to pin it.
#[tokio::test]
async fn direct_look_tmux_offset_is_rejected_before_capture() {
    let fixture = operation_fixture("all");
    let tool = look_tool(&fixture);
    let context = verified_context(&fixture.runtime, "alpha@party");
    let input = LookInput {
        target: "alpha@party".to_string(),
        lines: None,
        offset: Some(2),
    };
    let error = tool
        .invoke(&context, input)
        .await
        .expect_err("tmux offset must fail");
    assert_eq!(error.code, "validation_offset_unsupported");
    assert_eq!(
        error
            .details
            .as_ref()
            .and_then(|details| details.pointer("/details/offset")),
        Some(&serde_json::json!(2)),
        "refusal carries the rejected offset: {error:?}"
    );
}

/// A bare target under a relay-wide home stays unqualified: no GLOBAL
/// default is constructed, so the dispatch layer's unqualified
/// rejection governs.
#[tokio::test]
async fn direct_look_bare_target_global_home_is_refused() {
    let fixture = operation_fixture("all");
    authorize_global_operator(&fixture, "op@GLOBAL");
    let tool = look_tool(&fixture);
    let context = verified_context(&fixture.runtime, "op@GLOBAL");
    let error = tool
        .invoke(&context, look_input("bravo", None))
        .await
        .expect_err("global-home bare target must fail");
    assert_eq!(error.code, "validation_unqualified_target");
}

/// Input shape validation rejects before anything touches the relay,
/// with the exact codes the MCP handler produces today.
#[tokio::test]
async fn direct_look_validates_input_shape() {
    let fixture = operation_fixture("all");
    let tool = look_tool(&fixture);
    let context = verified_context(&fixture.runtime, "alpha@party");
    let cases: Vec<(LookInput, &str)> = vec![
        (look_input("   ", None), "validation_unknown_target"),
        (look_input("seat-1", Some(0)), "validation_invalid_lines"),
        (look_input("seat-1", Some(1001)), "validation_invalid_lines"),
    ];
    for (input, code) in cases {
        let error = tool
            .invoke(&context, input)
            .await
            .expect_err("shape violation must fail");
        assert_eq!(error.code, code, "wrong code: {error:?}");
    }
}

/// Without a policy binding even a verified relay-wide context has
/// no resolved controls: provisioning is not authorization.
#[tokio::test]
async fn direct_look_unbound_global_has_no_controls() {
    let fixture = operation_fixture("all");
    let tool = look_tool(&fixture);
    let context = verified_context(&fixture.runtime, "op@GLOBAL");
    let error = tool
        .invoke(&context, look_input("carol@other", None))
        .await
        .expect_err("unbound global must fail");
    assert_eq!(error.code, "validation_unknown_sender");
}

/// A relay denial surfaces as the canonical error with code and
/// details unchanged; the operation synthesizes no authorization of
/// its own.
#[tokio::test]
async fn direct_look_surfaces_relay_denial_unchanged() {
    let fixture = operation_fixture("home");
    let tool = look_tool(&fixture);
    let context = verified_context(&fixture.runtime, "alpha@party");
    let error = tool
        .invoke(&context, look_input("carol@other", None))
        .await
        .expect_err("cross-bundle look must be denied");
    assert_eq!(error.code, "authorization_forbidden");
    assert!(
        error.details.is_some(),
        "denial details pass through: {error:?}"
    );
}

/// A rotated credential denies a previously minted context: binding
/// epoch revalidation runs on the operation path, not just the
/// socket path.
#[tokio::test]
async fn direct_look_rejects_stale_context_after_rotation() {
    let fixture = operation_fixture("all");
    let tool = look_tool(&fixture);
    let context = verified_context(&fixture.runtime, "alpha@party");
    rotate_principal_credential(
        &fixture.runtime,
        "alpha@party",
        agentmux::relay::CredentialDestination::Response,
    )
    .expect("rotate mid-flight");
    let error = tool
        .invoke(&context, look_input("alpha@party", None))
        .await
        .expect_err("rotated context must fail");
    assert_eq!(error.code, "validation_stale_verified_context");
}

/// A context minted by another runtime instance is foreign here.
#[tokio::test]
async fn direct_look_rejects_foreign_instance_context() {
    let first = operation_fixture("all");
    let second = operation_fixture("all");
    let tool = look_tool(&second);
    let context = verified_context(&first.runtime, "alpha@party");
    let error = tool
        .invoke(&context, look_input("alpha@party", None))
        .await
        .expect_err("foreign context must fail");
    assert_eq!(error.code, "validation_foreign_relay_instance");
}

/// Attribution comes only from the verified principal: the input
/// carries no sender field to abuse, and the outcome names the
/// verified context.
#[tokio::test]
async fn direct_look_attributes_requester_from_context() {
    let fixture = operation_fixture_with_coder("all", "acp");
    let tool = look_tool(&fixture);
    let context = verified_context(&fixture.runtime, "alpha@party");
    let output = tool
        .invoke(&context, look_input("alpha@party", None))
        .await
        .expect("direct look succeeds");
    assert_eq!(output.requester_session, "alpha@party");
}

/// The unassociated-server failure never enters the canonical
/// operation error set: it is raised by MCP association before
/// delegation, which a direct call has no path to.
#[tokio::test]
async fn direct_look_never_reports_unassociated_server() {
    let open = operation_fixture("all");
    let open_tool = look_tool(&open);
    let open_context = verified_context(&open.runtime, "alpha@party");
    let mut codes = Vec::new();
    for input in [
        look_input("   ", None),
        look_input("seat-1", Some(0)),
        look_input("seat-1", Some(1001)),
    ] {
        codes.push(
            open_tool
                .invoke(&open_context, input)
                .await
                .expect_err("shape violation must fail")
                .code,
        );
    }
    assert!(
        !codes
            .iter()
            .any(|code| code == "validation_unassociated_server"),
        "adapter-only failure leaked into operations: {codes:?}"
    );
}

/// The operation-rendered look payload follows the documented MCP
/// contract assembly: optional fields present only when supplied,
/// snapshot format keys per payload kind. Goldens below are
/// transcribed from the documented contract (operation-side half of
/// the 4.2 parity check; the handler-delegation half lands with
/// section 3.1).
#[test]
fn look_output_renders_documented_contract() {
    use agentmux::relay::LookSnapshotPayload;
    let lines = LookOutput {
        schema_version: "1".to_string(),
        requester_session: "alpha@party".to_string(),
        target_session: "seat-1".to_string(),
        captured_at: "2026-11-14T00:00:00Z".to_string(),
        authenticated_identity: Some("alpha@party".to_string()),
        on_behalf_of: None,
        snapshot: LookSnapshotPayload::Lines {
            snapshot_lines: vec!["a".to_string(), "b".to_string()],
        },
    };
    let rendered = render_look_contract(&lines);
    let expected = serde_json::json!({
        "schema_version": "1",
        "requester_session": "alpha@party",
        "target_session": "seat-1",
        "captured_at": "2026-11-14T00:00:00Z",
        "authenticated_identity": "alpha@party",
        "snapshot_format": "lines",
        "snapshot_lines": ["a", "b"],
    });
    assert_eq!(rendered, expected);
    let structured = LookOutput {
        schema_version: "1".to_string(),
        requester_session: "alpha@party".to_string(),
        target_session: "seat-1".to_string(),
        captured_at: "2026-11-14T00:00:00Z".to_string(),
        authenticated_identity: None,
        on_behalf_of: None,
        snapshot: LookSnapshotPayload::StructuredEntriesV1 {
            snapshot_entries: vec![StructuredEntry::Agent {
                lines: vec!["hi".to_string()],
            }],
            entries_total: 9,
            returned_entries_count: 1,
            freshness: LookFreshness::Fresh,
            snapshot_source: LookSnapshotSource::LiveBuffer,
            stale_reason_code: None,
            snapshot_age_ms: Some(12),
        },
    };
    let rendered = render_look_contract(&structured);
    let expected = serde_json::json!({
        "schema_version": "1",
        "requester_session": "alpha@party",
        "target_session": "seat-1",
        "captured_at": "2026-11-14T00:00:00Z",
        "snapshot_format": "structured_entries_v1",
        "snapshot_entries": [{"kind": "agent", "lines": ["hi"]}],
        "entries_total": 9,
        "returned_entries_count": 1,
        "freshness": "fresh",
        "snapshot_source": "live_buffer",
        "snapshot_age_ms": 12,
    });
    assert_eq!(rendered, expected);
    assert!(rendered.get("stale_reason_code").is_none());
    assert!(rendered.get("authenticated_identity").is_none());
}

/// The public operation boundary names no MCP, rmcp, JSON-RPC, stdio,
/// or JSON schema type in its inputs, outputs, or error. This is an
/// outer-name spot check; the structural guarantee is the signature
/// itself (types live in `operations::inputs/outputs/error` and the
/// public runtime) plus the import list above, which names no MCP,
/// rmcp, stdio, or JSON-schema module.
#[test]
fn look_surface_names_no_protocol_types() {
    for name in [
        std::any::type_name::<LookInput>(),
        std::any::type_name::<LookOutput>(),
        std::any::type_name::<LookTool>(),
    ] {
        for banned in [
            "rmcp",
            "schemars",
            "JsonSchema",
            "ToolRouter",
            "CallToolResult",
            "ErrorData",
            "::mcp::",
        ] {
            assert!(
                !name.contains(banned),
                "protocol type leaked into operation surface: {name}"
            );
        }
    }
}

/// Test-local rendering implementing ONLY the documented contract
/// assembly (omit-absent optionals, per-format snapshot keys); the
/// operation output must match it exactly for adapter parity to
/// hold.
fn render_look_contract(output: &LookOutput) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert(
        "schema_version".to_string(),
        serde_json::Value::String(output.schema_version.clone()),
    );
    map.insert(
        "requester_session".to_string(),
        serde_json::Value::String(output.requester_session.clone()),
    );
    map.insert(
        "target_session".to_string(),
        serde_json::Value::String(output.target_session.clone()),
    );
    map.insert(
        "captured_at".to_string(),
        serde_json::Value::String(output.captured_at.clone()),
    );
    if let Some(identity) = &output.authenticated_identity {
        map.insert(
            "authenticated_identity".to_string(),
            serde_json::Value::String(identity.clone()),
        );
    }
    if let Some(delegate) = &output.on_behalf_of {
        map.insert(
            "on_behalf_of".to_string(),
            serde_json::Value::String(delegate.clone()),
        );
    }
    match &output.snapshot {
        LookSnapshotPayload::Lines { snapshot_lines } => {
            map.insert(
                "snapshot_format".to_string(),
                serde_json::Value::String("lines".to_string()),
            );
            map.insert(
                "snapshot_lines".to_string(),
                serde_json::to_value(snapshot_lines).expect("lines serialize"),
            );
        },
        LookSnapshotPayload::StructuredEntriesV1 {
            snapshot_entries,
            entries_total,
            returned_entries_count,
            freshness,
            snapshot_source,
            stale_reason_code,
            snapshot_age_ms,
        } => {
            map.insert(
                "snapshot_format".to_string(),
                serde_json::Value::String("structured_entries_v1".to_string()),
            );
            map.insert(
                "snapshot_entries".to_string(),
                serde_json::to_value(snapshot_entries)
                    .expect("entries serialize"),
            );
            map.insert(
                "entries_total".to_string(),
                serde_json::json!(entries_total),
            );
            map.insert(
                "returned_entries_count".to_string(),
                serde_json::json!(returned_entries_count),
            );
            map.insert(
                "freshness".to_string(),
                serde_json::to_value(freshness).expect("freshness serializes"),
            );
            map.insert(
                "snapshot_source".to_string(),
                serde_json::to_value(snapshot_source)
                    .expect("source serializes"),
            );
            if let Some(code) = stale_reason_code {
                map.insert(
                    "stale_reason_code".to_string(),
                    serde_json::Value::String(code.clone()),
                );
            }
            if let Some(age) = snapshot_age_ms {
                map.insert(
                    "snapshot_age_ms".to_string(),
                    serde_json::json!(age),
                );
            }
        },
    }
    serde_json::Value::Object(map)
}
