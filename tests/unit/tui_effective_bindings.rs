//! The table an operator's configuration produces over the compiled defaults.
//!
//! These build the table directly rather than through a running workbench, so
//! each capability class and each platform is exercisable without a terminal.

use crossterm::event::{KeyCode, KeyModifiers};

use agentmux::configuration::{embedded_binding_preset, shipped_binding_presets};
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
        preset_rows: Vec::new(),
        primary_modifier_on_macos: None,
        rows,
    }
}

/// A configuration whose named sets contributed `preset_rows`, and which
/// declares `rows` of its own.
///
/// The names those rows came from are left empty: what the tier does is decided
/// by the rows, and a test that fixed a name here would be asserting against
/// whichever sets happen to ship rather than against the tier.
fn with_presets(
    preset_rows: Vec<ConfiguredBinding>,
    rows: Vec<ConfiguredBinding>,
) -> BindingConfiguration {
    BindingConfiguration {
        preset_rows,
        ..configuration(rows)
    }
}

fn built(configuration: &BindingConfiguration) -> EffectiveBindings {
    EffectiveBindings::build(Some(configuration), CapabilityClass::Standard, false)
}

/// Absent configuration must leave every compiled default answering exactly as
/// it does without this machinery, on both capability classes. This is the
/// claim that the change alters nothing out of the box.
#[test]
fn without_a_configuration_every_compiled_default_still_answers() {
    for class in [CapabilityClass::Enhanced, CapabilityClass::Standard] {
        let table = EffectiveBindings::build(None, class, false);
        for context in BindingContext::ALL {
            for (code, modifiers) in chord_space() {
                assert_eq!(
                    table.action_for(context, code, modifiers),
                    default_binding(context, code, modifiers),
                    "{context:?} {code:?} {modifiers:?} diverged from the compiled default"
                );
            }
        }
    }
}

/// Every keystroke worth resolving against the table: each key code the
/// compiled rows use, under each modifier set they distinguish.
///
/// Swept rather than sampled, so a claim about what a table answers covers the
/// chords a set moved as well as the ones a reader thought to list. It spans
/// chords the compiled table binds somewhere and chords it binds nowhere, so a
/// comparison over it covers both answers.
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
            KeyModifiers::SUPER,
        ] {
            chords.push((code, modifiers));
        }
    }
    chords
}

/// The keystrokes from that space a terminal in one class can actually deliver.
///
/// A terminal that does not report modified keys distinctly delivers a modified
/// `Enter` as a bare `Enter`, so a row written as `Shift+Enter` is one no
/// keystroke there satisfies. A reachability question asked over the whole
/// space would accept such a row as an answer and so would miss the state a
/// set's class declaration exists to prevent: sending displaced onto a chord
/// that cannot arrive, with nothing left that sends.
///
/// Narrowed to `Enter` because that is the divergence the capability contract
/// is written about; a class that came to differ in some other key would need
/// this to follow.
fn deliverable_chords(class: CapabilityClass) -> Vec<(KeyCode, KeyModifiers)> {
    chord_space()
        .into_iter()
        .filter(|(code, modifiers)| {
            class == CapabilityClass::Enhanced || *code != KeyCode::Enter || modifiers.is_empty()
        })
        .collect()
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

    let without_configuration = built(&with_presets(preset.clone(), Vec::new()));
    assert_eq!(
        without_configuration.action_for(context, KeyCode::Char('j'), KeyModifiers::CONTROL),
        Some(Action::SendMessage),
        "a preset row did not outrank the compiled default"
    );

    let with_configuration = built(&with_presets(
        preset,
        vec![row(context, "ctrl+j", Action::InsertMessageNewline)],
    ));
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

    let table = built(&with_presets(preset.clone(), Vec::new()));
    assert_eq!(
        table.action_for(context, KeyCode::Char('j'), KeyModifiers::CONTROL),
        Some(Action::ToggleMode),
        "the earlier preset row answered, so later-wins is not in force"
    );

    // And the operator's own row still outranks whichever preset row won.
    let with_configuration = built(&with_presets(
        preset,
        vec![row(context, "ctrl+j", Action::InsertMessageNewline)],
    ));
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

    let table = built(&with_presets(preset, Vec::new()));
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
        let table = EffectiveBindings::build(Some(&configured), class, false);
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

    let enhanced = EffectiveBindings::build(Some(&configured), CapabilityClass::Enhanced, false);
    assert_eq!(
        enhanced.action_for(context, KeyCode::Char('j'), KeyModifiers::CONTROL),
        Some(Action::SendMessage)
    );

    // The class the row did not speak for keeps its compiled default.
    let standard = EffectiveBindings::build(Some(&configured), CapabilityClass::Standard, false);
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

    let elsewhere = EffectiveBindings::build(Some(&configured), CapabilityClass::Standard, false);
    assert_eq!(
        elsewhere.action_for(context, KeyCode::Enter, KeyModifiers::CONTROL),
        Some(Action::CommitPickerSession),
        "off macOS the symbolic modifier should resolve to Ctrl"
    );

    configured.primary_modifier_on_macos = Some(PrimaryModifier::Command);
    let on_macos = EffectiveBindings::build(Some(&configured), CapabilityClass::Standard, true);
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

/// The claim that this change alters nothing out of the box, asserted between
/// the two classes rather than against a table each was separately compared to.
///
/// The compiled rows carry no capability field, and this is what holds them to
/// it: a row that started varying by class would answer here before it reached
/// an operator who never asked for the variance and cannot see the
/// discriminator.
#[test]
fn the_compiled_defaults_are_identical_across_both_capability_classes() {
    let enhanced = EffectiveBindings::build(None, CapabilityClass::Enhanced, false);
    let standard = EffectiveBindings::build(None, CapabilityClass::Standard, false);
    for context in BindingContext::ALL {
        for (code, modifiers) in chord_space() {
            assert_eq!(
                enhanced.action_for(context, code, modifiers),
                standard.action_for(context, code, modifiers),
                "{context:?} {code:?} {modifiers:?} answers differently by capability class \
                 with nothing configured and no binding set applied"
            );
        }
    }
}

/// The rows one shipped binding set contributes, as a configuration naming it
/// would carry them.
fn shipped(name: &str) -> BindingConfiguration {
    let preset = shipped_binding_presets()
        .iter()
        .find(|preset| preset.name == name)
        .unwrap_or_else(|| panic!("no binding set named {name} ships"));
    BindingConfiguration {
        presets: vec![name.to_string()],
        preset_rows: embedded_binding_preset(preset.name, preset.text)
            .unwrap_or_else(|error| panic!("{name} does not parse: {error}")),
        ..BindingConfiguration::default()
    }
}

/// The two sets that ship, and the chord each one's name promises sending moves
/// to. Off macOS the symbolic modifier resolves to `Ctrl`.
const SHIPPED: [(&str, KeyModifiers); 2] = [
    ("enter-newline-shift-enter-sends", KeyModifiers::SHIFT),
    ("enter-newline-primary-enter-sends", KeyModifiers::CONTROL),
];

/// Exactly these two sets ship, so a third arriving without a case below is a
/// failure rather than a set nothing here asks anything of.
///
/// Compared as a set rather than as a sequence: the registry's order carries no
/// meaning — a configuration decides the order its own sets apply in by the
/// order it names them — so pinning it here would assert something the
/// implementation does not promise.
#[test]
fn the_shipped_binding_sets_are_the_two_this_change_promised() {
    let mut shipped: Vec<&str> = shipped_binding_presets()
        .iter()
        .map(|preset| preset.name)
        .collect();
    let mut named: Vec<&str> = SHIPPED.iter().map(|(name, _)| *name).collect();
    shipped.sort_unstable();
    named.sort_unstable();
    assert_eq!(shipped, named);
}

/// Each shipped set does what its name says: `Enter` inserts a newline, and
/// sending moves to the chord the name promises.
#[test]
fn each_shipped_binding_set_moves_sending_to_the_chord_its_name_promises() {
    let compose = BindingContext::ComposeMessage;
    for (name, sends_with) in SHIPPED {
        let table =
            EffectiveBindings::build(Some(&shipped(name)), CapabilityClass::Enhanced, false);
        assert_eq!(
            table.action_for(compose, KeyCode::Enter, KeyModifiers::NONE),
            Some(Action::InsertMessageNewline),
            "{name}: Enter does not insert a newline"
        );
        assert_eq!(
            table.action_for(compose, KeyCode::Enter, sends_with),
            Some(Action::SendMessage),
            "{name}: sending is not on the chord the set's name promises"
        );
    }
}

/// A set's chords are resolved like any other row, so the one written with the
/// symbolic modifier follows the operator's macOS selection.
#[test]
fn a_shipped_binding_set_honors_the_macos_primary_modifier_selection() {
    let compose = BindingContext::ComposeMessage;
    let configuration = BindingConfiguration {
        primary_modifier_on_macos: Some(PrimaryModifier::Command),
        ..shipped("enter-newline-primary-enter-sends")
    };
    let table = EffectiveBindings::build(Some(&configuration), CapabilityClass::Enhanced, true);
    assert_eq!(
        table.action_for(compose, KeyCode::Enter, KeyModifiers::SUPER),
        Some(Action::SendMessage),
        "the set's symbolic chord did not follow the macOS selection"
    );
}

/// Every shipped set declares the disambiguating class, so the probe reporting
/// the other one must leave the compiled defaults exactly as they were.
///
/// Swept over the whole keystroke space rather than over the chords the set
/// names, because a set leaking into the other class could displace a chord it
/// does not name — through the symbolic modifier, say.
#[test]
fn a_shipped_binding_set_contributes_nothing_under_the_other_capability_class() {
    for (name, _) in SHIPPED {
        let configuration = shipped(name);
        let standard =
            EffectiveBindings::build(Some(&configuration), CapabilityClass::Standard, false);
        for context in BindingContext::ALL {
            for (code, modifiers) in chord_space() {
                assert_eq!(
                    standard.action_for(context, code, modifiers),
                    default_binding(context, code, modifiers),
                    "{name}: {context:?} {code:?} {modifiers:?} moved off its compiled default \
                     under the class the set does not declare"
                );
                assert!(
                    !standard.is_configured(context, code, modifiers),
                    "{name}: {context:?} {code:?} {modifiers:?} reads as spoken for under the \
                     class the set does not declare"
                );
            }
        }

        // Without this, a set that parsed to no rows at all would satisfy the
        // sweep above by doing nothing anywhere.
        let enhanced =
            EffectiveBindings::build(Some(&configuration), CapabilityClass::Enhanced, false);
        assert!(
            BindingContext::ALL.iter().any(|context| {
                chord_space().iter().any(|(code, modifiers)| {
                    enhanced.action_for(*context, *code, *modifiers)
                        != default_binding(*context, *code, *modifiers)
                })
            }),
            "{name}: the set changes nothing under the class it does declare either"
        );
    }
}

/// A set that moved sending off the chord reaching it by default must leave
/// some chord that still reaches it. This is the state the class declaration
/// exists to prevent a set from shipping in.
///
/// What "sending" means is read from the context rather than named here: it is
/// whatever that context's compiled rows bind bare `Enter` to, which is the
/// commit behavior of every context that has one.
#[test]
fn every_shipped_binding_set_leaves_sending_reachable_in_every_context_it_touches() {
    for (name, _) in SHIPPED {
        let configuration = shipped(name);
        let mut touched: Vec<BindingContext> = Vec::new();
        for row in &configuration.preset_rows {
            if !touched.contains(&row.context) {
                touched.push(row.context);
            }
        }
        assert!(!touched.is_empty(), "{name}: the set touches no context");

        for context in touched {
            let sending = default_binding(context, KeyCode::Enter, KeyModifiers::NONE)
                .unwrap_or_else(|| panic!("{name}: {context:?} binds bare Enter to nothing"));
            for class in [CapabilityClass::Enhanced, CapabilityClass::Standard] {
                let table = EffectiveBindings::build(Some(&configuration), class, false);
                assert!(
                    deliverable_chords(class).iter().any(|(code, modifiers)| {
                        table.action_for(context, *code, *modifiers) == Some(sending)
                    }),
                    "{name}: nothing reaches {sending:?} in {context:?} under {class:?}"
                );
            }
        }
    }
}

/// The check above has teeth only where a set actually moved sending, and both
/// sets that ship do. Asserted rather than assumed, because a set that left
/// sending where it found it would satisfy that check without exercising it.
#[test]
fn every_shipped_binding_set_moves_sending_off_the_chord_that_reaches_it_by_default() {
    let compose = BindingContext::ComposeMessage;
    let sending = default_binding(compose, KeyCode::Enter, KeyModifiers::NONE);
    for (name, _) in SHIPPED {
        let table =
            EffectiveBindings::build(Some(&shipped(name)), CapabilityClass::Enhanced, false);
        assert_ne!(
            table.action_for(compose, KeyCode::Enter, KeyModifiers::NONE),
            sending,
            "{name}: Enter still reaches sending, so the reachability check cannot fail"
        );
    }
}

/// A compiled row naming a bare character accepts that character carrying
/// `Shift`, whether the row invokes a fixed action or carries the character
/// into one.
///
/// The one place exactness would break something a terminal actually does. A
/// terminal's report of a typed character is not a function of the key alone:
/// `Shift` and `Caps Lock` each alter both which character arrives and which
/// modifiers accompany it, so a form admitting only the bare arrival would
/// refuse a keystroke an operator produced by typing.
#[test]
fn a_shifted_character_reaches_the_row_that_names_it() {
    let choice = BindingContext::InteractionChoice;
    // A fixed-action character row.
    for character in ['c', 'C'] {
        for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
            assert_eq!(
                default_binding(choice, KeyCode::Char(character), modifiers),
                Some(Action::ResolveChoiceCancelled),
                "{character:?} under {modifiers:?} must resolve the choice as cancelled"
            );
        }
    }
    // A typing row, which carries the character rather than reaching a fixed
    // action.
    let compose = BindingContext::ComposeMessage;
    for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
        assert_eq!(
            default_binding(compose, KeyCode::Char('q'), modifiers),
            Some(Action::InsertComposeCharacter('q')),
            "typing must accept a character under {modifiers:?}"
        );
    }
}

/// An operator's bare single-character chord denotes the same two keystrokes
/// the compiled row does, so claiming the character claims all of what that row
/// answered.
///
/// Symmetry is the requirement, not the pair. Were a configured `c` to resolve
/// to the bare form alone, the configured row would claim one of the two while
/// the compiled row kept answering for the other — the exact condition exactness
/// exists to remove, reappearing in the one shape exempted from it.
#[test]
fn configuring_a_character_claims_both_of_its_forms() {
    let choice = BindingContext::InteractionChoice;
    assert_eq!(
        default_binding(choice, KeyCode::Char('c'), KeyModifiers::SHIFT),
        Some(Action::ResolveChoiceCancelled),
        "the premise fails -- the compiled row does not answer the shifted form"
    );

    let configuration = BindingConfiguration {
        rows: vec![row(choice, "c", Action::MoveNextChoiceOption)],
        ..BindingConfiguration::default()
    };
    let bindings = EffectiveBindings::build(Some(&configuration), CapabilityClass::Standard, false);

    for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
        assert_eq!(
            bindings.action_for(choice, KeyCode::Char('c'), modifiers),
            Some(Action::MoveNextChoiceOption),
            "the configured character must intercept its {modifiers:?} arrival"
        );
    }
    // The displaced compiled row reaches nothing through the character it named.
    // 'C' is a different row and is deliberately left alone.
    assert_eq!(
        bindings.action_for(choice, KeyCode::Char('C'), KeyModifiers::NONE),
        Some(Action::ResolveChoiceCancelled),
        "configuring 'c' must not disturb the row naming 'C'"
    );
}

/// A configured character written with a modifier is a chord rather than
/// typing, so it denotes that one keystroke and leaves the bare forms alone.
///
/// The boundary of the rule above: without this, resolving every configured
/// character to a two-keystroke shape would look identical in the test that
/// matters.
#[test]
fn a_configured_character_carrying_a_modifier_denotes_one_keystroke() {
    let choice = BindingContext::InteractionChoice;
    let configuration = BindingConfiguration {
        rows: vec![row(choice, "ctrl+c", Action::MoveNextChoiceOption)],
        ..BindingConfiguration::default()
    };
    let bindings = EffectiveBindings::build(Some(&configuration), CapabilityClass::Standard, false);

    assert_eq!(
        bindings.action_for(choice, KeyCode::Char('c'), KeyModifiers::CONTROL),
        Some(Action::MoveNextChoiceOption)
    );
    for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
        assert_eq!(
            bindings.action_for(choice, KeyCode::Char('c'), modifiers),
            Some(Action::ResolveChoiceCancelled),
            "a modified configured chord must leave the bare character alone"
        );
    }
}
