//! The operator-facing vocabulary and chord grammar a binding configuration is
//! written in.
//!
//! These read the crate's public surface only. That is the point of the
//! vocabulary: a configuration is written by someone who has not read the
//! source, and an external caller building its own binding reference reaches
//! the same names an operator does.

use crossterm::event::{KeyCode, KeyModifiers};

use agentmux::tui::{
    Action, BindingContext, ChordError, PrimaryModifier, default_binding, help_bindings,
    parse_chord, primary_modifier,
};

/// What a typing row renders as. It denotes typing rather than a keystroke, and
/// the rows behind it are deliberately outside the grammar.
const TYPING_PLACEHOLDER: &str = "Type";

/// Kebab-case: lowercase ASCII words joined by single hyphens, no leading,
/// trailing, or doubled separator.
fn is_kebab_case(name: &str) -> bool {
    !name.is_empty()
        && name
            .split('-')
            .all(|word| !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase()))
}

#[test]
fn every_configurable_action_and_context_has_one_distinct_name() {
    let mut names: Vec<&'static str> = Vec::new();
    for action in Action::ALL.iter().copied() {
        let Some(name) = action.configuration_name() else {
            continue;
        };
        assert!(is_kebab_case(name), "{action:?} names itself {name:?}");
        assert_eq!(
            Action::from_configuration_name(name),
            Some(action),
            "{name:?} did not resolve back to {action:?}"
        );
        names.push(name);
    }
    let before = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(before, names.len(), "two behaviors share a name");
    // Derived rather than chosen: every behavior is nameable except the ones
    // built from a typed character, so the count follows from the vocabulary
    // and stays correct as behaviors are added.
    let carrying = Action::ALL
        .iter()
        .filter(|action| action.carries_operator_input())
        .count();
    assert_eq!(
        before,
        Action::ALL.len() - carrying,
        "some behavior that takes no operator input went unnameable"
    );

    let mut contexts: Vec<&'static str> = Vec::new();
    for context in BindingContext::ALL {
        let name = context.configuration_name();
        assert!(is_kebab_case(name), "{context:?} names itself {name:?}");
        assert_eq!(
            BindingContext::from_configuration_name(name),
            Some(context),
            "{name:?} did not resolve back to {context:?}"
        );
        contexts.push(name);
    }
    let before = contexts.len();
    contexts.sort_unstable();
    contexts.dedup();
    assert_eq!(before, contexts.len(), "two contexts share a name");
}

/// A behavior built from the operator's own keystroke cannot be denoted by a
/// configuration row, which supplies a chord and a name and never a character.
/// The two declarations answering that question are independent, so this holds
/// them in agreement rather than trusting either alone.
#[test]
fn no_action_carrying_operator_input_is_nameable() {
    let mut carrying = 0;
    for action in Action::ALL.iter().copied() {
        assert_eq!(
            action.carries_operator_input(),
            action.configuration_name().is_none(),
            "{action:?} disagrees about whether it carries operator input"
        );
        if action.carries_operator_input() {
            carrying += 1;
        }
    }
    assert_eq!(
        carrying, 3,
        "the set of behaviors built from a typed character changed"
    );
}

/// An operator's first act is copying a chord out of the reference the TUI
/// renders. A grammar that does not accept what help shows fails exactly then,
/// so this walks the whole generated surface rather than sampling it.
#[test]
fn every_chord_help_presents_parses_back_to_itself() {
    let sections = help_bindings();
    assert!(!sections.is_empty(), "the generated help surface is empty");

    // Both the text drawn on a line and the rows folded out of it: a folded
    // chord is one an operator can still press and still wants to rebind, so
    // the grammar owes it the same acceptance as the one on screen.
    let mut chords: Vec<String> = Vec::new();
    for section in &sections {
        for entry in &section.entries {
            chords.extend(entry.chords.split(" / ").map(str::to_string));
            chords.extend(entry.sources.iter().map(|source| source.chord.clone()));
        }
    }
    chords.retain(|chord| chord != TYPING_PLACEHOLDER);
    chords.sort_unstable();
    chords.dedup();

    // Structural rather than a threshold: every heading the overlay draws must
    // contribute, which catches a section going missing without asserting a
    // count that ordinary edits would churn.
    for section in &sections {
        assert!(
            !section.entries.is_empty(),
            "the {:?} section presents nothing",
            section.heading
        );
    }
    assert!(
        chords.iter().any(|chord| chord == "Ctrl+C"),
        "the quit chord is not among the presented chords, so the surface is not whole"
    );
    for chord in &chords {
        let parsed = parse_chord(chord).unwrap_or_else(|error| {
            panic!("help presents {chord:?}, which does not parse: {error}")
        });
        assert_eq!(
            &parsed.render(),
            chord,
            "{chord:?} did not survive a parse and render"
        );
    }
}

#[test]
fn the_grammar_reads_modifiers_and_keys_an_operator_writes() {
    let literal = KeyModifiers::NONE;
    for (text, code, modifiers) in [
        ("Enter", KeyCode::Enter, KeyModifiers::NONE),
        ("Shift+Enter", KeyCode::Enter, KeyModifiers::SHIFT),
        ("Ctrl+Enter", KeyCode::Enter, KeyModifiers::CONTROL),
        ("Esc", KeyCode::Esc, KeyModifiers::NONE),
        ("F5", KeyCode::F(5), KeyModifiers::NONE),
        ("PgDn", KeyCode::PageDown, KeyModifiers::NONE),
        ("Space", KeyCode::Char(' '), KeyModifiers::NONE),
        ("Ctrl+C", KeyCode::Char('c'), KeyModifiers::CONTROL),
    ] {
        let parsed = parse_chord(text).unwrap_or_else(|error| panic!("{text:?}: {error}"));
        assert!(!parsed.uses_primary_modifier(), "{text:?} is not symbolic");
        assert_eq!(parsed.resolve(literal), (code, modifiers), "{text:?}");
    }
}

/// `Ctrl+C` reaches the process as a lowercase character, and the table stores
/// it that way, while the conventional spelling capitalizes it. Both directions
/// have to agree or a chord copied from help stops matching the key that was
/// pressed.
#[test]
fn a_control_chord_parses_to_the_character_a_terminal_reports() {
    let parsed = parse_chord("Ctrl+C").expect("Ctrl+C parses");
    assert_eq!(
        parsed.resolve(KeyModifiers::NONE),
        (KeyCode::Char('c'), KeyModifiers::CONTROL)
    );
    assert_eq!(parsed.render(), "Ctrl+C");
}

/// A single character is a binding in its own right, and case distinguishes two
/// of them, so it is the one part of the grammar read case-sensitively.
#[test]
fn character_keys_keep_their_case_while_modifiers_do_not() {
    let lower = parse_chord("c").expect("c parses");
    let upper = parse_chord("C").expect("C parses");
    assert_ne!(lower, upper);
    assert_eq!(lower.resolve(KeyModifiers::NONE).0, KeyCode::Char('c'));
    assert_eq!(upper.resolve(KeyModifiers::NONE).0, KeyCode::Char('C'));

    for spelling in ["ctrl+enter", "CTRL+Enter", "Ctrl+enter"] {
        assert_eq!(
            parse_chord(spelling).expect("modifier case is not significant"),
            parse_chord("Ctrl+Enter").expect("Ctrl+Enter parses"),
            "{spelling:?}"
        );
    }
}

#[test]
fn the_grammar_refuses_what_it_does_not_express() {
    assert_eq!(parse_chord(""), Err(ChordError::Empty));
    assert_eq!(parse_chord("   "), Err(ChordError::Empty));
    assert_eq!(parse_chord("Ctrl+"), Err(ChordError::Empty));
    assert_eq!(
        parse_chord("Hyper+Enter"),
        Err(ChordError::UnknownModifier("Hyper".to_string()))
    );
    assert_eq!(
        parse_chord("Ctrl+Ctrl+Enter"),
        Err(ChordError::RepeatedModifier("Ctrl".to_string()))
    );
    assert_eq!(
        parse_chord("Nonesuch"),
        Err(ChordError::UnknownKey("Nonesuch".to_string()))
    );
    // `Type` is what a typing row renders as. It names no key, and admitting it
    // would let a configuration rebind how characters are typed.
    assert_eq!(
        parse_chord("Type"),
        Err(ChordError::UnknownKey("Type".to_string()))
    );
}

/// `Shift+Tab` is the one key spelling that carries a modifier in the key half,
/// because that is how the back-tab row renders. It has to read back as the
/// representation a terminal reports -- `BackTab`, bare -- or the chord denotes
/// a keystroke that never arrives.
#[test]
fn the_back_tab_spelling_reads_as_the_key_a_terminal_reports() {
    for spelling in ["Shift+Tab", "shift+tab"] {
        let parsed = parse_chord(spelling).expect("the back-tab spelling parses");
        assert_eq!(
            parsed.resolve(KeyModifiers::NONE),
            (KeyCode::BackTab, KeyModifiers::NONE),
            "{spelling:?} resolved to a chord no keystroke satisfies"
        );
        assert_eq!(parsed.render(), "Shift+Tab");
    }
}

/// The point of the previous test, stated against the table rather than against
/// the parser: the chord an operator copies out of help has to reach the
/// behavior that chord is bound to.
#[test]
fn the_back_tab_spelling_resolves_against_a_compiled_row() {
    let parsed = parse_chord("Shift+Tab").expect("Shift+Tab parses");
    let (code, modifiers) = parsed.resolve(KeyModifiers::NONE);
    assert_eq!(
        default_binding(BindingContext::ComposeMessage, code, modifiers),
        Some(Action::CyclePreviousFocus),
        "a parsed Shift+Tab did not reach the row the table declares for it"
    );
}

#[test]
fn the_symbolic_modifier_stays_unresolved_until_a_platform_resolves_it() {
    let parsed = parse_chord("primary+Enter").expect("primary+Enter parses");
    assert!(parsed.uses_primary_modifier());
    assert_eq!(parsed.render(), "primary+Enter");

    assert_eq!(
        parsed.resolve(KeyModifiers::CONTROL),
        (KeyCode::Enter, KeyModifiers::CONTROL)
    );
    assert_eq!(
        parsed.resolve(KeyModifiers::SUPER),
        (KeyCode::Enter, KeyModifiers::SUPER)
    );
}

/// Off macOS there is no second application-command modifier to choose between,
/// so no selection governs the symbol there. On macOS the selection decides and
/// defaults to `Ctrl`, so a chord using the symbol is reachable without
/// depending on whether a terminal delivers `Cmd` chords at all.
#[test]
fn the_symbolic_modifier_resolves_per_platform() {
    assert_eq!(primary_modifier(false, None), KeyModifiers::CONTROL);
    assert_eq!(
        primary_modifier(false, Some(PrimaryModifier::Command)),
        KeyModifiers::CONTROL,
        "a macOS selection reached a platform it does not govern"
    );
    assert_eq!(
        primary_modifier(false, Some(PrimaryModifier::Control)),
        KeyModifiers::CONTROL
    );

    assert_eq!(
        primary_modifier(true, None),
        KeyModifiers::CONTROL,
        "the macOS default is not Ctrl"
    );
    assert_eq!(
        primary_modifier(true, Some(PrimaryModifier::Control)),
        KeyModifiers::CONTROL
    );
    assert_eq!(
        primary_modifier(true, Some(PrimaryModifier::Command)),
        KeyModifiers::SUPER
    );
}

/// Case folding follows the modifier that is actually in force, not the one the
/// symbol might have resolved to. A terminal reports `Ctrl+C` as a lowercase
/// character; it reports `Cmd+C` as the character that was typed. Folding at
/// resolution is what lets one written chord answer both.
#[test]
fn a_symbolic_chord_folds_case_only_where_it_resolved_to_control() {
    let parsed = parse_chord("primary+C").expect("primary+C parses");
    assert!(parsed.uses_primary_modifier());
    assert_eq!(parsed.render(), "primary+C");

    assert_eq!(
        parsed.resolve(KeyModifiers::CONTROL),
        (KeyCode::Char('c'), KeyModifiers::CONTROL),
        "a symbolic chord resolving to Ctrl did not fold to the reported character"
    );
    assert_eq!(
        parsed.resolve(KeyModifiers::SUPER),
        (KeyCode::Char('C'), KeyModifiers::SUPER),
        "a symbolic chord resolving to Cmd folded a character it should have left alone"
    );
}

/// The literal modifiers stay separately bindable on every platform, which is
/// what keeps the readline chords the default table declares correct on macOS
/// rather than being rewritten to the command modifier.
#[test]
fn literal_control_and_command_remain_distinct() {
    let control = parse_chord("Ctrl+Enter").expect("Ctrl+Enter parses");
    let command = parse_chord("Cmd+Enter").expect("Cmd+Enter parses");
    assert_ne!(control, command);
    assert_eq!(
        control.resolve(KeyModifiers::SUPER),
        (KeyCode::Enter, KeyModifiers::CONTROL),
        "a literal Ctrl binding followed the symbolic resolution"
    );
    assert_eq!(
        command.resolve(KeyModifiers::CONTROL),
        (KeyCode::Enter, KeyModifiers::SUPER)
    );
    // Resolution alone would not have caught a modifier the renderer dropped.
    assert_eq!(control.render(), "Ctrl+Enter");
    assert_eq!(command.render(), "Cmd+Enter");
}

/// Every modifier the grammar accepts has to survive rendering, or a configured
/// binding is presented as something an operator did not write -- and a command
/// chord would come back as a bare key.
#[test]
fn every_accepted_modifier_survives_a_round_trip() {
    for text in [
        "Cmd+Enter",
        "Cmd+C",
        "Ctrl+Cmd+Enter",
        "Alt+Enter",
        "Ctrl+Alt+Shift+Enter",
        "primary+Cmd+Enter",
    ] {
        let parsed = parse_chord(text).unwrap_or_else(|error| panic!("{text:?}: {error}"));
        assert_eq!(parsed.render(), text, "{text:?} did not survive rendering");
    }
}

/// A command chord carries no case folding, unlike a control chord: a terminal
/// reporting `Cmd+C` reports the character that was typed.
#[test]
fn a_command_character_chord_keeps_its_case() {
    let parsed = parse_chord("Cmd+C").expect("Cmd+C parses");
    assert_eq!(
        parsed.resolve(KeyModifiers::NONE),
        (KeyCode::Char('C'), KeyModifiers::SUPER)
    );
    assert_eq!(parsed.render(), "Cmd+C");
}
