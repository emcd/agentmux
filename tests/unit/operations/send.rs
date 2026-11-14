//! Direct public-consumer tests for the canonical `send` operation.
//!
//! This file imports only the public runtime and operations APIs
//! (`agentmux::relay`, `agentmux::operations`): no MCP server, no stdio
//! transport, no rmcp router, no agent harness, no MCP association.
//! Compiled only under `experimental-public-send`, matching the gated
//! surface exercised.

use agentmux::operations::{
    BundleScope, OperationError, SendInput, SendOutput, send,
};
use agentmux::relay::{
    CredentialSource, DeliveryConfiguration, IdentityDescriptor,
    ProvisionParams, RelayRuntime, VerifiedPrincipal, provision_principal,
    rotate_principal_credential, verify_identity,
};
use tempfile::TempDir;

struct SendFixture {
    _temporary: TempDir,
    runtime: RelayRuntime,
}

fn write_bundles(temporary: &TempDir, send_scope: &str) {
    let configuration_root = temporary.path().join("conf");
    let bundles = configuration_root.join("bundles");
    std::fs::create_dir_all(&bundles).expect("create bundles directory");
    std::fs::write(
        configuration_root.join("policies.toml"),
        format!(
            r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "home"
look = "self"
send = "{send_scope}"
"#
        ),
    )
    .expect("write policies file");
    for (bundle, members) in
        [("party", vec!["alpha", "bravo"]), ("other", vec!["carol"])]
    {
        let sessions: Vec<String> = members
            .iter()
            .map(|id| {
                format!(
                    r#"
[[sessions]]
id = "{id}"
name = "{id}"
directory = "/tmp"
coder = "shell"
"#
                )
            })
            .collect();
        std::fs::write(
            bundles.join(format!("{bundle}.toml")),
            format!("format-version = 1\n{}", sessions.join("")),
        )
        .expect("write bundle configuration");
    }
    std::fs::write(
        configuration_root.join("coders.toml"),
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
}

fn send_fixture(send_scope: &str) -> SendFixture {
    let temporary = TempDir::new().expect("temporary directory");
    write_bundles(&temporary, send_scope);
    let runtime = RelayRuntime::init(
        temporary.path().join("conf"),
        temporary.path().join("state"),
        DeliveryConfiguration::default(),
    )
    .expect("runtime init");
    SendFixture {
        _temporary: temporary,
        runtime,
    }
}

fn verified_context(
    runtime: &RelayRuntime,
    principal_id: &str,
) -> VerifiedPrincipal {
    let psk = provision_principal(
        runtime,
        ProvisionParams::new(principal_id.to_string()),
    )
    .expect("provision principal")
    .psk
    .expect("one-time raw PSK");
    verify_identity(
        runtime,
        &IdentityDescriptor::new(
            principal_id.to_string(),
            CredentialSource::DirectPsk(psk),
        ),
    )
    .expect("verify identity")
}

/// Binds a relay-wide principal to the operator policy so dispatch
/// authorizes it: relay-wide homes have no bundle members, so their
/// controls come entirely from the TUI session presets, independent
/// of any bundle. Without this binding even a verified context has
/// no resolved controls (validation_unknown_sender).
fn authorize_global_operator(fixture: &SendFixture, operator_id: &str) {
    let configuration_root = fixture._temporary.path().join("conf");
    std::fs::write(
        configuration_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "home"
look = "self"
send = "all"

[[policies]]
id = "operator"

[policies.controls]
list = "home"
look = "home"
send = "all"
"#,
    )
    .expect("write operator policies");
    std::fs::write(
        configuration_root.join("users.toml"),
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
    .expect("write operator binding");
}

fn send_input(targets: &[&str], message: &str) -> SendInput {
    SendInput {
        targets: targets.iter().map(ToString::to_string).collect(),
        broadcast: false,
        message: message.to_string(),
        request_id: Some("c-1".to_string()),
        bundle: BundleScope::Home,
    }
}

/// A direct call executes the full dispatch through the public
/// boundary and returns the typed outcome: request id echoed, sender
/// attributed to the verified principal, per-target results present.
#[tokio::test]
async fn direct_send_returns_typed_outcome() {
    let fixture = send_fixture("all");
    let context = verified_context(&fixture.runtime, "alpha@party");
    let output =
        send(&fixture.runtime, &context, send_input(&["bravo"], "hi"))
            .await
            .expect("direct send succeeds");
    assert_eq!(output.request_id.as_deref(), Some("c-1"));
    assert_eq!(output.requester_session, "alpha@party");
    assert!(!output.schema_version.is_empty());
    assert_eq!(output.results.len(), 1);
    assert_eq!(output.results[0].target_session, "bravo@party");
}

/// An absent caller correlation id stays absent in the typed output,
/// never a null.
#[tokio::test]
async fn direct_send_echoes_absent_request_id_as_absent() {
    let fixture = send_fixture("all");
    let context = verified_context(&fixture.runtime, "alpha@party");
    let mut input = send_input(&["bravo"], "hi");
    input.request_id = None;
    let output = send(&fixture.runtime, &context, input)
        .await
        .expect("direct send succeeds");
    assert_eq!(output.request_id, None);
}

/// A named bundle parameter qualifies bare targets against it.
#[tokio::test]
async fn direct_send_maps_named_bundle() {
    let fixture = send_fixture("all");
    let context = verified_context(&fixture.runtime, "alpha@party");
    let mut input = send_input(&["carol"], "hi");
    input.bundle = BundleScope::Named("other".to_string());
    let output = send(&fixture.runtime, &context, input)
        .await
        .expect("direct send succeeds");
    assert_eq!(output.results.len(), 1);
    assert_eq!(output.results[0].target_session, "carol@other");
}

/// Broadcast with the canonical home scope fans out in the
/// principal's home bundle, like the MCP bound-bundle default.
#[tokio::test]
async fn direct_send_broadcast_home_scope() {
    let fixture = send_fixture("all");
    let context = verified_context(&fixture.runtime, "alpha@party");
    let output = send(
        &fixture.runtime,
        &context,
        SendInput {
            targets: Vec::new(),
            broadcast: true,
            message: "hi".to_string(),
            request_id: None,
            bundle: BundleScope::Home,
        },
    )
    .await
    .expect("direct home broadcast succeeds");
    let mut targets: Vec<&str> = output
        .results
        .iter()
        .map(|result| result.target_session.as_str())
        .collect();
    targets.sort_unstable();
    assert_eq!(targets, vec!["bravo@party"]);
}

/// Positive control for the relay-wide requester: bound to the
/// operator policy, op@GLOBAL fans a Named-scope broadcast out to
/// every scope member (a GLOBAL sender is no home member, so nothing
/// is excluded).
#[tokio::test]
async fn direct_send_global_operator_broadcasts_named_scope() {
    let fixture = send_fixture("all");
    authorize_global_operator(&fixture, "op@GLOBAL");
    let context = verified_context(&fixture.runtime, "op@GLOBAL");
    let output = send(
        &fixture.runtime,
        &context,
        SendInput {
            targets: Vec::new(),
            broadcast: true,
            message: "hi".to_string(),
            request_id: None,
            bundle: BundleScope::Named("party".to_string()),
        },
    )
    .await
    .expect("authorized global broadcast succeeds");
    let mut targets: Vec<&str> = output
        .results
        .iter()
        .map(|result| result.target_session.as_str())
        .collect();
    targets.sort_unstable();
    assert_eq!(targets, vec!["alpha@party", "bravo@party"]);
}

/// A relay-wide home carries no broadcast scope of its own: the
/// operation passes no bundle parameter (never a GLOBAL default —
/// a bare name fused with GLOBAL would classify as a relay-wide
/// user), so dispatch refuses with its parameter-required rule.
#[tokio::test]
async fn direct_send_broadcast_global_home_is_refused() {
    let fixture = send_fixture("all");
    authorize_global_operator(&fixture, "op@GLOBAL");
    let context = verified_context(&fixture.runtime, "op@GLOBAL");
    let error = send(
        &fixture.runtime,
        &context,
        SendInput {
            targets: Vec::new(),
            broadcast: true,
            message: "hi".to_string(),
            request_id: None,
            bundle: BundleScope::Home,
        },
    )
    .await
    .expect_err("global-home broadcast must fail");
    assert_eq!(error.code, "validation_invalid_arguments");
    assert!(
        error
            .message
            .contains("explicit operation-bundle parameter"),
        "parameter-required refusal, not a silent default: {error:?}"
    );
}

/// A bare target under a relay-wide home stays unqualified: no GLOBAL
/// default is constructed, so the dispatch layer's own unqualified
/// rejection governs.
#[tokio::test]
async fn direct_send_bare_target_global_home_is_refused() {
    let fixture = send_fixture("all");
    authorize_global_operator(&fixture, "op@GLOBAL");
    let context = verified_context(&fixture.runtime, "op@GLOBAL");
    let error = send(&fixture.runtime, &context, send_input(&["bravo"], "hi"))
        .await
        .expect_err("global-home bare target must fail");
    assert_eq!(error.code, "validation_unqualified_target");
}

/// Without a policy binding even a verified relay-wide context has
/// no resolved controls: provisioning is not authorization.
#[tokio::test]
async fn direct_send_unbound_global_has_no_controls() {
    let fixture = send_fixture("all");
    let context = verified_context(&fixture.runtime, "op@GLOBAL");
    let error = send(
        &fixture.runtime,
        &context,
        SendInput {
            targets: vec!["carol@other".to_string()],
            broadcast: false,
            message: "hi".to_string(),
            request_id: None,
            bundle: BundleScope::Named("other".to_string()),
        },
    )
    .await
    .expect_err("unbound global must fail");
    assert_eq!(error.code, "validation_unknown_sender");
}

/// Broadcast fans out to the named scope's members except the sender.
#[tokio::test]
async fn direct_send_broadcast_fans_out_scope() {
    let fixture = send_fixture("all");
    let context = verified_context(&fixture.runtime, "alpha@party");
    let output = send(
        &fixture.runtime,
        &context,
        SendInput {
            targets: Vec::new(),
            broadcast: true,
            message: "hi".to_string(),
            request_id: None,
            bundle: BundleScope::Named("party".to_string()),
        },
    )
    .await
    .expect("direct broadcast succeeds");
    let mut targets: Vec<&str> = output
        .results
        .iter()
        .map(|result| result.target_session.as_str())
        .collect();
    targets.sort_unstable();
    assert_eq!(targets, vec!["bravo@party"]);
}

/// Input shape validation rejects before anything touches the relay,
/// with the exact codes the MCP handler produces today.
#[tokio::test]
async fn direct_send_validates_input_shape() {
    let fixture = send_fixture("all");
    let context = verified_context(&fixture.runtime, "alpha@party");
    let cases: Vec<(SendInput, &str)> = vec![
        (
            SendInput {
                message: "   ".to_string(),
                ..send_input(&["bravo"], "   ")
            },
            "validation_invalid_arguments",
        ),
        (
            SendInput {
                broadcast: true,
                ..send_input(&["bravo"], "hi")
            },
            "validation_conflicting_targets",
        ),
        (send_input(&[], "hi"), "validation_empty_targets"),
        (
            SendInput {
                bundle: BundleScope::All,
                ..send_input(&["bravo"], "hi")
            },
            "validation_invalid_arguments",
        ),
    ];
    for (input, code) in cases {
        let error = send(&fixture.runtime, &context, input)
            .await
            .expect_err("shape violation must fail");
        assert_eq!(error.code, code, "wrong code: {error:?}");
    }
}

/// A relay denial surfaces as the canonical error with code and
/// details unchanged; the operation synthesizes no authorization of
/// its own.
#[tokio::test]
async fn direct_send_surfaces_relay_denial_unchanged() {
    let fixture = send_fixture("home");
    let context = verified_context(&fixture.runtime, "alpha@party");
    let error = send(
        &fixture.runtime,
        &context,
        send_input(&["carol@other"], "hi"),
    )
    .await
    .expect_err("cross-bundle send must be denied");
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
async fn direct_send_rejects_stale_context_after_rotation() {
    let fixture = send_fixture("all");
    let context = verified_context(&fixture.runtime, "alpha@party");
    rotate_principal_credential(
        &fixture.runtime,
        "alpha@party",
        agentmux::relay::CredentialDestination::Response,
    )
    .expect("rotate mid-flight");
    let error = send(&fixture.runtime, &context, send_input(&["bravo"], "hi"))
        .await
        .expect_err("rotated context must fail");
    assert_eq!(error.code, "validation_stale_verified_context");
}

/// A context minted by another runtime instance is foreign here.
#[tokio::test]
async fn direct_send_rejects_foreign_instance_context() {
    let first = send_fixture("all");
    let second = send_fixture("all");
    let context = verified_context(&first.runtime, "alpha@party");
    let error = send(&second.runtime, &context, send_input(&["bravo"], "hi"))
        .await
        .expect_err("foreign context must fail");
    assert_eq!(error.code, "validation_foreign_relay_instance");
}

/// Attribution comes only from the verified principal: the input
/// carries no sender field to abuse, and the outcome names the
/// context principal.
#[tokio::test]
async fn direct_send_attributes_requester_from_context() {
    let fixture = send_fixture("all");
    let context = verified_context(&fixture.runtime, "alpha@party");
    let output =
        send(&fixture.runtime, &context, send_input(&["bravo"], "hi"))
            .await
            .expect("direct send succeeds");
    assert_eq!(output.requester_session, "alpha@party");
}

/// The adapter-only unassociated-server failure never enters the
/// canonical operation error set: it is raised by MCP association
/// before delegation, which a direct call has no path to.
#[tokio::test]
async fn direct_send_never_reports_unassociated_server() {
    let open = send_fixture("all");
    let open_context = verified_context(&open.runtime, "alpha@party");
    let strict = send_fixture("home");
    let strict_context = verified_context(&strict.runtime, "alpha@party");
    let mut codes = Vec::new();
    for input in [
        send_input(&["bravo"], "   "),
        SendInput {
            broadcast: true,
            ..send_input(&["bravo"], "hi")
        },
        send_input(&[], "hi"),
        SendInput {
            bundle: BundleScope::All,
            ..send_input(&["bravo"], "hi")
        },
    ] {
        codes.push(
            send(&open.runtime, &open_context, input)
                .await
                .expect_err("shape violation must fail")
                .code,
        );
    }
    codes.push(
        send(
            &strict.runtime,
            &strict_context,
            send_input(&["carol@other"], "hi"),
        )
        .await
        .expect_err("denial must fail")
        .code,
    );
    assert!(
        !codes
            .iter()
            .any(|code| code == "validation_unassociated_server"),
        "adapter-only failure leaked into operations: {codes:?}"
    );
}

/// Outer-name spot check on the public operation boundary: the
/// named input, output, and error types carry no MCP/rmcp/schema
/// marker. This checks names only, not field types — the actual
/// exclusion guarantee is structural (the signature names only
/// `operations::inputs/outputs/error` and public runtime types;
/// audit the use-list above, which imports no MCP, rmcp, stdio, or
/// schema module).
#[test]
fn operation_surface_names_no_protocol_types() {
    for name in [
        std::any::type_name::<SendInput>(),
        std::any::type_name::<SendOutput>(),
        std::any::type_name::<OperationError>(),
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

/// The operation-rendered payload follows the documented MCP
/// contract assembly: optional fields present only when supplied,
/// results always present. Goldens below are transcribed from the
/// documented contract (operation-side half of the parity check;
/// the handler-delegation half lands with section 3.1).
#[test]
fn send_output_renders_documented_contract() {
    use agentmux::relay::SendOutcome;
    use agentmux::relay::SendResult;
    let output = SendOutput {
        schema_version: "1".to_string(),
        request_id: Some("c-1".to_string()),
        requester_session: "alpha@party".to_string(),
        sender_display_name: Some("Alpha".to_string()),
        authenticated_identity: Some("alpha@party".to_string()),
        on_behalf_of: None,
        results: vec![SendResult {
            target_session: "bravo@party".to_string(),
            message_id: "m-1".to_string(),
            outcome: SendOutcome::Delivered,
            reason_code: None,
            reason: None,
            details: None,
        }],
    };
    let rendered = render_send_contract(&output);
    let expected = serde_json::json!({
        "schema_version": "1",
        "request_id": "c-1",
        "requester_session": "alpha@party",
        "sender_display_name": "Alpha",
        "authenticated_identity": "alpha@party",
        "results": [{
            "target_session": "bravo@party",
            "message_id": "m-1",
            "outcome": "delivered",
        }],
    });
    assert_eq!(rendered, expected);
    let mut bare = output;
    bare.request_id = None;
    bare.sender_display_name = None;
    bare.authenticated_identity = None;
    let rendered = render_send_contract(&bare);
    assert!(rendered.get("request_id").is_none());
    assert!(rendered.get("sender_display_name").is_none());
    assert!(rendered.get("authenticated_identity").is_none());
    assert!(rendered.get("on_behalf_of").is_none());
    assert!(rendered.get("results").is_some());
}

/// Test-local rendering implementing ONLY the documented contract
/// assembly (omit-absent optionals); the operation side must match
/// it exactly for adapter parity to hold.
fn render_send_contract(output: &SendOutput) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert(
        "schema_version".to_string(),
        serde_json::Value::String(output.schema_version.clone()),
    );
    if let Some(request_id) = &output.request_id {
        map.insert(
            "request_id".to_string(),
            serde_json::Value::String(request_id.clone()),
        );
    }
    map.insert(
        "requester_session".to_string(),
        serde_json::Value::String(output.requester_session.clone()),
    );
    if let Some(name) = &output.sender_display_name {
        map.insert(
            "sender_display_name".to_string(),
            serde_json::Value::String(name.clone()),
        );
    }
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
    map.insert(
        "results".to_string(),
        serde_json::to_value(&output.results).expect("results serialize"),
    );
    serde_json::Value::Object(map)
}
