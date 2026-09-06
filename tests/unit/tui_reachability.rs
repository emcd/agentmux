//! What a configuration leaves reachable, per terminal capability class.
//!
//! These ask the question pre-flight asks: not what the overlay presents, but
//! whether any chord at all still arrives at a behavior a context declares.
//! Built directly rather than through a loader so both capability classes and
//! both platforms are exercisable without a terminal.
//!
//! The fixtures are shaped by which compiled chord shape a behavior sits on,
//! and that is not incidental. A row written as one exact keystroke is lost
//! when that keystroke is claimed. A row matching a key under any modifier, or
//! a character under any set containing `Ctrl`, is lost only when every such
//! set is claimed — and since two of the six modifier flags a terminal can
//! report cannot be spelled in the configuration grammar, that never happens.
//! Behaviors on those shapes are therefore permanently reachable, which is why
//! every fixture below that produces a finding sits on the `Enter` family.

use agentmux::tui::{
    Action, AffectedClasses, BindingConfiguration, BindingContext, CapabilityClass,
    ConfiguredAction, ConfiguredBinding, PrimaryModifier, UnreachableAction, default_binding,
    parse_chord, quit_unreachable, unreachable_actions,
};
use crossterm::event::{KeyCode, KeyModifiers};

const BOTH: &[CapabilityClass] = &[CapabilityClass::Enhanced, CapabilityClass::Standard];

/// One configured row, speaking for the classes given.
fn row(
    context: BindingContext,
    chord: &str,
    action: ConfiguredAction,
    classes: &[CapabilityClass],
) -> ConfiguredBinding {
    let mut row = ConfiguredBinding {
        context,
        chord: parse_chord(chord).expect("chord parses"),
        enhanced: None,
        standard: None,
    };
    for class in classes {
        match class {
            CapabilityClass::Enhanced => row.enhanced = Some(action),
            CapabilityClass::Standard => row.standard = Some(action),
        }
    }
    row
}

fn configuration(rows: Vec<ConfiguredBinding>) -> BindingConfiguration {
    BindingConfiguration {
        rows,
        ..BindingConfiguration::default()
    }
}

/// Every modifier set the grammar can spell that contains `Ctrl`.
///
/// Eight of them, and a compiled control chord matches all eight. Claiming
/// fewer leaves the behavior reachable through whichever set was left, which is
/// what dispatch does and therefore what this has to agree with.
fn control_spellings(character: char) -> Vec<String> {
    [
        vec![],
        vec!["cmd"],
        vec!["alt"],
        vec!["shift"],
        vec!["cmd", "alt"],
        vec!["cmd", "shift"],
        vec!["alt", "shift"],
        vec!["cmd", "alt", "shift"],
    ]
    .into_iter()
    .map(|extras| {
        let mut parts = vec!["ctrl"];
        parts.extend(extras);
        format!("{}+{character}", parts.join("+"))
    })
    .collect()
}

/// Every modifier set the grammar can spell, applied to one key.
///
/// Sixteen: the four literal modifiers in every combination. The point of the
/// list is what it leaves out — a terminal can report `Hyper` and `Meta` too,
/// and no configuration can name them.
fn grammar_spellings(key: &str) -> Vec<String> {
    ["ctrl", "alt", "shift", "cmd"]
        .iter()
        .fold(vec![String::new()], |spellings, modifier| {
            spellings
                .iter()
                .flat_map(|prefix| [prefix.clone(), format!("{prefix}{modifier}+")])
                .collect()
        })
        .into_iter()
        .map(|prefix| format!("{prefix}{key}"))
        .collect()
}

/// Rows claiming every one of those spellings, which is as far as a
/// configuration can go against a compiled control chord.
fn claim_every_control_spelling(
    context: BindingContext,
    character: char,
    action: ConfiguredAction,
    classes: &[CapabilityClass],
) -> Vec<ConfiguredBinding> {
    control_spellings(character)
        .iter()
        .map(|chord| row(context, chord, action, classes))
        .collect()
}

/// The three chords compose binds sending to, each written as one exact
/// keystroke, so claiming all three is what takes sending off the field.
const SENDING_CHORDS: [&str; 3] = ["enter", "shift+enter", "ctrl+enter"];

/// Rows claiming all three of those for one behavior.
fn claim_sending(action: ConfiguredAction, classes: &[CapabilityClass]) -> BindingConfiguration {
    configuration(
        SENDING_CHORDS
            .iter()
            .map(|chord| row(BindingContext::ComposeMessage, chord, action, classes))
            .collect(),
    )
}

fn findings(configuration: &BindingConfiguration) -> Vec<UnreachableAction> {
    unreachable_actions(Some(configuration), false)
}

fn finding_for(
    configuration: &BindingConfiguration,
    context: BindingContext,
    action: Action,
) -> Option<AffectedClasses> {
    findings(configuration)
        .into_iter()
        .find(|finding| finding.context == context && finding.action == action)
        .map(|finding| finding.classes)
}

/// Nothing configured leaves every declared behavior reachable everywhere. The
/// control the rest of this file rests on: without it, a computation that
/// reported nothing at all would satisfy every "is not reported" assertion
/// below.
#[test]
fn the_compiled_table_alone_leaves_nothing_unreachable() {
    for on_macos in [false, true] {
        let findings = unreachable_actions(None, on_macos);
        assert!(
            findings.is_empty(),
            "the shipped defaults leave a behavior unreachable (on_macos={on_macos}): {findings:?}"
        );
    }
}

/// Losing a binding does not require an explicit unbinding: binding the chords
/// that already carried a behavior displaces it.
#[test]
fn displacing_every_chord_of_a_behavior_is_reported_under_both_classes() {
    let compose = BindingContext::ComposeMessage;
    for chord in SENDING_CHORDS {
        let (code, modifiers) = parse_chord(chord)
            .expect("chord parses")
            .resolve(KeyModifiers::CONTROL);
        assert_eq!(
            default_binding(compose, code, modifiers),
            Some(Action::SendMessage),
            "{chord} is not one of the compiled chords this test displaces"
        );
    }

    let configuration = claim_sending(ConfiguredAction::Invoke(Action::InsertMessageNewline), BOTH);
    assert_eq!(
        finding_for(&configuration, compose, Action::SendMessage),
        Some(AffectedClasses::Both)
    );
}

/// Declaring the chord against no action is how an operator says the removal
/// was meant. The report describes the outcome either way, so it reads the
/// same as the displacement above — this is the pair that establishes it does.
#[test]
fn an_explicit_unbinding_is_reported_exactly_as_a_displacement_is() {
    let compose = BindingContext::ComposeMessage;
    let displaced = claim_sending(ConfiguredAction::Invoke(Action::InsertMessageNewline), BOTH);
    let emptied = claim_sending(ConfiguredAction::Unbound, BOTH);

    assert_eq!(
        finding_for(&emptied, compose, Action::SendMessage),
        Some(AffectedClasses::Both)
    );
    assert_eq!(
        finding_for(&emptied, compose, Action::SendMessage),
        finding_for(&displaced, compose, Action::SendMessage),
        "an intended removal and a displacement are reported differently"
    );
}

/// A class-qualified row leaves the other class alone, so the finding names one
/// class rather than both.
#[test]
fn a_finding_holding_under_one_class_names_that_class() {
    let configuration = claim_sending(
        ConfiguredAction::Invoke(Action::InsertMessageNewline),
        &[CapabilityClass::Enhanced],
    );
    assert_eq!(
        finding_for(
            &configuration,
            BindingContext::ComposeMessage,
            Action::SendMessage
        ),
        Some(AffectedClasses::Only(CapabilityClass::Enhanced)),
        "a row speaking for one class was reported against the other too"
    );
}

/// A behavior with other chords in the same context survives losing one, so
/// displacing a single chord is not by itself a finding.
#[test]
fn a_behavior_reachable_by_another_chord_is_not_reported() {
    let compose = BindingContext::ComposeMessage;
    let configuration = configuration(vec![row(
        compose,
        "enter",
        ConfiguredAction::Invoke(Action::InsertMessageNewline),
        &[CapabilityClass::Enhanced],
    )]);
    assert_eq!(
        finding_for(&configuration, compose, Action::SendMessage),
        None,
        "sending was reported lost while two other chords still reach it"
    );
}

/// The class-aware half of that: under the class that cannot deliver a modified
/// Enter, the chords sending would fall back to are chords no keystroke
/// satisfies, so the same arrangement IS a finding there.
///
/// This is the case a reachability sweep over every representable keystroke
/// gets wrong, by accepting a row that cannot be typed as an answer.
#[test]
fn a_chord_the_class_cannot_deliver_does_not_count_as_reachable() {
    let compose = BindingContext::ComposeMessage;
    let configuration = configuration(vec![row(
        compose,
        "enter",
        ConfiguredAction::Invoke(Action::InsertMessageNewline),
        BOTH,
    )]);
    assert_eq!(
        finding_for(&configuration, compose, Action::SendMessage),
        Some(AffectedClasses::Only(CapabilityClass::Standard)),
        "moving sending onto modified Enter alone should be a finding exactly \
         where a modified Enter cannot arrive"
    );
}

/// A compiled row matching a key under any modifier keeps answering under every
/// combination a configuration did not claim, combinations of two included.
/// Claiming the five single-modifier states leaves eleven others.
#[test]
fn a_row_matching_any_modifier_survives_the_single_modifier_states_being_claimed() {
    let compose = BindingContext::ComposeMessage;
    assert_eq!(
        default_binding(compose, KeyCode::Esc, KeyModifiers::NONE),
        Some(Action::SnapChatHistoryToLatest),
        "the compiled row this test claims is not the one it assumes"
    );

    let configuration = configuration(
        ["esc", "shift+esc", "ctrl+esc", "alt+esc", "cmd+esc"]
            .iter()
            .map(|chord| {
                row(
                    compose,
                    chord,
                    ConfiguredAction::Invoke(Action::MoveMessageCursorUp),
                    BOTH,
                )
            })
            .collect(),
    );
    assert_eq!(
        finding_for(&configuration, compose, Action::SnapChatHistoryToLatest),
        None,
        "a row matching any modifier was reported lost while Ctrl+Shift+Esc still reaches it"
    );
}

#[test]
fn quit_stays_reachable_under_the_shipped_defaults() {
    for on_macos in [false, true] {
        assert_eq!(quit_unreachable(None, on_macos), None);
    }
}

/// Rebinding the bare quit chord does not lose quit: the compiled row matches
/// any set containing `Ctrl`, so `Ctrl+Shift+C` still quits, and dispatch says
/// so. Refusing the file here would refuse one that works.
#[test]
fn rebinding_the_bare_quit_chord_leaves_quit_reachable() {
    let configuration = configuration(vec![row(
        BindingContext::Global,
        "ctrl+c",
        ConfiguredAction::Invoke(Action::ToggleHelpOverlay),
        BOTH,
    )]);
    assert_eq!(quit_unreachable(Some(&configuration), false), None);
}

/// No configuration the grammar can express takes quit away.
///
/// The compiled quit row matches every set containing `Ctrl`, and two of the
/// six modifier flags are ones the grammar cannot spell, so `Ctrl+Hyper+C` is
/// unclaimable and still quits however much of the rest is claimed.
///
/// So the refusal is a guard this table cannot trip. It is kept because the
/// requirement asks for it and because a quit row written as one exact
/// keystroke would trip it — but that is a fact about the compiled table rather
/// than about this code, and it is asserted here rather than left to be
/// rediscovered.
#[test]
fn no_configuration_the_grammar_can_express_takes_quit_away() {
    for classes in [
        BOTH,
        &[CapabilityClass::Standard][..],
        &[CapabilityClass::Enhanced][..],
    ] {
        let configuration = configuration(claim_every_control_spelling(
            BindingContext::Global,
            'c',
            ConfiguredAction::Unbound,
            classes,
        ));
        for on_macos in [false, true] {
            assert_eq!(
                quit_unreachable(Some(&configuration), on_macos),
                None,
                "{classes:?} on_macos={on_macos}: Ctrl+Hyper+C is unclaimable and still quits"
            );
        }
    }
}

/// A modifier the grammar cannot spell still reaches a compiled row matching on
/// the key alone, so claiming every spelling an operator can write does not
/// take the behavior away.
///
/// A statement about dispatch rather than about ergonomics: `Hyper+Esc` is a
/// keystroke almost no keyboard produces, and the table answers it all the
/// same. Reachability has to agree with the table, because what it decides is
/// whether a configuration is refused.
#[test]
fn a_modifier_the_grammar_cannot_spell_keeps_a_compiled_row_reachable() {
    let compose = BindingContext::ComposeMessage;
    let claimed: Vec<ConfiguredBinding> = grammar_spellings("esc")
        .iter()
        .map(|chord| {
            row(
                compose,
                chord,
                ConfiguredAction::Invoke(Action::MoveMessageCursorUp),
                BOTH,
            )
        })
        .collect();
    assert_eq!(
        claimed.len(),
        16,
        "the grammar spells sixteen modifier sets, and all of them are claimed here"
    );

    assert_eq!(
        finding_for(
            &configuration(claimed),
            compose,
            Action::SnapChatHistoryToLatest
        ),
        None,
        "every spelling the grammar has was claimed, and Hyper+Esc still reaches \
         the compiled row"
    );
}

/// The symbolic modifier resolves as the tables are built, so which compiled
/// row a configuration claims depends on the platform reading it.
#[test]
fn which_chord_the_symbolic_modifier_claims_follows_the_platform() {
    let compose = BindingContext::ComposeMessage;
    // Bare and shifted Enter are claimed literally and the third symbolically.
    // Off macOS that resolves to Ctrl+Enter and covers all three sending rows;
    // on macOS with Cmd selected it lands elsewhere, leaving Ctrl+Enter sending.
    let mut configuration = configuration(
        ["enter", "shift+enter", "primary+enter"]
            .iter()
            .map(|chord| {
                row(
                    compose,
                    chord,
                    ConfiguredAction::Invoke(Action::InsertMessageNewline),
                    BOTH,
                )
            })
            .collect(),
    );
    configuration.primary_modifier_on_macos = Some(PrimaryModifier::Command);

    let sending_lost = |on_macos| {
        unreachable_actions(Some(&configuration), on_macos)
            .into_iter()
            .find(|finding| finding.context == compose && finding.action == Action::SendMessage)
            .map(|finding| finding.classes)
    };
    assert_eq!(
        sending_lost(false),
        Some(AffectedClasses::Both),
        "off macOS the symbolic modifier resolves to Ctrl and claims the third row"
    );
    assert_eq!(
        sending_lost(true),
        Some(AffectedClasses::Only(CapabilityClass::Standard)),
        "resolved to Cmd it leaves Ctrl+Enter sending, which only a terminal that \
         can deliver a modified Enter is able to use"
    );
}

/// A global row shadows a contextual one, so displacing a behavior does not
/// require binding anything in the context that declares it.
///
/// Dispatch consults the global rows before the active surface's and stops at
/// the first that answers. A sweep asking the context alone sees compose's own
/// `Enter` row still bound and reports nothing, while the keystroke never
/// reaches it.
///
/// Under the class that can deliver a modified `Enter`, sending survives on the
/// two rows the global chord does not name — which is what makes this a
/// single-class finding rather than a total loss, and what would make a sweep
/// that ignored capability class disagree here too.
#[test]
fn a_global_row_shadowing_a_contextual_chord_is_reported() {
    let compose = BindingContext::ComposeMessage;
    assert_eq!(
        default_binding(BindingContext::Global, KeyCode::Enter, KeyModifiers::NONE),
        None,
        "the global context already binds Enter, so this fixture displaces \
         something other than what it means to"
    );
    assert_eq!(
        default_binding(compose, KeyCode::Enter, KeyModifiers::NONE),
        Some(Action::SendMessage),
        "compose does not bind bare Enter to sending, so there is nothing to shadow"
    );

    let configuration = configuration(vec![row(
        BindingContext::Global,
        "enter",
        ConfiguredAction::Invoke(Action::Quit),
        BOTH,
    )]);
    assert_eq!(
        finding_for(&configuration, compose, Action::SendMessage),
        Some(AffectedClasses::Only(CapabilityClass::Standard)),
        "a global row took bare Enter, and where a modified Enter cannot arrive \
         nothing else reaches sending"
    );
}

/// The other half of the lookup order: a global row that empties a chord does
/// not silence it, because dispatch consults the surface next.
///
/// Without this, modelling the order as "the global rows win" rather than as
/// "the first context that answers wins" would pass every other assertion here.
#[test]
fn a_global_unbinding_uncovers_the_contextual_row_it_shadowed() {
    let compose = BindingContext::ComposeMessage;
    let configuration = configuration(vec![row(
        BindingContext::Global,
        "enter",
        ConfiguredAction::Unbound,
        BOTH,
    )]);
    assert_eq!(
        finding_for(&configuration, compose, Action::SendMessage),
        None,
        "an emptied global chord falls through to the surface, which still sends"
    );
}

/// A global row can be the only thing reaching a behavior the surface declares,
/// so candidates drawn from the surface alone report it lost while it answers.
///
/// The surface's own chords for sending are claimed first, which is a finding on
/// its own — the control below asserts exactly that, so what the global row
/// changes is visible rather than assumed. Adding a global row invoking the same
/// behavior makes it reachable again while compose is active, because the global
/// rows are consulted first and hold with any surface up. `Ctrl+G` appears in no
/// compose row, so it is a keystroke a surface-only candidate set never asks
/// about.
///
/// Sending is the behavior because compose declares it on three rows each
/// written as one exact keystroke, which a configuration can claim in full. A
/// behavior sitting on a row that matches a key under any modifier cannot be
/// taken off its surface at all, so no fixture built on one can put the global
/// row on the critical path.
///
/// Built through [`BindingConfiguration`] directly rather than through a file.
/// The loader would refuse a global row naming a compose behavior, since it
/// admits only the actions the global context declares — but `EffectiveBindings`
/// is public and answers for rows that never passed the loader, and it is that
/// answer this pins.
#[test]
fn a_global_row_can_be_the_only_thing_reaching_a_surface_behavior() {
    let compose = BindingContext::ComposeMessage;
    let displaced: Vec<ConfiguredBinding> = SENDING_CHORDS
        .iter()
        .map(|chord| {
            row(
                compose,
                chord,
                ConfiguredAction::Invoke(Action::InsertMessageNewline),
                BOTH,
            )
        })
        .collect();
    assert_eq!(
        finding_for(
            &configuration(displaced.clone()),
            compose,
            Action::SendMessage
        ),
        Some(AffectedClasses::Both),
        "the control failed: claiming compose's three sending chords is supposed \
         to take sending off the surface, and the global row below is what puts \
         it back"
    );

    let mut rescued = displaced;
    rescued.push(row(
        BindingContext::Global,
        "ctrl+g",
        ConfiguredAction::Invoke(Action::SendMessage),
        BOTH,
    ));
    assert_eq!(
        finding_for(&configuration(rescued), compose, Action::SendMessage),
        None,
        "a global Ctrl+G sends while compose is active, and sending was still \
         reported unreachable there"
    );
}

/// A finding names the context it holds in and no other, so a report cannot
/// implicate a context whose rows were never touched.
#[test]
fn a_finding_names_the_context_it_holds_in() {
    let configuration = claim_sending(ConfiguredAction::Unbound, BOTH);
    let reported: Vec<BindingContext> = findings(&configuration)
        .into_iter()
        .map(|finding| finding.context)
        .collect();
    assert_eq!(reported, vec![BindingContext::ComposeMessage]);
}
