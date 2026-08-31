use std::path::PathBuf;

use agentmux::relay::{
    ListedBundle, ListedBundleStartupHealth, ListedBundleState, ListedSession,
    ListedSessionTransport, StartupFailureRecord,
};
use agentmux::runtime::error::RuntimeError;
use agentmux::tui::{
    Action, BundleStatusDisplay, BundleStatusSeverity, KeyboardEnhancement, RecipientReadiness,
    TuiLaunchOptions, autocomplete_recipient_input, bundle_status_severity,
    format_bundle_status_line, format_keyboard_enhancement_lines, format_recipient_picker_label,
    format_startup_failure_lines, merge_tui_targets, parse_tui_target_identifier,
    sender_bound_bundle,
    workbench::{Workbench, WorkbenchField, WorkbenchMode, WorkbenchPickerColumn},
};

#[test]
fn parses_local_target_identifier() {
    let resolved = parse_tui_target_identifier("relay", Some("agentmux")).expect("target");
    assert_eq!(resolved, "relay@agentmux");
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
    assert_eq!(resolved, "relay@agentmux");
}

#[test]
fn parses_session_at_active_bundle_preserves_canonical_form() {
    // The relay requires fully-qualified targets, so a canonical id matching the
    // bound bundle is emitted verbatim rather than stripped to a bare session.
    let resolved = parse_tui_target_identifier("relay@agentmux", Some("agentmux")).expect("target");
    assert_eq!(resolved, "relay@agentmux");
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
fn relay_wide_sender_rejects_bare_target() {
    // A relay-wide sender has no bound bundle to qualify a bare target with, and
    // the relay rejects unqualified targets, so the client fails fast instead of
    // sending one the relay would reject.
    let error = parse_tui_target_identifier("relay", None).expect_err("must reject");
    assert!(error.to_string().contains("validation_unqualified_target"));
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
    // The bare `relay` qualifies to `relay@agentmux`, matching the canonical form,
    // so the two collapse to a single fully-qualified target.
    let targets = merge_tui_targets("relay, relay@agentmux", Some("agentmux")).expect("targets");
    assert_eq!(targets, vec!["relay@agentmux"]);
}

#[test]
fn merge_preserves_peer_bundle_target_alongside_local() {
    let targets = merge_tui_targets("relay, mcp@other-bundle", Some("agentmux")).expect("targets");
    assert_eq!(targets, vec!["relay@agentmux", "mcp@other-bundle"]);
}

#[test]
fn sender_bound_bundle_exposes_active_bundle_for_session_principal() {
    // A bundle-bound session sender (bare id, no `@GLOBAL`) is bound to the
    // active bundle, so bare targets are qualified with it on send.
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
    assert_eq!(
        targets,
        vec!["relay@agentmux", "mcp@agentmux", "tui@agentmux"]
    );
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
        principals: vec![ListedSession {
            id: "alpha".to_string(),
            name: None,
            transport: ListedSessionTransport::Tmux,
            ready: true,
        }],
        principals_partial: None,
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

fn startup_failure_record(session_id: &str, code: &str, reason: &str) -> StartupFailureRecord {
    StartupFailureRecord {
        session_id: session_id.to_string(),
        transport: ListedSessionTransport::Tmux,
        code: code.to_string(),
        reason: reason.to_string(),
        timestamp: "2026-07-02T00:00:00Z".to_string(),
        sequence: 1,
        details: None,
    }
}

#[test]
fn from_listed_bundle_carries_per_session_startup_failures() {
    let mut bundle = listed_bundle(
        "agentmux",
        true,
        ListedBundleState::Down,
        None,
        Some("startup_failed"),
        1,
    );
    bundle.recent_startup_failures = vec![startup_failure_record(
        "alpha",
        "runtime_startup_failed",
        "opencode: command not found",
    )];
    let display = BundleStatusDisplay::from_listed_bundle(&bundle);
    assert_eq!(
        format_startup_failure_lines(&display),
        vec![
            "startup_failure session=alpha code=runtime_startup_failed \
             reason=opencode: command not found"
                .to_string()
        ]
    );
}

#[test]
fn startup_failure_lines_empty_without_recorded_failures() {
    let display = BundleStatusDisplay::from_listed_bundle(&listed_bundle(
        "agentmux",
        true,
        ListedBundleState::Up,
        Some(ListedBundleStartupHealth::Healthy),
        None,
        0,
    ));
    assert!(format_startup_failure_lines(&display).is_empty());
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

#[test]
fn keyboard_enhancement_defaults_to_unsupported() {
    // The TUI renders help before it can prove anything about the terminal, so
    // the default has to be the conservative reading rather than the capable
    // one.
    assert_eq!(
        KeyboardEnhancement::default(),
        KeyboardEnhancement::Unsupported
    );
}

#[test]
fn only_active_keyboard_enhancement_disambiguates_modified_keys() {
    assert!(KeyboardEnhancement::Active.disambiguates_modified_keys());
    assert!(!KeyboardEnhancement::Unsupported.disambiguates_modified_keys());
    assert!(!KeyboardEnhancement::ProbeFailed.disambiguates_modified_keys());
}

#[test]
fn active_keyboard_enhancement_reports_distinct_modified_enter() {
    assert_eq!(
        format_keyboard_enhancement_lines(KeyboardEnhancement::Active),
        vec![
            "Kitty keyboard protocol: active".to_string(),
            "Enter with modifiers is reported distinctly".to_string(),
            "Ctrl+J inserts a newline in every case".to_string(),
        ]
    );
}

#[test]
fn unsupported_keyboard_enhancement_names_the_collapsed_modified_enter() {
    let lines = format_keyboard_enhancement_lines(KeyboardEnhancement::Unsupported);
    assert_eq!(lines[0], "Kitty keyboard protocol: unsupported");
    assert_eq!(lines[1], "Enter with modifiers arrives as bare Enter");
}

#[test]
fn probe_failure_reads_differently_from_an_answered_unsupported_probe() {
    // A terminal that answered "no" and a probe that never got an answer are
    // different operator problems: the first is the terminal, the second is
    // usually a missing tty or a swallowed reply. Collapsing them would hide
    // the second behind a wrong explanation.
    let failed = format_keyboard_enhancement_lines(KeyboardEnhancement::ProbeFailed);
    let unsupported = format_keyboard_enhancement_lines(KeyboardEnhancement::Unsupported);
    assert_eq!(failed[0], "Kitty keyboard protocol: probe failed");
    assert_ne!(failed[0], unsupported[0]);
}

#[test]
fn probe_failure_claims_nothing_about_the_terminal() {
    // A failed probe establishes only that the TUI could not determine or
    // enable disambiguation. Wording it as a fact about the terminal ("cannot
    // distinguish Shift+Enter") would send the operator after the wrong
    // problem, since a capable terminal reaches this outcome whenever the
    // query is swallowed.
    let lines = format_keyboard_enhancement_lines(KeyboardEnhancement::ProbeFailed);
    assert_eq!(lines[1], "Keyboard capability is undetermined");
    assert!(
        !lines.iter().any(|line| line.contains("Terminal cannot")),
        "probe-failure report must not assert a terminal limitation: {lines:?}"
    );
}

/// Builds a workbench from public launch options alone. The relay socket is
/// never contacted: every action these tests apply is local to workbench state.
fn action_workbench() -> Workbench {
    Workbench::new(TuiLaunchOptions {
        namespace: "agentmux".to_string(),
        sender_session: "tui".to_string(),
        relay_socket: PathBuf::from("/tmp/agentmux-action-test-relay.sock"),
        look_lines: None,
        available_bundles: vec!["agentmux".to_string()],
    })
}

fn apply(workbench: &mut Workbench, action: Action) {
    workbench
        .apply_action(action)
        .unwrap_or_else(|error| panic!("apply {action:?}: {error}"));
}

#[test]
fn applying_an_action_produces_its_behavior_without_a_key_event() {
    // Resolution and behavior are separable: naming the action is enough to
    // reach the behavior, with nothing standing in for a terminal event.
    let mut workbench = action_workbench();
    assert_eq!(workbench.focus(), WorkbenchField::To);
    apply(&mut workbench, Action::CycleNextFocus);
    assert_eq!(workbench.focus(), WorkbenchField::Message);
    apply(&mut workbench, Action::InsertComposeCharacter('h'));
    apply(&mut workbench, Action::InsertComposeCharacter('i'));
    assert_eq!(workbench.message_field(), "hi");
    apply(&mut workbench, Action::DeleteComposeCharacter);
    assert_eq!(workbench.message_field(), "h");
    apply(&mut workbench, Action::OpenPicker);
    assert!(workbench.picker_open());
    assert_eq!(workbench.picker_column(), WorkbenchPickerColumn::Sessions);
    apply(&mut workbench, Action::TogglePickerFocus);
    assert_eq!(workbench.picker_column(), WorkbenchPickerColumn::Bundles);
    apply(&mut workbench, Action::AppendPickerFilterCharacter('a'));
    assert_eq!(workbench.picker_filter(), "a");
    apply(&mut workbench, Action::DeletePickerFilterCharacter);
    assert_eq!(workbench.picker_filter(), "");
    apply(&mut workbench, Action::ClosePicker);
    assert!(!workbench.picker_open());
    assert!(!workbench.should_quit());
    apply(&mut workbench, Action::Quit);
    assert!(workbench.should_quit());
}

#[test]
fn a_public_caller_drives_the_workbench_by_action_alone() {
    // Everything named here is public: `Workbench`, `Action`, and the launch
    // options. A host outside the crate can compose and navigate a message
    // without constructing a `KeyEvent` and without reaching internal state.
    let mut workbench = action_workbench();
    apply(&mut workbench, Action::CycleNextFocus);
    for character in "line".chars() {
        apply(&mut workbench, Action::InsertComposeCharacter(character));
    }
    apply(&mut workbench, Action::InsertMessageNewline);
    for character in "two".chars() {
        apply(&mut workbench, Action::InsertComposeCharacter(character));
    }
    assert_eq!(workbench.message_field(), "line\ntwo");
    assert_eq!(workbench.message_cursor_line_and_column(), (1, 3));
    apply(&mut workbench, Action::MoveMessageCursorHome);
    assert_eq!(workbench.message_cursor_line_and_column(), (1, 0));
    apply(&mut workbench, Action::MoveMessageCursorUp);
    assert_eq!(workbench.message_cursor_line_and_column(), (0, 0));
    // A surface-switching action reaches the mode beneath from the same seam,
    // and Interaction with no target opens the picker to choose one.
    assert_eq!(workbench.mode(), WorkbenchMode::Communication);
    apply(&mut workbench, Action::ToggleMode);
    assert_eq!(workbench.mode(), WorkbenchMode::Interaction);
    assert!(workbench.picker_open());
}

#[test]
fn toggling_the_mode_dismisses_whichever_surface_is_open() {
    // The mode beneath is what changes, so a surface open over it is cleared
    // first. Applying the action directly is the only path that exercises this;
    // the key handlers reach the same end state through per-surface sequences.
    let mut workbench = action_workbench();
    apply(&mut workbench, Action::ToggleEventsOverlay);
    assert!(workbench.events_overlay_open());
    apply(&mut workbench, Action::ToggleMode);
    assert!(!workbench.events_overlay_open());
    assert_eq!(workbench.mode(), WorkbenchMode::Interaction);
    // Interaction without a target auto-opens the picker to choose one, which
    // makes the picker the surface the next toggle has to dismiss.
    assert!(workbench.picker_open());
    apply(&mut workbench, Action::ToggleMode);
    assert!(!workbench.picker_open());
    assert_eq!(workbench.mode(), WorkbenchMode::Communication);
    apply(&mut workbench, Action::ToggleHelpOverlay);
    assert!(workbench.help_overlay_open());
    apply(&mut workbench, Action::ToggleMode);
    assert!(!workbench.help_overlay_open());
    assert_eq!(workbench.mode(), WorkbenchMode::Interaction);
}

#[test]
fn committing_a_picker_session_resolves_by_screen_mode() {
    let mut workbench = action_workbench();
    workbench.set_recipients(&["master"]);
    apply(&mut workbench, Action::OpenPicker);
    apply(&mut workbench, Action::CommitPickerSession);
    assert_eq!(workbench.mode(), WorkbenchMode::Communication);
    assert_eq!(workbench.to_field(), "master");
    assert!(!workbench.picker_open());

    // The same action in Interaction mode opens the target instead, which needs
    // a relay `Look`. Reaching the relay at all is the assertion: a selection
    // that never got that far would fail with `validation_unknown_target`.
    let mut workbench = action_workbench();
    workbench.set_recipients(&["master"]);
    apply(&mut workbench, Action::ToggleMode);
    assert_eq!(workbench.mode(), WorkbenchMode::Interaction);
    assert!(workbench.picker_open());
    match workbench.apply_action(Action::CommitPickerSession) {
        Err(RuntimeError::Validation { code, .. }) => assert_eq!(code, "relay_unavailable"),
        Err(RuntimeError::Io { source, .. }) => {
            assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied)
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn navigating_the_interaction_pane_follows_the_write_draft() {
    let mut workbench = action_workbench();
    apply(&mut workbench, Action::ToggleMode);
    apply(&mut workbench, Action::ClosePicker);
    // With no draft there is no cursor to move, so the look snapshot takes the
    // movement.
    assert_eq!(workbench.interaction_snapshot_scroll(), 0);
    apply(&mut workbench, Action::NavigateInteractionUp);
    apply(&mut workbench, Action::NavigateInteractionUp);
    assert_eq!(workbench.interaction_snapshot_scroll(), 2);
    apply(&mut workbench, Action::NavigateInteractionDown);
    assert_eq!(workbench.interaction_snapshot_scroll(), 1);
    // A draft claims the movement, and the snapshot stops taking it.
    apply(&mut workbench, Action::InsertRawwCharacter('a'));
    apply(&mut workbench, Action::InsertRawwCharacter('b'));
    apply(&mut workbench, Action::InsertRawwNewline);
    apply(&mut workbench, Action::InsertRawwCharacter('c'));
    apply(&mut workbench, Action::InsertRawwCharacter('d'));
    assert_eq!(workbench.raww_draft(), "ab\ncd");
    apply(&mut workbench, Action::NavigateInteractionUp);
    assert_eq!(workbench.interaction_snapshot_scroll(), 1);
    // The write cursor moved up a line, which the next insertion reveals.
    apply(&mut workbench, Action::InsertRawwCharacter('X'));
    assert_eq!(workbench.raww_draft(), "abX\ncd");
}

#[test]
fn every_keyboard_enhancement_outcome_names_the_portable_newline_binding() {
    // Ctrl+J is the one binding that holds regardless of the probe outcome, so
    // it belongs in all three reports rather than only the degraded ones.
    for enhancement in [
        KeyboardEnhancement::Active,
        KeyboardEnhancement::Unsupported,
        KeyboardEnhancement::ProbeFailed,
    ] {
        let lines = format_keyboard_enhancement_lines(enhancement);
        assert!(
            lines.iter().any(|line| line.contains("Ctrl+J")),
            "{enhancement:?} report omits the portable newline binding: {lines:?}"
        );
    }
}
