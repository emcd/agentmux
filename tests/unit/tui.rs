use agentmux::tui::{
    autocomplete_recipient_input, merge_tui_targets, parse_tui_target_identifier,
    resolve_tui_look_target,
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

#[test]
fn look_target_prefers_selected_recipient() {
    let resolved = resolve_tui_look_target(Some("mcp".to_string()), "relay, tui", "agentmux")
        .expect("look target");
    assert_eq!(resolved, "mcp");
}

#[test]
fn look_target_falls_back_to_first_to_recipient() {
    let resolved =
        resolve_tui_look_target(None, "relay, tui", "agentmux").expect("look target from to");
    assert_eq!(resolved, "relay");
}

#[test]
fn look_target_requires_selection_or_to_recipient() {
    let error = resolve_tui_look_target(None, "", "agentmux").expect_err("must fail");
    assert!(error.to_string().contains("validation_unknown_target"));
}
