use agentmux::relay::{
    ListedBundle, ListedBundleStartupHealth, ListedBundleState, ListedSession,
    ListedSessionTransport,
};
use agentmux::tui::{
    BundleStatusDisplay, BundleStatusSeverity, autocomplete_recipient_input,
    bundle_status_severity, format_bundle_status_line, merge_tui_targets,
    parse_tui_target_identifier,
};

#[test]
fn parses_local_target_identifier() {
    let resolved = parse_tui_target_identifier("relay", "agentmux").expect("target");
    assert_eq!(resolved, "relay");
}

#[test]
fn rejects_slash_qualified_target_identifier() {
    let error = parse_tui_target_identifier("agentmux/relay", "agentmux").expect_err("must reject");
    assert!(error.to_string().contains("validation_unknown_target"));
}

#[test]
fn parses_at_prefixed_target_identifier() {
    let resolved = parse_tui_target_identifier("@relay", "agentmux").expect("target");
    assert_eq!(resolved, "relay");
}

#[test]
fn merges_to_field_into_deterministic_targets() {
    let targets = merge_tui_targets("relay, mcp, tui", "agentmux").expect("targets");
    assert_eq!(targets, vec!["relay", "mcp", "tui"]);
}

#[test]
fn merge_rejects_slash_qualified_target() {
    let error = merge_tui_targets("relay, agentmux/mcp", "agentmux").expect_err("must fail");
    assert!(error.to_string().contains("validation_unknown_target"));
}

#[test]
fn merge_rejects_empty_target_set() {
    let error = merge_tui_targets("", "agentmux").expect_err("must fail");
    assert!(error.to_string().contains("validation_empty_targets"));
}

#[test]
fn autocomplete_replaces_current_token_after_comma() {
    let candidates = vec!["relay".to_string(), "mcp".to_string(), "tui".to_string()];
    let completed = autocomplete_recipient_input("relay, tu", &candidates).expect("completion");
    assert_eq!(completed, "relay, tui");
}

#[test]
fn autocomplete_strips_at_prefix_from_current_token() {
    let candidates = vec!["relay".to_string(), "mcp".to_string(), "tui".to_string()];
    let completed = autocomplete_recipient_input("@tu", &candidates).expect("completion");
    assert_eq!(completed, "tui");
}

#[test]
fn autocomplete_returns_none_when_no_match_exists() {
    let candidates = vec!["relay".to_string(), "mcp".to_string()];
    let completed = autocomplete_recipient_input("x", &candidates);
    assert_eq!(completed, None);
}

fn listed_bundle(
    id: &str,
    hosted: bool,
    state: ListedBundleState,
    startup_health: Option<ListedBundleStartupHealth>,
    state_reason_code: Option<&str>,
    startup_failure_count: usize,
) -> ListedBundle {
    ListedBundle {
        id: id.to_string(),
        hosted,
        state,
        startup_health,
        state_reason_code: state_reason_code.map(str::to_string),
        state_reason: None,
        startup_failure_count,
        recent_startup_failures: Vec::new(),
        sessions: vec![ListedSession {
            id: "alpha".to_string(),
            name: None,
            transport: ListedSessionTransport::Tmux,
        }],
    }
}

#[test]
fn bundle_status_line_renders_hosted_up_healthy() {
    let bundle = listed_bundle(
        "agentmux",
        true,
        ListedBundleState::Up,
        Some(ListedBundleStartupHealth::Healthy),
        None,
        0,
    );
    let display = BundleStatusDisplay::from_listed_bundle(&bundle);
    assert_eq!(
        format_bundle_status_line(&display),
        "bundle=agentmux hosted=yes state=up startup_health=healthy"
    );
    assert_eq!(
        bundle_status_severity(&display),
        BundleStatusSeverity::Healthy
    );
}

#[test]
fn bundle_status_line_renders_hosted_down_distinct_from_unhosted() {
    let hosted_down = BundleStatusDisplay::from_listed_bundle(&listed_bundle(
        "agentmux",
        true,
        ListedBundleState::Down,
        None,
        Some("startup_failed"),
        3,
    ));
    assert_eq!(
        format_bundle_status_line(&hosted_down),
        "bundle=agentmux hosted=yes state=down reason_code=startup_failed startup_failure_count=3"
    );
    assert_eq!(
        bundle_status_severity(&hosted_down),
        BundleStatusSeverity::HostedDown
    );

    let unhosted = BundleStatusDisplay::from_listed_bundle(&listed_bundle(
        "agentmux",
        false,
        ListedBundleState::Down,
        None,
        Some("not_started"),
        0,
    ));
    assert_eq!(
        format_bundle_status_line(&unhosted),
        "bundle=agentmux hosted=no state=down reason_code=not_started"
    );
    assert_eq!(
        bundle_status_severity(&unhosted),
        BundleStatusSeverity::Unhosted
    );

    assert_ne!(
        format_bundle_status_line(&hosted_down),
        format_bundle_status_line(&unhosted),
        "hosted=true/down must be visibly distinct from hosted=false/down"
    );
    assert_ne!(
        bundle_status_severity(&hosted_down),
        bundle_status_severity(&unhosted),
        "severity buckets must differ so the picker can color them apart"
    );
}

#[test]
fn bundle_status_line_renders_hosted_up_degraded() {
    let display = BundleStatusDisplay::from_listed_bundle(&listed_bundle(
        "agentmux",
        true,
        ListedBundleState::Up,
        Some(ListedBundleStartupHealth::Degraded),
        None,
        1,
    ));
    assert_eq!(
        format_bundle_status_line(&display),
        "bundle=agentmux hosted=yes state=up startup_health=degraded startup_failure_count=1"
    );
    assert_eq!(
        bundle_status_severity(&display),
        BundleStatusSeverity::Degraded
    );
}
