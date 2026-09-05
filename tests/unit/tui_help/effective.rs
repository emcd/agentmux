//! Presentation against the effective table: what a configuration does to the
//! help catalogue and to the pane hint strips.
//!
//! Separate from the hub's cases, which ask whether the catalogue covers every
//! surface. These ask a different question — whether what is presented agrees
//! with what a lookup would answer — and the way they fail is different too:
//! a chord advertised under a behavior it does not reach, rather than a
//! surface missing from the catalogue.

use agentmux::tui::{
    Action, BindingConfiguration, BindingContext, CapabilityClass, ConfiguredBinding,
    EffectiveBindings, HelpSection, default_binding, default_help_bindings, help_bindings,
    interaction_choice_hint, interaction_write_hint, picker_hint,
};
use crossterm::event::{KeyCode, KeyModifiers};

use super::{
    chord_space, entries, help_workbench, one_row, presented_lines, presented_under, row,
    workbench_with,
};

#[test]
fn an_unconfigured_run_presents_the_same_bindings_under_either_capability_class() {
    // This property used to be structural: the presentation functions took no
    // state, so no probe outcome could reach them. They now take the effective
    // table, which is built from the probe outcome, and that is deliberate --
    // a configuration may qualify a row by capability class, and presentation
    // has to show what is actually in force.
    //
    // What replaces the structural guarantee is this behavioral one, which is
    // the claim the change actually rests on: out of the box, nothing varies
    // by class. It can fail where the old one could not -- a capability field
    // added to the compiled rows, or a class-conditioned branch in generation,
    // breaks it, and the old test would have passed through either.
    let mut presented = None;
    for class in [CapabilityClass::Enhanced, CapabilityClass::Standard] {
        for on_macos in [false, true] {
            let bindings = EffectiveBindings::build(None, class, on_macos);
            let current = (
                help_bindings(&bindings),
                picker_hint(&bindings, BindingContext::PickerSessions),
                interaction_write_hint(&bindings),
                interaction_choice_hint(&bindings),
            );
            match &presented {
                None => presented = Some(current),
                Some(first) => assert_eq!(
                    &current, first,
                    "unconfigured presentation differs under {class:?}, on_macos={on_macos}"
                ),
            }
        }
    }
    let catalogue = presented.expect("the sweep runs at least once").0;
    assert!(!catalogue.is_empty(), "the catalogue is empty");

    // No presented chord names a modified Enter, which is the only chord the
    // probe outcome changes the delivery of. If the defaults ever became
    // capability-conditioned, this is the shape it would take.
    for entry in entries(&catalogue) {
        assert!(
            !entry.chords.contains("Shift+Enter") && !entry.chords.contains("Ctrl+Enter"),
            "presentation names a modified Enter in {:?}",
            entry.chords
        );
    }
}

#[test]
fn a_configured_rebinding_reaches_the_help_overlay_and_the_strip_that_advertises_it() {
    // Closing the picker is the case that exercises both surfaces at once: the
    // picker's hint strip advertises it and the help overlay catalogues it, so
    // one configured row is visible in both places or in neither.
    let closes_picker = |sections: &[HelpSection]| {
        presented_lines(sections)
            .into_iter()
            .find(|line| line.ends_with(": Close picker"))
            .expect("the catalogue presents closing the picker")
    };

    // Measured first, so what follows is against a baseline rather than
    // against a chord written down here.
    let before = closes_picker(&help_workbench().help_bindings());
    assert!(
        !before.starts_with("Ctrl+W"),
        "the fixture chord is already the default, so rebinding it proves nothing: {before}"
    );

    let workbench = workbench_with(Some(one_row(
        BindingContext::PickerBundles,
        "ctrl+w",
        Action::ClosePicker,
    )));

    // The overlay presents it, ahead of the compiled chords that still reach
    // the same behavior.
    let after = closes_picker(&workbench.help_bindings());
    assert!(
        after.starts_with("Ctrl+W"),
        "the configured chord does not lead the catalogue line: {after}"
    );

    // And the strip that advertises this behavior presents it too. A one-line
    // strip has room for one chord, so leading the line is what decides which.
    let strip = picker_hint(workbench.bindings(), BindingContext::PickerBundles)
        .into_iter()
        .find(|entry| entry.description == "Close picker")
        .expect("the picker strip advertises closing the picker");
    assert_eq!(strip.primary_chord(), "Ctrl+W");

    // Neither surface moved for a run that configured nothing, so the two
    // assertions above are about the configuration rather than about an edit
    // that changed the default.
    assert_eq!(closes_picker(&help_workbench().help_bindings()), before);
    assert_eq!(closes_picker(&default_help_bindings()), before);
}

#[test]
fn a_configured_chord_displaces_the_compiled_row_it_takes_over() {
    // The half that "the configured chord appears" leaves open. Binding `Esc`
    // in the message field to inserting a newline has to stop the overlay
    // saying `Esc` snaps chat history, or the operator reads a chord that now
    // does something else -- which is the defect the tier ordering exists to
    // prevent.
    //
    // The message field rather than the picker, because the picker's two
    // columns declare the same rows: configuring one column there leaves the
    // other's `Esc` standing, and the catalogue would rightly keep presenting
    // it. Snapping history is declared in one context only, so its
    // displacement is visible in the catalogue rather than masked by a sibling.
    const SNAPS: &str = ": Message: snap history";
    let before = presented_lines(&help_workbench().help_bindings());
    assert!(
        before.iter().any(|line| line == &format!("Esc{SNAPS}")),
        "Esc is not the sole default for snapping history, so displacing it \
         would not be observable: {before:#?}"
    );

    let workbench = workbench_with(Some(one_row(
        BindingContext::ComposeMessage,
        "esc",
        Action::InsertMessageNewline,
    )));
    let lines = presented_lines(&workbench.help_bindings());

    assert!(
        !lines.iter().any(|line| line.ends_with(SNAPS)),
        "the overlay still advertises a chord for snapping history: {lines:#?}"
    );

    // And the behavior that took the chord is presented under it, so the row
    // was moved rather than merely dropped.
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("Esc") && line.ends_with(": Message: insert newline")),
        "no line presents Esc as the chord that took it: {lines:#?}"
    );

    // The compiled table is untouched: a configuration is a run's, not a
    // build's.
    assert_eq!(
        default_binding(
            BindingContext::ComposeMessage,
            KeyCode::Esc,
            KeyModifiers::NONE
        ),
        Some(Action::SnapChatHistoryToLatest)
    );
}

/// The chord the tier cases bind. Nothing compiled reaches it in the write
/// pane, so what the catalogue says about it is the tiers' doing alone.
const TIER_CHORD: &str = "Ctrl+G";

/// The two behaviors the tier cases contend over, both declared by the write
/// pane's compiled rows, so a configuration may bind either there.
const LOSER: Action = Action::MoveRawwCursorLeft;
const WINNER: Action = Action::MoveRawwCursorRight;

/// What the catalogue and a lookup each say about the contended chord.
fn tier_outcome(bindings: &EffectiveBindings) -> (Vec<&'static str>, Option<Action>) {
    (
        presented_under(&help_bindings(bindings), TIER_CHORD),
        bindings.action_for(
            BindingContext::InteractionWrite,
            KeyCode::Char('g'),
            KeyModifiers::CONTROL,
        ),
    )
}

#[test]
fn the_catalogue_presents_a_contended_chord_only_under_the_row_that_wins_it() {
    // Presentation walks the configured and preset tiers; a lookup takes the
    // last matching row in a tier, and the configured tier before the preset
    // one. Where those two disagree the overlay names a chord under a behavior
    // it does not reach -- the same defect as advertising a compiled row a
    // configuration took over, arriving from the other direction.
    //
    // Neither arrangement is reachable through a shipped set: the loader
    // refuses a configuration binding one chord twice, and the sets that ship
    // bind one context that neither of these touches. Both are reachable
    // through a `BindingConfiguration` carrying binding-set rows directly, so
    // the precedence rule is a property of the table rather than something the
    // shipped sets stand in for.
    let write = BindingContext::InteractionWrite;
    let build = |preset_rows: Vec<ConfiguredBinding>, rows: Vec<ConfiguredBinding>| {
        EffectiveBindings::build(
            Some(&BindingConfiguration {
                presets: Vec::new(),
                preset_rows,
                primary_modifier_on_macos: None,
                rows,
            }),
            CapabilityClass::Standard,
            false,
        )
    };

    for (arrangement, bindings) in [
        (
            "a later binding-set row supersedes an earlier one",
            build(
                vec![row(write, "ctrl+g", LOSER), row(write, "ctrl+g", WINNER)],
                Vec::new(),
            ),
        ),
        (
            "a configured row supersedes a binding-set row",
            build(
                vec![row(write, "ctrl+g", LOSER)],
                vec![row(write, "ctrl+g", WINNER)],
            ),
        ),
    ] {
        let (presented, resolved) = tier_outcome(&bindings);
        assert_eq!(
            resolved,
            Some(WINNER),
            "{arrangement}: the premise fails -- the lookup does not reach the winner"
        );
        assert_eq!(
            presented,
            vec![WINNER.describe()],
            "{arrangement}: the catalogue presents {TIER_CHORD} under something \
             other than the single behavior it reaches"
        );
    }
}

#[test]
fn a_chord_no_tier_contends_is_presented_once_under_the_row_that_binds_it() {
    // The control for the case above. Without it, a projection that dropped
    // every higher-tier row would satisfy the equality there by presenting
    // nothing at all.
    let bindings = EffectiveBindings::build(
        Some(&one_row(BindingContext::InteractionWrite, "ctrl+g", WINNER)),
        CapabilityClass::Standard,
        false,
    );
    let (presented, resolved) = tier_outcome(&bindings);
    assert_eq!(resolved, Some(WINNER));
    assert_eq!(presented, vec![WINNER.describe()]);

    // And nothing presents it before a configuration binds it, so the line
    // above is the configuration's doing.
    assert!(presented_under(&default_help_bindings(), TIER_CHORD).is_empty());
}

#[test]
fn a_compiled_row_a_configuration_took_over_is_not_advertised_under_its_old_behavior() {
    // The `Chord::AnyModifiers` case, which renders as the bare key and matches
    // that key under every modifier. Configuring the bare key takes the
    // keystroke the row is *written* as, so the row may not keep printing it.
    //
    // What outlasts the row is the modified forms: `Shift+Up` still reaches the
    // compiled behavior. Those go unadvertised rather than being spelled out --
    // "any modifier" has no finite spelling -- and that silence is asserted
    // here so it stays a known consequence rather than an assumption.
    let write = BindingContext::InteractionWrite;
    let navigates = Action::NavigateInteractionUp.describe();
    assert_eq!(
        default_binding(write, KeyCode::Up, KeyModifiers::NONE),
        Some(Action::NavigateInteractionUp),
        "the premise fails -- plain Up does not reach the behavior being displaced"
    );

    let bindings = EffectiveBindings::build(
        Some(&one_row(write, "up", Action::MoveRawwCursorLeft)),
        CapabilityClass::Standard,
        false,
    );

    // Dispatch: the configuration takes the bare key, the compiled row keeps
    // the modified ones.
    assert_eq!(
        bindings.action_for(write, KeyCode::Up, KeyModifiers::NONE),
        Some(Action::MoveRawwCursorLeft)
    );
    assert_eq!(
        bindings.action_for(write, KeyCode::Up, KeyModifiers::SHIFT),
        Some(Action::NavigateInteractionUp)
    );

    // Presentation: "Up" is presented under the configured behavior and under
    // nothing else, and the displaced behavior is no longer offered a chord in
    // this context.
    let sections = help_bindings(&bindings);
    assert_eq!(
        presented_under(&sections, "Up")
            .into_iter()
            .filter(|description| *description == navigates)
            .count(),
        0,
        "the overlay still advertises Up as reaching {navigates:?}"
    );
    assert!(
        !sections
            .iter()
            .flat_map(|section| section.entries.iter())
            .any(|entry| entry.description == navigates && entry.covers(write)),
        "the write pane still advertises a chord for {navigates:?}"
    );

    // The other contexts that declare the same behavior are untouched: a
    // configured row is scoped to the context that named it.
    assert_eq!(
        default_help_bindings()
            .iter()
            .flat_map(|section| section.entries.iter())
            .filter(|entry| entry.description == navigates)
            .count(),
        1,
        "the defaults no longer present the displaced behavior at all"
    );
}

#[test]
fn the_picker_strip_advertises_only_chords_that_work_on_the_focused_column() {
    // A strip that read one fixed column advertised "Ctrl+W Close picker" on
    // both, while the chord did nothing with the Sessions column focused:
    // dispatch was right and the strip was not.
    //
    // The compiled table declares the same rows in both columns, so reading
    // either answered for both and the difference could not show. A
    // configuration can bind one column alone, and then the column a strip
    // reads from is the whole of whether it tells the truth.
    let bundles_only = one_row(BindingContext::PickerBundles, "ctrl+w", Action::ClosePicker);
    let workbench = workbench_with(Some(bundles_only));
    let bindings = workbench.bindings();

    let advertised = |focused| {
        picker_hint(bindings, focused)
            .into_iter()
            .find(|entry| entry.description == "Close picker")
            .map(|entry| entry.primary_chord().to_string())
            .expect("both picker columns still close the picker")
    };

    // The column that names the chord leads with it; the column that does not
    // keeps the chord it actually answers to.
    assert_eq!(advertised(BindingContext::PickerBundles), "Ctrl+W");
    assert_eq!(advertised(BindingContext::PickerSessions), "Esc");

    // The property behind both, put where it can fail: every chord a column's
    // strip prints is *resolved* in that column, through the same table
    // dispatch reads, and must reach the behavior printed beside it.
    //
    // Asserting the printed chord came from the focused context would not be
    // this property. It would check where the row was filed, and the defect
    // was a row filed correctly and advertised on a surface it did not answer
    // for. Only resolving the keystroke distinguishes the two.
    for focused in [
        BindingContext::PickerBundles,
        BindingContext::PickerSessions,
    ] {
        let strip = picker_hint(bindings, focused);
        assert!(!strip.is_empty(), "{focused:?} advertises nothing");
        let mut resolved_any = false;
        for entry in strip {
            let printed = entry.primary_chord();
            let source = entry
                .sources
                .iter()
                .find(|source| source.shown && source.chord == printed)
                .unwrap_or_else(|| panic!("no source backs {printed:?}"));

            // The two `Enter` entries are the deliberate exception. They are
            // read from the column they describe rather than the focused one,
            // because `Enter` means something different in each and conveying
            // that is why the strip exists -- so exactly one of them names a
            // behavior the focused column does not reach, by design.
            if matches!(
                entry.description,
                "Bundle col: switch bundle" | "Session col: insert or open look"
            ) {
                continue;
            }

            let reached = chord_space()
                .into_iter()
                .filter(|(code, modifiers)| source.matches(*code, *modifiers))
                .find_map(|(code, modifiers)| bindings.action_for(focused, code, modifiers))
                .unwrap_or_else(|| {
                    panic!("{focused:?} advertises {printed:?}, which resolves to nothing there")
                });
            assert_eq!(
                reached.describe(),
                entry.description,
                "{focused:?} advertises {printed:?} as {:?}, but there it reaches {reached:?}",
                entry.description
            );
            resolved_any = true;
        }
        // Without this the loop above is vacuous on a strip that carried only
        // the two exempt entries.
        assert!(
            resolved_any,
            "{focused:?} advertises nothing outside the two Enter entries, so \
             nothing above was resolved"
        );
    }
}
