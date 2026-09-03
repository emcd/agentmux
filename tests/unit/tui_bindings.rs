//! The TUI's default binding table and the context a chord resolves against.
//!
//! These read the table through the public surface — `BindingContext` plus
//! `default_binding` — rather than by inspecting rows, so the assertions hold
//! whatever shape the rows take internally.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyModifiers};

use agentmux::tui::{
    Action, BindingContext, TuiLaunchOptions, default_binding,
    workbench::{Workbench, WorkbenchField, WorkbenchPickerColumn},
};

fn binding_workbench() -> Workbench {
    Workbench::new(TuiLaunchOptions {
        namespace: "agentmux".to_string(),
        sender_session: "tui".to_string(),
        relay_socket: PathBuf::from("/tmp/agentmux-binding-test-relay.sock"),
        look_lines: None,
        available_bundles: vec!["agentmux".to_string()],
    })
}

fn apply(workbench: &mut Workbench, action: Action) {
    workbench
        .apply_action(action)
        .unwrap_or_else(|error| panic!("apply {action:?}: {error}"));
}

fn control(character: char) -> (KeyCode, KeyModifiers) {
    (KeyCode::Char(character), KeyModifiers::CONTROL)
}

/// Resolves a chord the way dispatch will: the global rows first, then the
/// surface the workbench is on.
fn resolve_here(workbench: &Workbench, code: KeyCode, modifiers: KeyModifiers) -> Option<Action> {
    workbench
        .binding_lookup_order()
        .into_iter()
        .find_map(|context| default_binding(context, code, modifiers))
}

#[test]
fn every_context_is_listed_once_in_declaration_order() {
    // `position` matches on every variant, so a context added later fails to
    // compile until it is placed in `ALL` too. This pins the two together.
    for (index, context) in BindingContext::ALL.iter().enumerate() {
        assert_eq!(context.position(), index, "{context:?} is misplaced in ALL");
    }
}

#[test]
fn the_active_binding_context_follows_the_open_surface_then_the_focused_field() {
    let mut workbench = binding_workbench();
    workbench.set_focus(WorkbenchField::To);
    assert_eq!(workbench.binding_context(), BindingContext::ComposeTo);
    workbench.set_focus(WorkbenchField::Message);
    assert_eq!(workbench.binding_context(), BindingContext::ComposeMessage);

    apply(&mut workbench, Action::OpenPicker);
    assert_eq!(workbench.picker_column(), WorkbenchPickerColumn::Sessions);
    assert_eq!(workbench.binding_context(), BindingContext::PickerSessions);
    apply(&mut workbench, Action::TogglePickerFocus);
    assert_eq!(workbench.binding_context(), BindingContext::PickerBundles);
    apply(&mut workbench, Action::ClosePicker);

    apply(&mut workbench, Action::ToggleEventsOverlay);
    assert_eq!(workbench.binding_context(), BindingContext::EventsOverlay);
    apply(&mut workbench, Action::ToggleHelpOverlay);
    assert_eq!(workbench.binding_context(), BindingContext::HelpOverlay);
    apply(&mut workbench, Action::ToggleHelpOverlay);

    apply(&mut workbench, Action::ToggleMode);
    apply(&mut workbench, Action::ClosePicker);
    assert_eq!(
        workbench.binding_context(),
        BindingContext::InteractionWrite
    );
    workbench.set_interaction_target("alpha");
    workbench.inject_pending_choice("alpha");
    assert_eq!(
        workbench.binding_context(),
        BindingContext::InteractionChoice
    );
    // A draft returns the surface to the write input, choices still pending.
    apply(&mut workbench, Action::InsertRawwCharacter('x'));
    assert_eq!(
        workbench.binding_context(),
        BindingContext::InteractionWrite
    );

    assert_ne!(
        workbench.binding_context(),
        BindingContext::Global,
        "the contextual owner is a surface, never the global rows"
    );
}

#[test]
fn the_global_rows_lead_the_lookup_whatever_surface_is_open() {
    // A globally bound chord must not be shadowed by an open surface. Dispatch
    // reads this order rather than testing those chords ahead of the table,
    // which would put a chord's action in a second place.
    let mut workbench = binding_workbench();
    let surfaces = [
        (None, BindingContext::ComposeTo),
        (Some(Action::OpenPicker), BindingContext::PickerSessions),
        (
            Some(Action::OpenBundlePicker),
            BindingContext::PickerBundles,
        ),
        (
            Some(Action::ToggleEventsOverlay),
            BindingContext::EventsOverlay,
        ),
        (Some(Action::ToggleHelpOverlay), BindingContext::HelpOverlay),
    ];
    for (opener, surface) in surfaces {
        if let Some(opener) = opener {
            apply(&mut workbench, opener);
        }
        assert_eq!(
            workbench.binding_lookup_order(),
            [BindingContext::Global, surface],
            "global rows must lead the lookup with {surface:?} active"
        );
    }
}

#[test]
fn the_global_chords_keep_their_action_with_any_surface_open() {
    // `handle_key` tests Ctrl+C and F1 ahead of every overlay today. That reach
    // has to survive as table rows rather than as an early return, or a chord's
    // action ends up declared in a second place.
    let mut workbench = binding_workbench();
    let openers = [
        None,
        Some(Action::OpenPicker),
        Some(Action::OpenBundlePicker),
        Some(Action::ToggleEventsOverlay),
        Some(Action::ToggleHelpOverlay),
    ];
    for opener in openers {
        if let Some(opener) = opener {
            apply(&mut workbench, opener);
        }
        let surface = workbench.binding_context();
        let (code, modifiers) = control('c');
        assert_eq!(
            resolve_here(&workbench, code, modifiers),
            Some(Action::Quit),
            "Ctrl+C must still quit with {surface:?} active"
        );
        assert_eq!(
            resolve_here(&workbench, KeyCode::F(1), KeyModifiers::NONE),
            Some(Action::ToggleHelpOverlay),
            "F1 must still reach help with {surface:?} active"
        );
    }
}

#[test]
fn enter_carrying_other_modifiers_keeps_reaching_its_handler_action() {
    // The interaction and picker arms match `KeyCode::Enter` whatever the
    // modifiers, so Alt+Enter acts there today. The three explicit rows own the
    // modifier sets capability neutrality governs; a fallback row after them
    // keeps the rest reaching the same action rather than silently going inert.
    let alt = KeyModifiers::ALT;
    let combined = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
    for (context, action) in [
        (BindingContext::InteractionWrite, Action::DispatchRaww),
        (
            BindingContext::InteractionChoice,
            Action::ResolveChoiceSelected,
        ),
        (BindingContext::PickerBundles, Action::CommitPickerBundle),
        (BindingContext::PickerSessions, Action::CommitPickerSession),
    ] {
        assert_eq!(
            default_binding(context, KeyCode::Enter, alt),
            Some(action),
            "{context:?} drops Alt+Enter its handler accepts"
        );
        assert_eq!(
            default_binding(context, KeyCode::Enter, combined),
            Some(action),
            "{context:?} drops Ctrl+Shift+Enter its handler accepts"
        );
    }
    // Compose guarded on an empty modifier set, so it keeps rejecting the rest.
    assert_eq!(
        default_binding(BindingContext::ComposeMessage, KeyCode::Enter, alt),
        None
    );
    assert_eq!(
        default_binding(BindingContext::ComposeTo, KeyCode::Enter, alt),
        None
    );
}

#[test]
fn a_control_chord_matches_however_the_modifiers_are_combined() {
    // The control blocks test `modifiers.contains(CONTROL)`, so Ctrl+Shift+J
    // reaches the same behavior as Ctrl+J today.
    let combined = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
    assert_eq!(
        default_binding(BindingContext::ComposeMessage, KeyCode::Char('j'), combined),
        Some(Action::InsertMessageNewline)
    );
    assert_eq!(
        default_binding(
            BindingContext::InteractionWrite,
            KeyCode::Char('r'),
            combined
        ),
        Some(Action::RefreshRecipients)
    );
    assert_eq!(
        default_binding(BindingContext::Global, KeyCode::Char('c'), combined),
        Some(Action::Quit)
    );
    // Without Ctrl the same character is ordinary typing.
    assert_eq!(
        default_binding(
            BindingContext::ComposeMessage,
            KeyCode::Char('j'),
            KeyModifiers::NONE
        ),
        Some(Action::InsertComposeCharacter('j'))
    );
}

#[test]
fn each_context_keeps_the_enter_action_its_handler_had() {
    let bare = |context| default_binding(context, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(
        bare(BindingContext::ComposeTo),
        Some(Action::AcceptToCompletion)
    );
    assert_eq!(
        bare(BindingContext::ComposeMessage),
        Some(Action::SendMessage)
    );
    assert_eq!(
        bare(BindingContext::InteractionWrite),
        Some(Action::DispatchRaww)
    );
    assert_eq!(
        bare(BindingContext::InteractionChoice),
        Some(Action::ResolveChoiceSelected)
    );
    assert_eq!(
        bare(BindingContext::PickerBundles),
        Some(Action::CommitPickerBundle)
    );
    assert_eq!(
        bare(BindingContext::PickerSessions),
        Some(Action::CommitPickerSession)
    );
    // The overlays never bound Enter, and the global rows own no Enter either.
    assert_eq!(bare(BindingContext::EventsOverlay), None);
    assert_eq!(bare(BindingContext::HelpOverlay), None);
    assert_eq!(bare(BindingContext::Global), None);
}

#[test]
fn every_context_binding_enter_also_binds_both_modified_enters() {
    for context in BindingContext::ALL {
        let bare = default_binding(context, KeyCode::Enter, KeyModifiers::NONE);
        let shifted = default_binding(context, KeyCode::Enter, KeyModifiers::SHIFT);
        let controlled = default_binding(context, KeyCode::Enter, KeyModifiers::CONTROL);
        if bare.is_none() {
            assert!(
                shifted.is_none() && controlled.is_none(),
                "{context:?} binds a modified Enter without binding Enter itself"
            );
            continue;
        }
        assert!(
            shifted.is_some(),
            "{context:?} declares Enter but leaves Shift+Enter to be inherited"
        );
        assert!(
            controlled.is_some(),
            "{context:?} declares Enter but leaves Ctrl+Enter to be inherited"
        );
    }
}

#[test]
fn modified_enter_resolves_to_the_same_action_as_bare_enter_in_every_context() {
    // Capability neutrality, asserted over the whole table rather than context
    // by context: a row added later cannot reintroduce a chord that only works
    // on terminals which disambiguate modified keys.
    for context in BindingContext::ALL {
        let bare = default_binding(context, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            default_binding(context, KeyCode::Enter, KeyModifiers::SHIFT),
            bare,
            "{context:?} gives Shift+Enter an action its Enter does not have"
        );
        assert_eq!(
            default_binding(context, KeyCode::Enter, KeyModifiers::CONTROL),
            bare,
            "{context:?} gives Ctrl+Enter an action its Enter does not have"
        );
    }
}

#[test]
fn shift_enter_in_the_message_field_sends_the_message() {
    // The regression the keyboard-enhancement probe introduced: activating
    // disambiguation made a modified Enter reach no binding in compose, so
    // Shift+Enter stopped sending on capable terminals. Asserted directly
    // rather than left to follow from the whole-table neutrality check.
    assert_eq!(
        default_binding(
            BindingContext::ComposeMessage,
            KeyCode::Enter,
            KeyModifiers::SHIFT
        ),
        Some(Action::SendMessage)
    );
    assert_eq!(
        default_binding(
            BindingContext::ComposeMessage,
            KeyCode::Enter,
            KeyModifiers::CONTROL
        ),
        Some(Action::SendMessage)
    );
}

#[test]
fn control_j_inserts_a_newline_exactly_in_the_contexts_that_own_a_text_draft() {
    let (code, modifiers) = control('j');
    assert_eq!(
        default_binding(BindingContext::ComposeMessage, code, modifiers),
        Some(Action::InsertMessageNewline)
    );
    assert_eq!(
        default_binding(BindingContext::InteractionWrite, code, modifiers),
        Some(Action::InsertRawwNewline)
    );
    // The choice pane forwards typing into the write draft, and its Ctrl+J does
    // the same today, so the row is declared there rather than dropped.
    assert_eq!(
        default_binding(BindingContext::InteractionChoice, code, modifiers),
        Some(Action::InsertRawwNewline)
    );
    for context in BindingContext::ALL {
        if matches!(
            context,
            BindingContext::ComposeMessage
                | BindingContext::InteractionWrite
                | BindingContext::InteractionChoice
        ) {
            continue;
        }
        assert_eq!(
            default_binding(context, code, modifiers),
            None,
            "{context:?} owns no text draft and must not bind a newline"
        );
    }
}

#[test]
fn a_character_row_outranks_the_contexts_typing_row() {
    // Declaration order is the tiebreak, so the cancel chord has to be declared
    // before the row that sends any character to the draft.
    assert_eq!(
        default_binding(
            BindingContext::InteractionChoice,
            KeyCode::Char('c'),
            KeyModifiers::NONE
        ),
        Some(Action::ResolveChoiceCancelled)
    );
    assert_eq!(
        default_binding(
            BindingContext::InteractionChoice,
            KeyCode::Char('C'),
            KeyModifiers::SHIFT
        ),
        Some(Action::ResolveChoiceCancelled)
    );
    assert_eq!(
        default_binding(
            BindingContext::InteractionChoice,
            KeyCode::Char('x'),
            KeyModifiers::NONE
        ),
        Some(Action::InsertRawwCharacter('x'))
    );
    // The same character is ordinary typing where no row claims it.
    assert_eq!(
        default_binding(
            BindingContext::InteractionWrite,
            KeyCode::Char('c'),
            KeyModifiers::NONE
        ),
        Some(Action::InsertRawwCharacter('c'))
    );
    // A character carrying Ctrl is not typing, so it reaches no row at all.
    assert_eq!(
        default_binding(
            BindingContext::InteractionWrite,
            KeyCode::Char('x'),
            KeyModifiers::CONTROL
        ),
        None
    );
}

#[test]
fn the_viewport_chords_reach_the_help_overlay_and_nothing_else() {
    // The overlay presents more than a short terminal shows, so it is drawn
    // through a viewport. The chords that move it are declared like any other
    // binding — reachable through the table, and reachable there only. A scroll
    // action leaking into another context would move a viewport that context
    // does not have.
    let viewport = [
        (KeyCode::Up, Action::ScrollHelpUp),
        (KeyCode::Down, Action::ScrollHelpDown),
        (KeyCode::PageUp, Action::ScrollHelpPageUp),
        (KeyCode::PageDown, Action::ScrollHelpPageDown),
        (KeyCode::Home, Action::ScrollHelpToStart),
        (KeyCode::End, Action::ScrollHelpToEnd),
    ];
    for (code, action) in viewport {
        assert_eq!(
            default_binding(BindingContext::HelpOverlay, code, KeyModifiers::NONE),
            Some(action),
            "the help overlay does not bind {code:?}"
        );
    }
    let scroll_actions: Vec<Action> = viewport.iter().map(|(_, action)| *action).collect();
    for context in BindingContext::ALL {
        if context == BindingContext::HelpOverlay {
            continue;
        }
        for (code, _) in viewport {
            let resolved = default_binding(context, code, KeyModifiers::NONE);
            assert!(
                resolved.is_none_or(|action| !scroll_actions.contains(&action)),
                "{context:?} resolves {code:?} to {resolved:?}, a help-overlay viewport action"
            );
        }
    }
    // Dismissal is not shadowed by the rows added above it.
    assert_eq!(
        default_binding(
            BindingContext::HelpOverlay,
            KeyCode::Esc,
            KeyModifiers::NONE
        ),
        Some(Action::ToggleHelpOverlay)
    );
}
