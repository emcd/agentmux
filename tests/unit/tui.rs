use agentmux::tui::{autocomplete_recipient_input, merge_tui_targets, parse_tui_target_identifier};

#[test]
fn parses_local_target_identifier() {
    let resolved = parse_tui_target_identifier("relay", "agentmux").expect("target");
    assert_eq!(resolved, "relay");
}

#[test]
fn parses_same_bundle_qualified_target_identifier() {
    let resolved = parse_tui_target_identifier("agentmux/relay", "agentmux").expect("target");
    assert_eq!(resolved, "relay");
}

#[test]
fn parses_at_prefixed_target_identifier() {
    let resolved = parse_tui_target_identifier("@relay", "agentmux").expect("target");
    assert_eq!(resolved, "relay");
}

#[test]
fn rejects_cross_bundle_qualified_target_identifier() {
    let error = parse_tui_target_identifier("other/relay", "agentmux").expect_err("must reject");
    assert!(
        error
            .to_string()
            .contains("validation_cross_bundle_unsupported")
    );
}

#[test]
fn merges_to_field_into_deterministic_targets() {
    let targets = merge_tui_targets("relay, mcp, agentmux/mcp, tui", "agentmux").expect("targets");
    assert_eq!(targets, vec!["relay", "mcp", "tui"]);
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
