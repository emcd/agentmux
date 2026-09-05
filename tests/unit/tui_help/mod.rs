//! Generated help presentation.
//!
//! The regression these guard against is a help overlay that shows only what
//! the current surface binds. That would be the natural mistake, because
//! dispatch already has a function answering "which context owns a chord right
//! now" and reusing it here would compile and look right from whichever
//! surface the author happened to test from.
//!
//! - [`effective`]: what a configuration does to the catalogue and the strips,
//!   which is the other way presentation can go wrong — agreeing with no
//!   lookup rather than covering no surface.
//!
//! Helpers both modules need live in this hub.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyModifiers};

use agentmux::tui::{
    Action, BindingConfiguration, BindingContext, ConfiguredAction, ConfiguredBinding,
    EffectiveBindings, HelpSection, TuiLaunchOptions, default_binding, help_bindings,
    interaction_choice_hint, interaction_write_hint, parse_chord, picker_hint,
    workbench::{Workbench, WorkbenchField},
};

mod effective;

fn help_workbench() -> Workbench {
    workbench_with(None)
}

fn workbench_with(bindings: Option<BindingConfiguration>) -> Workbench {
    Workbench::new(TuiLaunchOptions {
        namespace: "agentmux".to_string(),
        sender_session: "tui".to_string(),
        relay_socket: PathBuf::from("/tmp/agentmux-help-test-relay.sock"),
        look_lines: None,
        available_bundles: vec!["agentmux".to_string()],
        bindings,
    })
}

/// The table an unconfigured run produces: the compiled defaults alone.
fn defaults() -> EffectiveBindings {
    EffectiveBindings::default()
}

/// One row, speaking for both capability classes as an unqualified row in a
/// file does.
fn row(context: BindingContext, chord: &str, action: Action) -> ConfiguredBinding {
    let configured = ConfiguredAction::Invoke(action);
    ConfiguredBinding {
        context,
        chord: parse_chord(chord).expect("the fixture chord parses"),
        enhanced: Some(configured),
        standard: Some(configured),
    }
}

/// A binding group holding one row, for the cases that ask what a
/// configuration does to presentation.
fn one_row(context: BindingContext, chord: &str, action: Action) -> BindingConfiguration {
    BindingConfiguration {
        presets: Vec::new(),
        primary_modifier_on_macos: None,
        rows: vec![row(context, chord, action)],
    }
}

/// Every behavior the catalogue presents the given chord under.
///
/// A list rather than an `Option`, because the defect these cases guard is a
/// chord advertised under *two* behaviors when it reaches one.
fn presented_under(sections: &[HelpSection], chord: &str) -> Vec<&'static str> {
    sections
        .iter()
        .flat_map(|section| section.entries.iter())
        .filter(|entry| entry.chords.split(" / ").any(|shown| shown == chord))
        .map(|entry| entry.description)
        .collect()
}

/// Every line the overlay would print, as `chords: description`.
fn presented_lines(sections: &[HelpSection]) -> Vec<String> {
    sections
        .iter()
        .flat_map(|section| {
            section
                .entries
                .iter()
                .map(|entry| format!("{}: {}", entry.chords, entry.description))
        })
        .collect()
}

/// A named way to put a workbench on one surface, so the arrangements read as
/// a list of surfaces rather than as a tuple of function pointers.
struct Surface {
    name: &'static str,
    arrange: fn(&mut Workbench),
}

fn arrange_compose_message(workbench: &mut Workbench) {
    workbench.set_focus(WorkbenchField::Message);
}

fn arrange_interaction_write(workbench: &mut Workbench) {
    workbench.set_recipients(&["master"]);
    workbench.set_interaction_target("master");
    let _ = workbench.apply_action(Action::ToggleMode);
}

fn arrange_interaction_choice(workbench: &mut Workbench) {
    arrange_interaction_write(workbench);
    workbench.inject_pending_choice("master");
}

fn arrange_picker(workbench: &mut Workbench) {
    workbench.set_recipients(&["master"]);
    let _ = workbench.apply_action(Action::OpenPicker);
}

fn arrange_bundle_picker(workbench: &mut Workbench) {
    let _ = workbench.apply_action(Action::OpenBundlePicker);
}

fn arrange_events_overlay(workbench: &mut Workbench) {
    let _ = workbench.apply_action(Action::ToggleEventsOverlay);
}

fn arrange_help_overlay(workbench: &mut Workbench) {
    let _ = workbench.apply_action(Action::ToggleHelpOverlay);
}

#[test]
fn the_help_catalogue_is_identical_whichever_surface_it_is_opened_from() {
    // The assertion task 4.2 names, made where it can fail: through the
    // workbench, whose state a context-filtered implementation would have to
    // read. Comparing the catalogue against itself would prove nothing.
    let surfaces = [
        Surface {
            name: "compose to",
            arrange: |_| {},
        },
        Surface {
            name: "compose message",
            arrange: arrange_compose_message,
        },
        Surface {
            name: "interaction write",
            arrange: arrange_interaction_write,
        },
        Surface {
            name: "interaction choice",
            arrange: arrange_interaction_choice,
        },
        Surface {
            name: "picker sessions",
            arrange: arrange_picker,
        },
        Surface {
            name: "picker bundles",
            arrange: arrange_bundle_picker,
        },
        Surface {
            name: "events overlay",
            arrange: arrange_events_overlay,
        },
    ];

    let mut baseline_workbench = help_workbench();
    arrange_help_overlay(&mut baseline_workbench);
    let baseline = baseline_workbench.help_bindings();
    assert!(
        !baseline.is_empty(),
        "the catalogue is empty, so equality across surfaces would be vacuous"
    );

    for surface in surfaces {
        let mut workbench = help_workbench();
        (surface.arrange)(&mut workbench);
        assert_eq!(
            workbench.help_bindings(),
            baseline,
            "help differs when opened from {}",
            surface.name
        );
    }

    // And the same catalogue the overlay renders is what a host reads without
    // a workbench at all.
    assert_eq!(help_bindings(&defaults()), baseline);
}

#[test]
fn the_help_catalogue_carries_the_compose_interaction_and_picker_bindings() {
    // One binding from each of the three surfaces a context-filtered help would
    // have dropped, named concretely rather than counted.
    let lines = presented_lines(&help_bindings(&defaults()));
    for expected in [
        "Enter: Message: send",
        "Ctrl+J: Message: insert newline",
        "Ctrl+U: To: clear field",
        "Enter: Write: dispatch to active target",
        "Enter: Choice: resolve selected option",
        "c / C: Choice: resolve as cancelled",
        "Esc / F2 / F5: Close picker",
        "Enter: Bundle col: switch bundle",
        "Ctrl+C: Quit from anywhere",
    ] {
        assert!(
            lines.iter().any(|line| line == expected),
            "help is missing {expected:?}; it presents {lines:#?}"
        );
    }
}

#[test]
fn the_sections_are_presented_in_declaration_order() {
    let headings = help_bindings(&defaults())
        .into_iter()
        .map(|section| section.heading)
        .collect::<Vec<_>>();
    assert_eq!(
        headings,
        vec![
            "Modes",
            "Communication Mode",
            "Interaction Mode",
            "Picker",
            "Help Overlay"
        ]
    );
}

/// Every key code and modifier set worth resolving against the table.
fn chord_space() -> Vec<(KeyCode, KeyModifiers)> {
    let mut codes = vec![
        KeyCode::Enter,
        KeyCode::Esc,
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Backspace,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
    ];
    for number in 1..=5u8 {
        codes.push(KeyCode::F(number));
    }
    for character in "abcdefghijklmnopqrstuvwxyz C".chars() {
        codes.push(KeyCode::Char(character));
    }

    let mut chords = Vec::new();
    for code in codes {
        for modifiers in [
            KeyModifiers::NONE,
            KeyModifiers::SHIFT,
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
        ] {
            chords.push((code, modifiers));
        }
    }
    chords
}

fn entries(sections: &[HelpSection]) -> Vec<&agentmux::tui::HelpEntry> {
    sections
        .iter()
        .flat_map(|section| section.entries.iter())
        .collect()
}

#[test]
fn every_context_that_binds_anything_contributes_to_the_help() {
    // The gap a description-only check leaves open. Most contexts bind
    // behaviors that some other context also binds, so dropping one from
    // `help_contexts` loses its chords while every description still appears.
    // Provenance is what makes the loss visible.
    let sections = help_bindings(&defaults());
    let entries = entries(&sections);

    for context in BindingContext::ALL {
        let binds_anything = chord_space()
            .into_iter()
            .any(|(code, modifiers)| default_binding(context, code, modifiers).is_some());
        if !binds_anything {
            continue;
        }
        assert!(
            entries.iter().any(|entry| entry.covers(context)),
            "{context:?} binds chords that help never presents"
        );
    }
}

#[test]
fn every_binding_the_table_resolves_is_presented_by_the_row_that_answers_it() {
    // Completeness against the table rather than against a fixture copying it:
    // a row added to a context but skipped by generation fails here without
    // anyone extending a list.
    //
    // Matching on the source's own pattern, not on context and behavior. Two
    // rows in one context can reach the same behavior -- the events overlay
    // binds both Esc and F3 to toggling itself -- so a check satisfied by
    // "some source from this context describes this action" is satisfied by
    // the surviving sibling when one of them is dropped.
    let sections = help_bindings(&defaults());
    let entries = entries(&sections);

    for context in BindingContext::ALL {
        for (code, modifiers) in chord_space() {
            let Some(action) = default_binding(context, code, modifiers) else {
                continue;
            };
            let description = action.describe();
            assert!(
                entries.iter().any(|entry| {
                    entry.description == description
                        && entry.sources.iter().any(|source| {
                            source.context == context && source.matches(code, modifiers)
                        })
                }),
                "{context:?} resolves {code:?}+{modifiers:?} to {action:?}, and no source \
                 of that behavior in that context answers that chord"
            );
        }
    }
}

#[test]
fn every_folded_chord_is_covered_by_one_that_is_shown() {
    // A chord may be dropped from the printing only because the line already
    // says it, or because it is a modified `Enter` matching the bare one. Any
    // other omission is a binding the operator cannot discover.
    for entry in entries(&help_bindings(&defaults())) {
        let shown = entry
            .sources
            .iter()
            .filter(|source| source.shown)
            .map(|source| source.chord.as_str())
            .collect::<Vec<_>>();
        for folded in entry.sources.iter().filter(|source| !source.shown) {
            let duplicate = shown.contains(&folded.chord.as_str());
            let modified_enter = matches!(folded.chord.as_str(), "Shift+Enter" | "Ctrl+Enter")
                && shown.contains(&"Enter");
            assert!(
                duplicate || modified_enter,
                "{:?} folds {:?} out of {:?} under no stated rule",
                folded.context,
                folded.chord,
                entry.chords
            );
        }
        assert_eq!(
            entry.chords,
            shown.join(" / "),
            "the displayed chords disagree with the sources marked shown"
        );
    }
}

#[test]
fn a_modifier_agnostic_enter_is_bound_only_where_the_handlers_had_one() {
    // The reference note distinguishes two behaviors, so both are pinned. The
    // interaction panes and the picker carry a modifier-agnostic fallback row,
    // so Alt+Enter reaches their Enter action; compose guards on an empty
    // modifier set and deliberately does not.
    for context in [
        BindingContext::InteractionWrite,
        BindingContext::InteractionChoice,
        BindingContext::PickerBundles,
        BindingContext::PickerSessions,
    ] {
        assert_eq!(
            default_binding(context, KeyCode::Enter, KeyModifiers::ALT),
            default_binding(context, KeyCode::Enter, KeyModifiers::NONE),
            "{context:?} should reach its Enter action under any modifier"
        );
    }
    for context in [BindingContext::ComposeTo, BindingContext::ComposeMessage] {
        assert!(
            default_binding(context, KeyCode::Enter, KeyModifiers::ALT).is_none(),
            "{context:?} binds no modifier-agnostic Enter fallback"
        );
        assert!(
            default_binding(context, KeyCode::Enter, KeyModifiers::SHIFT).is_some(),
            "{context:?} still binds the three explicit Enter chords"
        );
    }
}

#[test]
fn a_modified_enter_is_folded_into_the_bare_one_it_matches() {
    // Capability-neutral defaults make the modified forms redundant on any line
    // that already shows Enter, and spelling all three out tripled the width of
    // the lines carrying them. They stay resolvable; they are just not printed.
    let lines = presented_lines(&help_bindings(&defaults()));
    for line in &lines {
        assert!(
            !line.contains("Shift+Enter") && !line.contains("Ctrl+Enter"),
            "a modified Enter is spelled out in {line:?}"
        );
    }
    assert_eq!(
        default_binding(
            BindingContext::ComposeMessage,
            KeyCode::Enter,
            KeyModifiers::SHIFT
        ),
        Some(Action::SendMessage),
        "folding it out of the presentation must not unbind it"
    );
}

#[test]
fn a_hint_strip_presents_only_the_context_it_annotates() {
    // The asymmetry with the help overlay, pinned rather than left to a
    // reviewer's memory: help catalogues every surface, a strip annotates the
    // one it sits on. Composing a strip from `help_bindings` would compile and
    // look plausible, and would advertise compose bindings on the write pane.
    for source in picker_hint(&defaults(), BindingContext::PickerSessions)
        .iter()
        .flat_map(|entry| entry.sources.iter())
    {
        assert!(
            matches!(
                source.context,
                BindingContext::PickerBundles | BindingContext::PickerSessions
            ),
            "the picker strip advertises {:?} from {:?}",
            source.chord,
            source.context
        );
    }
    for source in interaction_write_hint(&defaults())
        .iter()
        .flat_map(|entry| entry.sources.iter())
    {
        assert_eq!(
            source.context,
            BindingContext::InteractionWrite,
            "the write pane strip advertises {:?} from {:?}",
            source.chord,
            source.context
        );
    }

    for source in interaction_choice_hint(&defaults())
        .iter()
        .flat_map(|entry| entry.sources.iter())
    {
        assert_eq!(
            source.context,
            BindingContext::InteractionChoice,
            "the choice pane strip advertises {:?} from {:?}",
            source.chord,
            source.context
        );
    }

    // And each strip actually says something, so the assertions above are not
    // satisfied by an empty strip. The choice pane advertises fewer: it prints
    // its bindings in a block title, which does not wrap, so it carries the two
    // decisions and leaves navigation to the help overlay.
    assert!(picker_hint(&defaults(), BindingContext::PickerSessions).len() >= 3);
    assert!(interaction_write_hint(&defaults()).len() >= 3);
    assert!(interaction_choice_hint(&defaults()).len() >= 2);
}

#[test]
fn the_chord_a_hint_prints_resolves_to_the_behavior_it_names() {
    // Not that the chord resolves -- it is built from a row of that context, so
    // it does by construction -- but that it resolves to *this* behavior. A row
    // shadowed by an earlier one in the same context would still appear as a
    // source while its chord reached something else, and the strip would then
    // advertise a key that does the wrong thing.
    for entry in picker_hint(&defaults(), BindingContext::PickerSessions)
        .into_iter()
        .chain(interaction_write_hint(&defaults()))
        .chain(interaction_choice_hint(&defaults()))
    {
        let printed = entry.primary_chord();
        let source = entry
            .sources
            .iter()
            .find(|source| source.shown && source.chord == printed)
            .unwrap_or_else(|| panic!("no source backs the printed chord {printed:?}"));
        let resolved = chord_space()
            .into_iter()
            .filter(|(code, modifiers)| source.matches(*code, *modifiers))
            .filter_map(|(code, modifiers)| default_binding(source.context, code, modifiers))
            .next()
            .unwrap_or_else(|| panic!("{:?} resolves nothing for {printed:?}", source.context));
        assert_eq!(
            resolved.describe(),
            entry.description,
            "{:?} advertises {printed:?} as {:?}, but that chord resolves to {resolved:?}",
            source.context,
            entry.description
        );
    }
}
