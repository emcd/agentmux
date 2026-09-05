//! The table an operator's configuration produces over the compiled defaults.
//!
//! These build the table directly rather than through a running workbench, so
//! each capability class and each platform is exercisable without a terminal.

use crossterm::event::{KeyCode, KeyModifiers};

use agentmux::tui::{
    Action, BindingConfiguration, BindingContext, CapabilityClass, ConfiguredAction,
    ConfiguredBinding, EffectiveBindings, PrimaryModifier, default_binding, parse_chord,
};

/// A configured row binding one chord to one action on both capability classes.
fn row(context: BindingContext, chord: &str, action: Action) -> ConfiguredBinding {
    let invoke = Some(ConfiguredAction::Invoke(action));
    ConfiguredBinding {
        context,
        chord: parse_chord(chord).expect("chord parses"),
        enhanced: invoke,
        standard: invoke,
    }
}

/// A configured row speaking for one class only.
fn row_for(
    context: BindingContext,
    chord: &str,
    class: CapabilityClass,
    action: ConfiguredAction,
) -> ConfiguredBinding {
    let mut binding = ConfiguredBinding {
        context,
        chord: parse_chord(chord).expect("chord parses"),
        enhanced: None,
        standard: None,
    };
    match class {
        CapabilityClass::Enhanced => binding.enhanced = Some(action),
        CapabilityClass::Standard => binding.standard = Some(action),
    }
    binding
}

fn configuration(rows: Vec<ConfiguredBinding>) -> BindingConfiguration {
    BindingConfiguration {
        presets: Vec::new(),
        primary_modifier_on_macos: None,
        rows,
    }
}

fn built(configuration: &BindingConfiguration) -> EffectiveBindings {
    EffectiveBindings::build(Some(configuration), &[], CapabilityClass::Standard, false)
}

/// Absent configuration must leave every compiled default answering exactly as
/// it does without this machinery, on both capability classes. This is the
/// claim that the change alters nothing out of the box.
#[test]
fn without_a_configuration_every_compiled_default_still_answers() {
    for class in [CapabilityClass::Enhanced, CapabilityClass::Standard] {
        let table = EffectiveBindings::build(None, &[], class, false);
        for context in BindingContext::ALL {
            for (code, modifiers) in probe_chords() {
                assert_eq!(
                    table.action_for(context, code, modifiers),
                    default_binding(context, code, modifiers),
                    "{context:?} {code:?} {modifiers:?} diverged from the compiled default"
                );
            }
        }
    }
}

/// A spread of chords the compiled table binds somewhere, plus some it binds
/// nowhere, so the comparison above covers both answers.
fn probe_chords() -> Vec<(KeyCode, KeyModifiers)> {
    vec![
        (KeyCode::Enter, KeyModifiers::NONE),
        (KeyCode::Enter, KeyModifiers::SHIFT),
        (KeyCode::Enter, KeyModifiers::CONTROL),
        (KeyCode::Esc, KeyModifiers::NONE),
        (KeyCode::Tab, KeyModifiers::NONE),
        (KeyCode::BackTab, KeyModifiers::NONE),
        (KeyCode::F(1), KeyModifiers::NONE),
        (KeyCode::F(5), KeyModifiers::NONE),
        (KeyCode::Char('c'), KeyModifiers::CONTROL),
        (KeyCode::Char('j'), KeyModifiers::CONTROL),
        (KeyCode::Char('q'), KeyModifiers::CONTROL),
        (KeyCode::PageUp, KeyModifiers::NONE),
    ]
}

#[test]
fn a_default_the_configuration_does_not_name_survives() {
    let table = built(&configuration(vec![row(
        BindingContext::ComposeMessage,
        "ctrl+w",
        Action::SendMessage,
    )]));
    // Ctrl+J is a compiled row in that same context, and was not named.
    assert_eq!(
        table.action_for(
            BindingContext::ComposeMessage,
            KeyCode::Char('j'),
            KeyModifiers::CONTROL
        ),
        Some(Action::InsertMessageNewline)
    );
}

#[test]
fn a_configured_row_wins_over_the_compiled_row_it_shadows() {
    let context = BindingContext::ComposeMessage;
    assert_eq!(
        default_binding(context, KeyCode::Char('j'), KeyModifiers::CONTROL),
        Some(Action::InsertMessageNewline),
        "the compiled row this test shadows is not the one it assumes"
    );

    let table = built(&configuration(vec![row(
        context,
        "ctrl+j",
        Action::SendMessage,
    )]));
    assert_eq!(
        table.action_for(context, KeyCode::Char('j'), KeyModifiers::CONTROL),
        Some(Action::SendMessage)
    );
}

/// The tier the named binding sets fill sits between the operator's own rows
/// and the compiled defaults: it overrides what ships, and yields to what the
/// operator wrote.
#[test]
fn a_preset_row_outranks_the_compiled_default_and_yields_to_a_configured_row() {
    let context = BindingContext::ComposeMessage;
    let preset = vec![row(context, "ctrl+j", Action::SendMessage)];

    let without_configuration =
        EffectiveBindings::build(None, &preset, CapabilityClass::Standard, false);
    assert_eq!(
        without_configuration.action_for(context, KeyCode::Char('j'), KeyModifiers::CONTROL),
        Some(Action::SendMessage),
        "a preset row did not outrank the compiled default"
    );

    let configured = configuration(vec![row(context, "ctrl+j", Action::InsertMessageNewline)]);
    let with_configuration =
        EffectiveBindings::build(Some(&configured), &preset, CapabilityClass::Standard, false);
    assert_eq!(
        with_configuration.action_for(context, KeyCode::Char('j'), KeyModifiers::CONTROL),
        Some(Action::InsertMessageNewline),
        "a preset row overrode the operator's own row"
    );
}

/// Binding sets are applied in the order they are named, so a later one
/// supersedes an earlier one binding the same chord. The rule lives in the
/// table: whoever assembles the sets hands them over in the order named, and
/// does not have to reverse them to get the declared precedence.
#[test]
fn a_later_preset_row_supersedes_an_earlier_one_on_the_same_chord() {
    let context = BindingContext::ComposeMessage;
    let preset = vec![
        row(context, "ctrl+j", Action::SendMessage),
        row(context, "ctrl+j", Action::ToggleMode),
    ];

    let table = EffectiveBindings::build(None, &preset, CapabilityClass::Standard, false);
    assert_eq!(
        table.action_for(context, KeyCode::Char('j'), KeyModifiers::CONTROL),
        Some(Action::ToggleMode),
        "the earlier preset row answered, so later-wins is not in force"
    );

    // And the operator's own row still outranks whichever preset row won.
    let configured = configuration(vec![row(context, "ctrl+j", Action::InsertMessageNewline)]);
    let with_configuration =
        EffectiveBindings::build(Some(&configured), &preset, CapabilityClass::Standard, false);
    assert_eq!(
        with_configuration.action_for(context, KeyCode::Char('j'), KeyModifiers::CONTROL),
        Some(Action::InsertMessageNewline)
    );
}

/// A later set emptying a chord an earlier one bound leaves it empty, since
/// superseding is about which row answers rather than about which binds.
#[test]
fn a_later_preset_row_can_supersede_an_earlier_one_with_an_unbinding() {
    let context = BindingContext::ComposeMessage;
    let unbind = ConfiguredBinding {
        context,
        chord: parse_chord("ctrl+j").expect("chord parses"),
        enhanced: Some(ConfiguredAction::Unbound),
        standard: Some(ConfiguredAction::Unbound),
    };
    let preset = vec![row(context, "ctrl+j", Action::SendMessage), unbind];

    let table = EffectiveBindings::build(None, &preset, CapabilityClass::Standard, false);
    assert_eq!(
        table.action_for(context, KeyCode::Char('j'), KeyModifiers::CONTROL),
        None
    );
}

/// Contexts are consulted by the caller, so a configured contextual row cannot
/// reach a chord the global rows answer first.
#[test]
fn a_configured_contextual_row_does_not_shadow_a_compiled_global_row() {
    let table = built(&configuration(vec![row(
        BindingContext::ComposeMessage,
        "ctrl+c",
        Action::SendMessage,
    )]));
    // The global context still answers Ctrl+C with quit, and a caller walking
    // the lookup order asks it first.
    assert_eq!(
        table.action_for(
            BindingContext::Global,
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        ),
        Some(Action::Quit)
    );
}

/// Emptying a chord is a statement, not a silence: it must not fall through to
/// the compiled row beneath it.
#[test]
fn an_explicit_unbinding_leaves_the_chord_inert() {
    let context = BindingContext::ComposeMessage;
    let table = built(&configuration(vec![ConfiguredBinding {
        context,
        chord: parse_chord("ctrl+j").expect("chord parses"),
        enhanced: Some(ConfiguredAction::Unbound),
        standard: Some(ConfiguredAction::Unbound),
    }]));

    assert_eq!(
        table.action_for(context, KeyCode::Char('j'), KeyModifiers::CONTROL),
        None,
        "an unbound chord fell through to its compiled default"
    );
    assert!(
        table.is_configured(context, KeyCode::Char('j'), KeyModifiers::CONTROL),
        "an unbound chord should read as spoken for, not as absent"
    );
}

#[test]
fn a_single_value_row_applies_on_both_capability_classes() {
    let context = BindingContext::ComposeMessage;
    let configured = configuration(vec![row(context, "ctrl+w", Action::SendMessage)]);
    for class in [CapabilityClass::Enhanced, CapabilityClass::Standard] {
        let table = EffectiveBindings::build(Some(&configured), &[], class, false);
        assert_eq!(
            table.action_for(context, KeyCode::Char('w'), KeyModifiers::CONTROL),
            Some(Action::SendMessage),
            "{class:?}"
        );
    }
}

#[test]
fn a_class_qualified_row_applies_only_to_the_class_it_names() {
    let context = BindingContext::ComposeMessage;
    let configured = configuration(vec![row_for(
        context,
        "ctrl+j",
        CapabilityClass::Enhanced,
        ConfiguredAction::Invoke(Action::SendMessage),
    )]);

    let enhanced =
        EffectiveBindings::build(Some(&configured), &[], CapabilityClass::Enhanced, false);
    assert_eq!(
        enhanced.action_for(context, KeyCode::Char('j'), KeyModifiers::CONTROL),
        Some(Action::SendMessage)
    );

    // The class the row did not speak for keeps its compiled default.
    let standard =
        EffectiveBindings::build(Some(&configured), &[], CapabilityClass::Standard, false);
    assert_eq!(
        standard.action_for(context, KeyCode::Char('j'), KeyModifiers::CONTROL),
        Some(Action::InsertMessageNewline),
        "an omitted class did not keep its compiled default"
    );
    assert!(
        !standard.is_configured(context, KeyCode::Char('j'), KeyModifiers::CONTROL),
        "a class the row left alone should not read as spoken for"
    );
}

/// The symbolic modifier is resolved as the table is built, so the same
/// configuration produces different chords on different platforms.
#[test]
fn the_symbolic_modifier_resolves_as_the_table_is_built() {
    let context = BindingContext::PickerSessions;
    let mut configured = configuration(vec![row(
        context,
        "primary+enter",
        Action::CommitPickerSession,
    )]);

    let elsewhere =
        EffectiveBindings::build(Some(&configured), &[], CapabilityClass::Standard, false);
    assert_eq!(
        elsewhere.action_for(context, KeyCode::Enter, KeyModifiers::CONTROL),
        Some(Action::CommitPickerSession),
        "off macOS the symbolic modifier should resolve to Ctrl"
    );

    configured.primary_modifier_on_macos = Some(PrimaryModifier::Command);
    let on_macos =
        EffectiveBindings::build(Some(&configured), &[], CapabilityClass::Standard, true);
    assert_eq!(
        on_macos.action_for(context, KeyCode::Enter, KeyModifiers::SUPER),
        Some(Action::CommitPickerSession),
        "the macOS selection did not reach the built table"
    );
    assert!(
        !on_macos.is_configured(context, KeyCode::Enter, KeyModifiers::CONTROL),
        "the chord should no longer sit on Ctrl once it resolved to Cmd"
    );
}

#[test]
fn the_capability_class_follows_the_probe_outcome() {
    assert_eq!(CapabilityClass::of(true), CapabilityClass::Enhanced);
    assert_eq!(CapabilityClass::of(false), CapabilityClass::Standard);
}
