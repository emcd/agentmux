use agentmux::relay::{
    ListedBundle, ListedBundleStartupHealth, ListedBundleState, ListedSession,
    ListedSessionTransport,
};
use agentmux::tui::{
    BundleStatusDisplay, BundleStatusSeverity, RecipientReadiness, autocomplete_recipient_input,
    bundle_status_severity, format_bundle_status_line, format_recipient_picker_label,
    merge_tui_targets, parse_tui_target_identifier, sender_bound_bundle,
};

#[test]
fn parses_local_target_identifier() {
    let resolved = parse_tui_target_identifier("relay", Some("agentmux")).expect("target");
    assert_eq!(resolved, "relay");
}

#[test]
fn rejects_slash_qualified_target_identifier() {
    let error =
        parse_tui_target_identifier("agentmux/relay", Some("agentmux")).expect_err("must reject");
    assert!(error.to_string().contains("validation_unknown_target"));
}

#[test]
fn parses_at_prefixed_target_identifier() {
    let resolved = parse_tui_target_identifier("@relay", Some("agentmux")).expect("target");
    assert_eq!(resolved, "relay");
}

#[test]
fn parses_session_at_active_bundle_to_bare_session() {
    let resolved = parse_tui_target_identifier("relay@agentmux", Some("agentmux")).expect("target");
    assert_eq!(resolved, "relay");
}

#[test]
fn parses_session_at_peer_bundle_preserves_canonical_form() {
    let resolved =
        parse_tui_target_identifier("relay@other-bundle", Some("agentmux")).expect("target");
    assert_eq!(resolved, "relay@other-bundle");
}

#[test]
fn parses_global_user_target_identifier() {
    let resolved =
        parse_tui_target_identifier("operator@GLOBAL", Some("agentmux")).expect("target");
    assert_eq!(resolved, "operator@GLOBAL");
}

#[test]
fn relay_wide_sender_preserves_bundle_suffix_matching_display_bundle() {
    // A relay-wide sender (no bound bundle, modelled as `None`) must preserve a
    // peer-bundle suffix even when it equals the TUI's displayed active bundle.
    // The relay derives Send routing from this suffix; stripping it would strand
    // the target with no routing namespace.
    let resolved = parse_tui_target_identifier("qa-partner@agentmux-qa", None).expect("target");
    assert_eq!(resolved, "qa-partner@agentmux-qa");
}

#[test]
fn relay_wide_sender_preserves_all_bundle_suffixes() {
    let resolved = parse_tui_target_identifier("relay@agentmux", None).expect("target");
    assert_eq!(resolved, "relay@agentmux");
}

#[test]
fn relay_wide_sender_leaves_bare_target_unqualified() {
    let resolved = parse_tui_target_identifier("relay", None).expect("target");
    assert_eq!(resolved, "relay");
}

#[test]
fn parses_at_prefixed_canonical_target_identifier() {
    let resolved =
        parse_tui_target_identifier("@relay@other-bundle", Some("agentmux")).expect("target");
    assert_eq!(resolved, "relay@other-bundle");
}

#[test]
fn rejects_empty_session_in_canonical_target() {
    let error = parse_tui_target_identifier("@@relay", Some("agentmux")).expect_err("must reject");
    assert!(error.to_string().contains("validation_unknown_target"));
}

#[test]
fn rejects_empty_bundle_in_canonical_target() {
    let error = parse_tui_target_identifier("relay@", Some("agentmux")).expect_err("must reject");
    assert!(error.to_string().contains("validation_unknown_target"));
}

#[test]
fn rejects_multiple_at_separators_in_target_identifier() {
    let error =
        parse_tui_target_identifier("relay@one@two", Some("agentmux")).expect_err("must reject");
    assert!(error.to_string().contains("validation_unknown_target"));
}

#[test]
fn rejects_slash_in_bundle_qualifier() {
    let error =
        parse_tui_target_identifier("relay@bun/dle", Some("agentmux")).expect_err("must reject");
    assert!(error.to_string().contains("validation_unknown_target"));
}

#[test]
fn merge_dedupes_canonical_and_bare_intra_bundle_targets() {
    let targets = merge_tui_targets("relay, relay@agentmux", Some("agentmux")).expect("targets");
    assert_eq!(targets, vec!["relay"]);
}

#[test]
fn merge_preserves_peer_bundle_target_alongside_local() {
    let targets = merge_tui_targets("relay, mcp@other-bundle", Some("agentmux")).expect("targets");
    assert_eq!(targets, vec!["relay", "mcp@other-bundle"]);
}

#[test]
fn sender_bound_bundle_exposes_active_bundle_for_session_principal() {
    // A bundle-bound session sender (bare id, no `@GLOBAL`) is bound to the
    // active bundle, so intra-bundle suffixes are stripped on send.
    assert_eq!(sender_bound_bundle("tui", "agentmux"), Some("agentmux"));
}

#[test]
fn sender_bound_bundle_is_none_for_relay_wide_principal() {
    // A relay-wide sender (`@GLOBAL`) has no bound bundle; target suffixes must
    // be preserved so the relay can route the send (todos/tui/46).
    assert_eq!(sender_bound_bundle("operator@GLOBAL", "agentmux"), None);
}

#[test]
fn merge_relay_wide_sender_preserves_cross_bundle_target() {
    // Regression for todos/tui/46: a relay-wide TUI principal sending to a peer
    // bundle must keep the `@bundle` suffix so the relay can resolve routing,
    // even when the target bundle equals the displayed active bundle.
    let targets = merge_tui_targets("qa-partner@agentmux-qa", None).expect("targets");
    assert_eq!(targets, vec!["qa-partner@agentmux-qa"]);
}

#[test]
fn merges_to_field_into_deterministic_targets() {
    let targets = merge_tui_targets("relay, mcp, tui", Some("agentmux")).expect("targets");
    assert_eq!(targets, vec!["relay", "mcp", "tui"]);
}

#[test]
fn merge_rejects_slash_qualified_target() {
    let error = merge_tui_targets("relay, agentmux/mcp", Some("agentmux")).expect_err("must fail");
    assert!(error.to_string().contains("validation_unknown_target"));
}

#[test]
fn merge_rejects_empty_target_set() {
    let error = merge_tui_targets("", Some("agentmux")).expect_err("must fail");
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
            ready: true,
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
fn recipient_picker_label_renders_ready_session_without_marker() {
    let with_name =
        format_recipient_picker_label("alpha", Some("Alpha"), RecipientReadiness::Ready);
    assert_eq!(with_name, "alpha (Alpha)");
    let without_name = format_recipient_picker_label("alpha", None, RecipientReadiness::Ready);
    assert_eq!(without_name, "alpha");
}

#[test]
fn recipient_picker_label_appends_not_ready_marker() {
    let labelled =
        format_recipient_picker_label("alpha", Some("Alpha"), RecipientReadiness::NotReady);
    assert_eq!(labelled, "alpha (Alpha)  [not ready]");
    let bare = format_recipient_picker_label("alpha", None, RecipientReadiness::NotReady);
    assert_eq!(bare, "alpha  [not ready]");
    assert_ne!(
        format_recipient_picker_label("alpha", Some("Alpha"), RecipientReadiness::Ready),
        labelled,
        "ready and not-ready labels must be visibly distinct even without color"
    );
}

#[test]
fn recipient_readiness_classifies_relay_ready_field() {
    assert_eq!(
        RecipientReadiness::from_ready(true),
        RecipientReadiness::Ready
    );
    assert_eq!(
        RecipientReadiness::from_ready(false),
        RecipientReadiness::NotReady
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
