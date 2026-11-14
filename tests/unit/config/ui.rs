use agentmux::configuration::ConfigurationRoots;
use std::fs;

use tempfile::TempDir;

use agentmux::configuration::{
    ConfigurationError, UiConfiguration, embedded_base_preset,
    embedded_binding_preset, load_ui_configuration, shipped_binding_presets,
};
use agentmux::tui::{
    Action, BindingContext, ConfiguredAction, PrimaryModifier,
    default_binding, parse_chord,
};
use crossterm::event::{KeyCode, KeyModifiers};

#[test]
fn loads_default_bundle_from_ui_configuration() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");
    fs::write(
        root.join("ui.toml"),
        r#"
default-bundle = "agentmux"
"#,
    )
    .expect("write ui.toml");

    let loaded = load_ui_configuration(&ConfigurationRoots::single(&root))
        .expect("load ui configuration")
        .expect("existing config");
    assert_eq!(loaded.default_bundle.as_deref(), Some("agentmux"));
}

#[test]
fn ignores_missing_ui_configuration() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");

    let loaded = load_ui_configuration(&ConfigurationRoots::single(&root))
        .expect("load ui config");
    assert!(loaded.is_none(), "missing file should be ignored");
}

#[test]
fn empty_ui_configuration_resolves_no_default_bundle() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");
    fs::write(root.join("ui.toml"), "").expect("write ui.toml");

    let loaded = load_ui_configuration(&ConfigurationRoots::single(&root))
        .expect("load ui config")
        .expect("existing config");
    assert!(loaded.default_bundle.is_none());
}

#[test]
fn rejects_malformed_ui_configuration() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");
    fs::write(root.join("ui.toml"), "default-bundle = ")
        .expect("write ui.toml");

    let error = load_ui_configuration(&ConfigurationRoots::single(&root))
        .expect_err("malformed ui.toml should fail");
    assert!(
        error.to_string().contains("ui.toml"),
        "error should name the offending file: {error}"
    );
}

/// Writes a `ui.toml` holding `body` and loads it.
fn load_bindings(
    body: &str,
) -> Result<Option<UiConfiguration>, ConfigurationError> {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");
    fs::write(root.join("ui.toml"), body).expect("write ui.toml");
    load_ui_configuration(&ConfigurationRoots::single(&root))
}

/// The shape the specification documents, character for character.
///
/// Its purpose is that the published shape cannot drift from what the loader
/// accepts: an edit to either that the other does not follow shows up here.
/// That only holds if this is the whole published text, so it is reproduced
/// complete, preset line included.
const DOCUMENTED_SHAPE: &str = r#"[bindings]
# Binding sets applied before individually configured rows, in the order named.
presets = ["enter-newline-primary-enter-sends"]
# Which literal modifier the symbolic `primary` resolves to on macOS.
primary-modifier-on-macos = "control"

# One sub-table per binding context, keyed by the context's operator name.
[bindings.compose-message]
# A chord mapped to one action applies on both terminal capability classes.
"ctrl+w" = "send-message"
# A chord mapped to a class-qualified table applies only where stated; the
# omitted class keeps its compiled default.
"shift+enter" = { enhanced = "insert-message-newline" }
# An explicitly unbound chord is inert, and does not fall through.
"ctrl+j" = "none"

[bindings.picker-sessions]
"primary+enter" = "commit-picker-session"
"#;

/// The documented shape, loaded whole and asserted row by row.
#[test]
fn the_documented_configuration_shape_loads_as_written() {
    let loaded = load_bindings(DOCUMENTED_SHAPE)
        .expect("the documented shape loads")
        .expect("existing config");
    let bindings = loaded.bindings.expect("a binding group");

    assert_eq!(
        bindings.primary_modifier_on_macos,
        Some(PrimaryModifier::Control)
    );
    assert_eq!(bindings.presets, vec!["enter-newline-primary-enter-sends"]);
    assert!(
        !bindings.preset_rows.is_empty(),
        "the named binding set contributed no rows"
    );
    assert_eq!(bindings.rows.len(), 4, "{:?}", bindings.rows);

    let row = |context, chord: &str| {
        bindings
            .rows
            .iter()
            .find(|row| {
                row.context == context
                    && row.chord == parse_chord(chord).expect("chord")
            })
            .unwrap_or_else(|| panic!("no row for {chord} in {context:?}"))
    };

    // A chord mapped to one action applies on both terminal capability classes.
    let both = row(BindingContext::ComposeMessage, "ctrl+w");
    assert_eq!(
        both.enhanced,
        Some(ConfiguredAction::Invoke(Action::SendMessage))
    );
    assert_eq!(both.standard, both.enhanced);

    // A class-qualified row speaks for one class and leaves the other alone.
    let qualified = row(BindingContext::ComposeMessage, "shift+enter");
    assert_eq!(
        qualified.enhanced,
        Some(ConfiguredAction::Invoke(Action::InsertMessageNewline))
    );
    assert_eq!(
        qualified.standard, None,
        "the omitted class should keep its compiled default"
    );

    // An explicit unbinding is a value, not an absence.
    let unbound = row(BindingContext::ComposeMessage, "ctrl+j");
    assert_eq!(unbound.enhanced, Some(ConfiguredAction::Unbound));
    assert_eq!(unbound.standard, Some(ConfiguredAction::Unbound));

    // The symbolic modifier survives loading unresolved.
    let symbolic = row(BindingContext::PickerSessions, "primary+enter");
    assert!(symbolic.chord.uses_primary_modifier());
    assert_eq!(
        symbolic.enhanced,
        Some(ConfiguredAction::Invoke(Action::CommitPickerSession))
    );
}

#[test]
fn a_ui_configuration_without_bindings_resolves_none() {
    let loaded = load_bindings("default-bundle = \"agentmux\"\n")
        .expect("load")
        .expect("existing config");
    assert!(
        loaded.bindings.is_none(),
        "an absent binding group is absence, not an empty configuration"
    );
}

#[test]
fn an_empty_binding_group_is_accepted_and_configures_nothing() {
    let loaded = load_bindings("[bindings]\n")
        .expect("load")
        .expect("existing config");
    let bindings = loaded.bindings.expect("a binding group");
    assert!(bindings.rows.is_empty());
    assert!(bindings.presets.is_empty());
    assert!(bindings.primary_modifier_on_macos.is_none());
}

/// Every way a binding group can be wrong, and the fragment of the message that
/// tells the operator which of them it was.
#[test]
fn an_invalid_binding_group_is_refused_with_its_reason() {
    for (body, expected) in [
        (
            "[bindings]\n[bindings.compose-message]\n\"ctrl+w\" = \"fly-to-the-moon\"\n",
            "unknown action",
        ),
        (
            "[bindings]\n[bindings.nowhere]\n\"ctrl+w\" = \"send-message\"\n",
            "unknown binding context",
        ),
        (
            "[bindings]\n[bindings.compose-message]\n\"Hyper+w\" = \"send-message\"\n",
            "unknown modifier",
        ),
        (
            "[bindings]\npresets = [\"no-such-binding-set\"]\n",
            "unknown binding preset",
        ),
        (
            "[bindings]\nprimary-modifier-on-macos = \"meta\"\n",
            "primary-modifier-on-macos must be control or command",
        ),
        (
            "[bindings]\n[bindings.not-a-context]\n\"ctrl+w\" = \"send-message\"\n",
            "unknown binding context",
        ),
        (
            "[bindings]\n[bindings.compose-message]\n\"ctrl+w\" = { enhanced = \"send-message\", unknown = \"x\" }\n",
            "unknown terminal class: unknown",
        ),
        (
            "[bindings]\n[bindings.compose-message]\n\"ctrl+w\" = 3\n",
            "not an action name or a table of them",
        ),
    ] {
        let error = load_bindings(body)
            .expect_err(&format!("expected refusal for {body:?}"));
        let rendered = error.to_string();
        assert!(
            rendered.contains(expected),
            "{body:?} reported {rendered:?}, which does not mention {expected:?}"
        );
    }
}

/// A key under `[bindings]` that is neither an option nor a context is refused
/// whatever shape its value takes. A scalar is caught by the shape check before
/// the context vocabulary is consulted, so it is refused for a different stated
/// reason than a table is — but refused, which is what matters: a misspelled key
/// that loaded silently would leave an operator believing a configuration is in
/// force that does nothing.
#[test]
fn an_unrecognized_key_is_refused_whatever_its_shape() {
    assert!(
        load_bindings("[bindings]\nnot-a-context = 3\n").is_err(),
        "a scalar under [bindings] was accepted"
    );
    assert!(
        load_bindings("[bindings]\nnot-a-context = { a = \"b\" }\n").is_err(),
        "a table under an unknown key was accepted"
    );
}

/// Two spellings can denote one keystroke. Left accepted, which of them took
/// effect would fall to the order the file's keys happen to sort in, which is a
/// precedence rule nobody declared — so a configuration naming a chord twice is
/// refused rather than silently resolved.
#[test]
fn two_spellings_of_one_chord_in_a_context_are_refused() {
    for (body, expected) in [
        // Modifier aliases.
        (
            "[bindings.compose-message]\n\"ctrl+j\" = \"send-message\"\n\"control+j\" = \"toggle-mode\"\n",
            "denote the same chord",
        ),
        // A control chord written in either case: both fold to the character a
        // terminal reports.
        (
            "[bindings.compose-message]\n\"ctrl+j\" = \"send-message\"\n\"Ctrl+J\" = \"toggle-mode\"\n",
            "denote the same chord",
        ),
        // The symbolic modifier landing on a literal chord the file also names.
        // Refused everywhere rather than only where it resolves onto it, so one
        // file is read the same way on every machine.
        (
            "[bindings.picker-sessions]\n\"ctrl+enter\" = \"commit-picker-session\"\n\"primary+enter\" = \"toggle-picker-focus\"\n",
            "denote the same chord",
        ),
    ] {
        let error = load_bindings(body)
            .expect_err(&format!("expected refusal for {body:?}"));
        let rendered = error.to_string();
        assert!(
            rendered.contains(expected),
            "{body:?} reported {rendered:?}, which does not mention {expected:?}"
        );
    }
}

/// The same chord in two different contexts is not a duplicate: contexts are
/// separate surfaces and a chord means what each of them says it means.
#[test]
fn one_chord_may_appear_in_two_contexts() {
    let loaded = load_bindings(
        "[bindings.compose-message]\n\"ctrl+w\" = \"send-message\"\n\n[bindings.compose-to]\n\"ctrl+w\" = \"clear-to-field\"\n",
    )
    .expect("distinct contexts are not a collision")
    .expect("existing config");
    assert_eq!(loaded.bindings.expect("a binding group").rows.len(), 2);
}

/// A configuration is applied whole or not at all. The rows before a mistake
/// must not survive it, or which bindings took effect would depend on where in
/// the file the operator's error happened to sit.
#[test]
fn an_invalid_group_applies_no_binding_from_that_configuration() {
    let body = r#"
[bindings.compose-message]
"ctrl+w" = "send-message"
"ctrl+g" = "fly-to-the-moon"
"#;
    let error = load_bindings(body).expect_err("the group is refused");
    assert!(error.to_string().contains("unknown action"));

    // The valid row above the mistake is not reachable through any successful
    // load, because there is no successful load.
    assert!(
        load_bindings(body).is_err(),
        "a configuration carrying a fault must not resolve"
    );
}

/// The compiled table declares a behavior only where it has an effect, so a
/// configuration may not bind one anywhere else: `clear-to-field` acts on the
/// `To` field and does nothing in the message field.
#[test]
fn a_behavior_the_context_does_not_declare_is_refused() {
    let error = load_bindings(
        "[bindings.compose-message]\n\"ctrl+g\" = \"clear-to-field\"\n",
    )
    .expect_err("an inert binding is refused");
    let rendered = error.to_string();
    assert!(
        rendered.contains("clear-to-field") && rendered.contains("no effect"),
        "{rendered:?}"
    );
}

/// Quit is the one behavior whose loss is refused rather than reported, since
/// an operator whose configuration cannot quit cannot fix it from inside the
/// TUI. Taking its chord is now enough to trip that, in every way the grammar
/// can take it.
///
/// The refusal was previously untrippable: the compiled quit row matched every
/// modifier set containing `Ctrl`, two of the six flags have no spelling, so
/// `Ctrl+Hyper+C` still quit however much an operator claimed. `Ctrl+C` denotes
/// one keystroke now, so the guard does what it was written to do.
#[test]
fn taking_the_quit_chord_is_refused_however_it_is_taken() {
    for value in [
        "\"none\"",
        "\"toggle-help-overlay\"",
        "{ standard = \"none\" }",
    ] {
        let error = load_bindings(&format!(
            "[bindings.global]\n\"ctrl+c\" = {value}\n"
        ))
        .expect_err("taking the only chord that quits must be refused");
        assert!(
            error.to_string().contains("no chord quits the TUI"),
            "{value} was refused for the wrong reason: {error}"
        );
    }
}

/// A further modifier is a different keystroke, so claiming `Ctrl+Shift+C`
/// leaves quit alone and the group loads.
///
/// The other side of the refusal above, and the one that would catch a guard
/// written too broadly — refusing anything that mentions the quit character
/// would refuse this too.
#[test]
fn claiming_a_modified_variant_of_the_quit_chord_still_loads() {
    let loaded = load_bindings(
        "[bindings.global]\n\"ctrl+shift+c\" = \"toggle-help-overlay\"\n",
    )
    .expect("Ctrl+Shift+C is not the quit chord and taking it loses nothing")
    .expect("existing config");
    assert_eq!(loaded.bindings.expect("a binding group").rows.len(), 1);
}

/// The control for the refusals above: a configuration that touches the global
/// context without taking every spelling still loads. Without it, a refusal
/// that fired on any global row at all would satisfy them.
#[test]
fn a_global_row_that_leaves_quit_alone_still_loads() {
    let loaded = load_bindings("[bindings.global]\n\"ctrl+q\" = \"quit\"\n")
        .expect("a global row that leaves quit reachable loads")
        .expect("existing config");
    assert_eq!(loaded.bindings.expect("a binding group").rows.len(), 1);
}

/// Losing any other behavior is a finding rather than a refusal, so the
/// configuration still loads. What pre-flight then says about it is asserted
/// against the reachability report itself.
#[test]
fn displacing_a_behavior_other_than_quit_still_loads() {
    let loaded = load_bindings(
        "[bindings.compose-message]\n\"enter\" = \"insert-message-newline\"\n\
         \"shift+enter\" = \"insert-message-newline\"\n\
         \"ctrl+enter\" = \"insert-message-newline\"\n",
    )
    .expect("displacing a behavior is reported, not refused")
    .expect("existing config");
    assert_eq!(loaded.bindings.expect("a binding group").rows.len(), 3);
}

/// The other half of that rule: a behavior the context does declare may be
/// given a chord it did not have, which is the main thing a configuration is
/// for.
#[test]
fn a_new_chord_for_a_declared_behavior_is_accepted() {
    let loaded = load_bindings(
        "[bindings.compose-to]\n\"ctrl+g\" = \"clear-to-field\"\n",
    )
    .expect("the binding is accepted")
    .expect("existing config");
    let rows = loaded.bindings.expect("a binding group").rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].context, BindingContext::ComposeTo);
    assert_eq!(
        rows[0].enhanced,
        Some(ConfiguredAction::Invoke(Action::ClearToField))
    );
}

/// Every binding set built into this binary is read by the parser that reads an
/// operator's file, so a set that does not parse fails here rather than
/// reaching a release.
#[test]
fn every_shipped_binding_set_parses() {
    let shipped = shipped_binding_presets();
    assert!(!shipped.is_empty(), "this build ships no binding set");
    for preset in shipped {
        let rows = embedded_binding_preset(preset.name, preset.text)
            .unwrap_or_else(|error| {
                panic!("{} does not parse: {error}", preset.name)
            });
        assert!(
            !rows.is_empty(),
            "{} parses but declares no binding",
            preset.name
        );
    }
}

/// A malformed binding set is a defect in our own artifact, and must not be
/// reported as a fault in a file the operator wrote.
///
/// The text is fixed at compile time and the parser is the one the check above
/// exercises, so nothing they can edit changes the outcome; naming their file
/// would send them auditing one that is fine. Exercised by handing the reader
/// text that does not parse rather than by shipping a set that does not, which
/// is the thing the check above exists to prevent.
#[test]
fn a_malformed_binding_set_is_not_reported_against_the_operators_file() {
    for (description, text) in [
        ("not TOML at all", "this is not ["),
        (
            "an unknown action",
            "[bindings.compose-message]\n\"ctrl+w\" = \"fly-to-the-moon\"\n",
        ),
        (
            "an unparseable chord",
            "[bindings.compose-message]\n\"Hyper+w\" = \"send-message\"\n",
        ),
        ("no binding group at all", "default-bundle = \"agentmux\"\n"),
        (
            "a set naming a set",
            "[bindings]\npresets = [\"enter-newline-shift-enter-sends\"]\n",
        ),
    ] {
        let error = embedded_binding_preset("binding set under test", text)
            .expect_err(&format!("{description} should be refused"));
        assert!(
            matches!(
                error,
                ConfigurationError::MalformedEmbeddedArtifact { .. }
            ),
            "{description}: classified as {error:?} rather than as a defect in our artifact"
        );
        let rendered = error.to_string();
        assert!(
            rendered.contains("built into this binary"),
            "{description}: the message does not say where the fault is: {rendered}"
        );
        assert!(
            !rendered.contains("ui.toml"),
            "{description}: the message points at the operator's file: {rendered}"
        );
    }
}

/// A bare character and its shifted form denote overlapping keystrokes, so
/// naming both in one context is refused rather than accepted and disagreed
/// about downstream.
///
/// A bare character denotes the character bare and carrying `Shift`; `shift+c`
/// denotes the second of those. Accepting both would give dispatch two rows
/// answering for the same keystroke while presentation dropped one of them, so
/// the overlay would stop advertising a chord dispatch still answers.
#[test]
fn a_character_and_its_shifted_form_collide_in_one_context() {
    let error = load_bindings(
        "[bindings.interaction-choice]\n\"c\" = \"move-next-choice-option\"\n\
         \"shift+c\" = \"move-previous-choice-option\"\n",
    )
    .expect_err("a character and its shifted form denote the same keystroke");
    assert!(
        error.to_string().contains("denote the same chord"),
        "refused for the wrong reason: {error}"
    );
}

/// The control for that refusal: `c` and `C` are distinct characters, so a
/// context may bind both. Without this, a collision check keyed on the letter
/// rather than on the keystrokes would satisfy the case above.
#[test]
fn the_two_cases_of_a_character_are_distinct_bindings() {
    let loaded = load_bindings(
        "[bindings.interaction-choice]\n\"c\" = \"move-next-choice-option\"\n\
         \"C\" = \"move-previous-choice-option\"\n",
    )
    .expect("c and C are different characters and may both be bound")
    .expect("existing config");
    assert_eq!(loaded.bindings.expect("a binding group").rows.len(), 2);
}

/// The embedded base table parses through the full loader path and agrees
/// with the defaults dispatch answers, row for row.
///
/// The TUI parses the same text with its own reader — the module dependency
/// stays one way that way — so this holds the two readers to the same row
/// set: every row the full parser yields must resolve identically through
/// `default_binding`, over every keystroke its chord denotes plus the typing
/// and unbound space around it, and the behavior sets per context must match
/// exactly. A grammar change the base cannot express fails here rather than
/// reaching a release.
#[test]
fn embedded_base_table_matches_shipped_defaults() {
    let base = embedded_base_preset();
    let rows = embedded_binding_preset(base.name, base.text)
        .unwrap_or_else(|error| panic!("base table does not parse: {error}"));
    assert!(!rows.is_empty(), "base table declares no binding");

    // The base speaks for both terminal classes with bare action names: a
    // class-qualified table or a symbolic modifier has nothing to say here,
    // and the TUI's reader refuses both.
    for row in &rows {
        assert!(
            !row.chord.uses_primary_modifier(),
            "base table uses the symbolic modifier"
        );
        assert_eq!(
            row.enhanced, row.standard,
            "base table qualifies a terminal class"
        );
        assert!(
            row.enhanced.is_some(),
            "base table names no action for either class"
        );
    }

    // No two rows in one context share a keystroke, so resolution order is
    // immaterial and the expectation below is unambiguous.
    for context in BindingContext::ALL {
        let mut claimed = Vec::new();
        for row in rows.iter().filter(|row| row.context == context) {
            for keystroke in denoted(row) {
                assert!(
                    !claimed.contains(&keystroke),
                    "base table binds one keystroke twice in {}",
                    context.configuration_name()
                );
                claimed.push(keystroke);
            }
        }
    }

    // Every denoted keystroke, every printable character bare and shifted,
    // and every named key under the common modifiers: what the full parser
    // yields must be what dispatch answers, with typing falling through to
    // the context's sink exactly where the table takes no text.
    let mut keystrokes = Vec::new();
    for row in &rows {
        keystrokes.extend(denoted(row));
    }
    for scalar in 0x20u8..=0x7Eu8 {
        let code = KeyCode::Char(scalar as char);
        keystrokes.push((code, KeyModifiers::NONE));
        keystrokes.push((code, KeyModifiers::SHIFT));
    }
    for character in 'a'..='z' {
        keystrokes.push((KeyCode::Char(character), KeyModifiers::CONTROL));
    }
    keystrokes.push((KeyCode::Char('C'), KeyModifiers::CONTROL));
    keystrokes.push((KeyCode::Char(' '), KeyModifiers::CONTROL));
    for code in [
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
    ]
    .into_iter()
    .chain((1..=12).map(KeyCode::F))
    {
        for modifiers in [
            KeyModifiers::NONE,
            KeyModifiers::SHIFT,
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ] {
            keystrokes.push((code, modifiers));
        }
    }

    let mut reached: Vec<(BindingContext, Action)> = Vec::new();
    for context in BindingContext::ALL {
        for (code, modifiers) in &keystrokes {
            let expected = rows
                .iter()
                .filter(|row| row.context == context)
                .find_map(|row| {
                    denoted(row).contains(&(*code, *modifiers)).then_some({
                        match row.enhanced {
                            Some(ConfiguredAction::Invoke(action)) => {
                                Some(action)
                            },
                            _ => None,
                        }
                    })
                })
                .unwrap_or_else(|| sink_action(context, code, *modifiers));
            let answered = default_binding(context, *code, *modifiers);
            assert_eq!(
                answered,
                expected,
                "base table and dispatch disagree in {} on {code:?} with {modifiers:?}",
                context.configuration_name()
            );
            if let Some(action) = answered
                && !is_typing_action(action)
                && !reached.contains(&(context, action))
            {
                reached.push((context, action));
            }
        }
    }

    // The base declares each behavior exactly where it has an effect. This
    // pins the behavior sets the old static table carried, so a row naming a
    // behavior its context cannot perform fails here rather than advertising
    // a binding that does nothing.
    for (context_name, actions) in EXPECTED_BASE_BEHAVIORS {
        let context = BindingContext::from_configuration_name(context_name)
            .unwrap_or_else(|| panic!("unknown context: {context_name}"));
        let mut expected: Vec<Action> = actions
            .iter()
            .map(|name| {
                Action::from_configuration_name(name)
                    .unwrap_or_else(|| panic!("unknown action: {name}"))
            })
            .collect();
        expected.sort_by_key(|action| {
            action.configuration_name().expect("behavior has no name")
        });
        let mut found: Vec<Action> = reached
            .iter()
            .filter(|(reached_context, _)| *reached_context == context)
            .map(|(_, action)| *action)
            .collect();
        found.sort_by_key(|action| {
            action.configuration_name().expect("behavior has no name")
        });
        assert_eq!(
            found, expected,
            "base table behavior set changed in {context_name}"
        );
    }
}

/// Every keystroke one parsed row denotes, mirroring the resolver: a bare
/// character denotes itself bare and carrying `Shift`, anything else denotes
/// the one keystroke it resolves to. The base never uses the symbolic
/// modifier, so resolving with `Control` is immaterial.
fn denoted(
    row: &agentmux::tui::ConfiguredBinding,
) -> Vec<(KeyCode, KeyModifiers)> {
    let (code, modifiers) = row.chord.resolve(KeyModifiers::CONTROL);
    match code {
        KeyCode::Char(_) if modifiers.is_empty() => {
            vec![(code, KeyModifiers::NONE), (code, KeyModifiers::SHIFT)]
        },
        _ => vec![(code, modifiers)],
    }
}

/// What typing a character reaches in a context: the sink's action where the
/// context takes text, nothing where it does not. This duplicates the
/// context's sink mapping on purpose — it pins which contexts take text, so
/// gaining or losing one is an explicit test edit rather than drift.
fn sink_action(
    context: BindingContext,
    code: &KeyCode,
    modifiers: KeyModifiers,
) -> Option<Action> {
    let KeyCode::Char(character) = code else {
        return None;
    };
    if modifiers != KeyModifiers::NONE && modifiers != KeyModifiers::SHIFT {
        return None;
    }
    match context {
        BindingContext::ComposeTo | BindingContext::ComposeMessage => {
            Some(Action::InsertComposeCharacter(*character))
        },
        BindingContext::InteractionWrite
        | BindingContext::InteractionChoice => {
            Some(Action::InsertRawwCharacter(*character))
        },
        BindingContext::PickerBundles | BindingContext::PickerSessions => {
            Some(Action::AppendPickerFilterCharacter(*character))
        },
        BindingContext::Global
        | BindingContext::EventsOverlay
        | BindingContext::HelpOverlay => None,
    }
}

/// Whether an action carries the operator's own keystroke rather than naming a
/// behavior in advance. Such actions are reachable but not declared, so the
/// behavior-set pin above counts them out.
fn is_typing_action(action: Action) -> bool {
    matches!(
        action,
        Action::InsertComposeCharacter(_)
            | Action::InsertRawwCharacter(_)
            | Action::AppendPickerFilterCharacter(_)
    )
}

/// Every behavior the base table declares per context, by configuration name.
///
/// Generated from the table and pinned here so the set is reviewed as data: a
/// behavior added or removed in any context updates this list in the same
/// commit, and a row naming a behavior its context cannot perform fails the
/// resolution check above rather than advertising a binding that does
/// nothing.
const EXPECTED_BASE_BEHAVIORS: &[(&str, &[&str])] = &[
    ("global", &["quit", "toggle-help-overlay"]),
    (
        "compose-to",
        &[
            "open-picker",
            "toggle-events-overlay",
            "toggle-mode",
            "open-bundle-picker",
            "refresh-recipients",
            "move-to-field-cursor-home",
            "move-to-field-cursor-end",
            "clear-to-field",
            "autocomplete-recipient",
            "cycle-next-focus",
            "cycle-previous-focus",
            "accept-to-completion",
            "move-previous-to-completion",
            "move-next-to-completion",
            "move-to-field-cursor-left",
            "move-to-field-cursor-right",
            "delete-compose-character",
            "scroll-chat-history-page-up",
            "scroll-chat-history-page-down",
        ],
    ),
    (
        "compose-message",
        &[
            "open-picker",
            "toggle-events-overlay",
            "toggle-mode",
            "open-bundle-picker",
            "refresh-recipients",
            "move-message-cursor-home",
            "move-message-cursor-end",
            "insert-message-newline",
            "cycle-next-focus",
            "cycle-previous-focus",
            "send-message",
            "snap-chat-history-to-latest",
            "move-message-cursor-up",
            "move-message-cursor-down",
            "move-message-cursor-left",
            "move-message-cursor-right",
            "delete-compose-character",
            "scroll-chat-history-page-up",
            "scroll-chat-history-page-down",
        ],
    ),
    (
        "interaction-write",
        &[
            "open-picker",
            "toggle-events-overlay",
            "toggle-mode",
            "open-bundle-picker",
            "refresh-recipients",
            "insert-raww-newline",
            "dispatch-raww",
            "move-raww-cursor-left",
            "move-raww-cursor-right",
            "navigate-interaction-up",
            "navigate-interaction-down",
            "move-raww-cursor-home",
            "move-raww-cursor-end",
            "delete-raww-character",
            "scroll-interaction-snapshot-page-up",
            "scroll-interaction-snapshot-page-down",
        ],
    ),
    (
        "interaction-choice",
        &[
            "open-picker",
            "toggle-events-overlay",
            "toggle-mode",
            "open-bundle-picker",
            "refresh-recipients",
            "insert-raww-newline",
            "resolve-choice-selected",
            "move-previous-choice-request",
            "move-next-choice-request",
            "move-previous-choice-option",
            "move-next-choice-option",
            "resolve-choice-cancelled",
            "delete-raww-character",
            "scroll-interaction-snapshot-page-up",
            "scroll-interaction-snapshot-page-down",
        ],
    ),
    (
        "picker-bundles",
        &[
            "close-picker",
            "toggle-events-overlay",
            "toggle-mode",
            "commit-picker-bundle",
            "toggle-picker-focus",
            "move-next-picker-selection",
            "move-previous-picker-selection",
            "delete-picker-filter-character",
        ],
    ),
    (
        "picker-sessions",
        &[
            "close-picker",
            "toggle-events-overlay",
            "toggle-mode",
            "commit-picker-session",
            "toggle-picker-focus",
            "move-next-picker-selection",
            "move-previous-picker-selection",
            "delete-picker-filter-character",
        ],
    ),
    (
        "events-overlay",
        &[
            "toggle-events-overlay",
            "open-picker",
            "toggle-mode",
            "open-bundle-picker",
        ],
    ),
    (
        "help-overlay",
        &[
            "toggle-help-overlay",
            "open-picker",
            "toggle-events-overlay",
            "toggle-mode",
            "open-bundle-picker",
            "scroll-help-up",
            "scroll-help-down",
            "scroll-help-page-up",
            "scroll-help-page-down",
            "scroll-help-to-start",
            "scroll-help-to-end",
        ],
    ),
];
