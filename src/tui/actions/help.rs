//! Generated presentation of the binding table.
//!
//! Help is a catalogue, not a lookup. Dispatch asks which single context owns a
//! chord right now; help asks what every context offers, and must answer the
//! same way wherever the operator opened it from. The two rules are therefore
//! separate functions, and this one takes no `AppState` at all -- which is what
//! makes the identical-content property structural rather than something a test
//! has to keep watch over.
//!
//! Rows are grouped by the behavior they reach rather than by the chord that
//! reaches it, so a context's several `Enter` rows fold onto one line instead
//! of printing three times. That folding is why the wording of a behavior has
//! to say which pane it belongs to: the chord alone cannot separate `Enter` in
//! the write pane from `Enter` in the choice pane.
//!
//! What is presented is the **effective** table -- what an operator configured
//! over what ships -- rather than the compiled rows alone. An operator's first
//! move after rebinding a chord is to open the overlay and check it took, so a
//! catalogue that could only describe the defaults would be wrong for exactly
//! the reader most likely to consult it. [`default_help_bindings`] is the one
//! projection that stays on the compiled rows, for generated documentation,
//! which has no operator's configuration to speak for.

use crossterm::event::{KeyCode, KeyModifiers};

use super::action::Action;
use super::bindings::{BoundAction, ContextRow, DisplaySection, default_rows, default_section};
use super::chord::Chord;
use super::context::BindingContext;
use super::effective::EffectiveBindings;

/// One row that contributed to a presented binding.
///
/// Provenance exists so presentation can be checked against the table rather
/// than against a copy of it. Without it, a context dropped from
/// [`help_contexts`] is invisible from outside: its behaviors are mostly
/// described identically by some other context, so every description still
/// appears and only the chords quietly go missing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HelpSource {
    pub context: BindingContext,
    pub chord: String,
    /// Whether this chord appears in the entry's [`HelpEntry::chords`]. False
    /// where another source already put the same text on the line, or where a
    /// modified `Enter` was folded into the bare one it matches.
    pub shown: bool,
    /// The row's own pattern, kept so a caller can ask which source answers a
    /// key without this type restating the pattern vocabulary publicly. A
    /// displayed chord string is not enough for that: two rows in one context
    /// can reach the same behavior, so matching on context and behavior alone
    /// cannot tell which of them is present.
    pattern: Chord,
}

impl HelpSource {
    /// Whether the row behind this source is one that answers this key.
    pub fn matches(&self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        self.pattern.matches(code, modifiers)
    }
}

/// One presented binding: every chord that reaches a behavior, and the
/// behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HelpEntry {
    /// The chords shown, in the order the table declares them, joined for
    /// display.
    pub chords: String,
    pub description: &'static str,
    /// Every row behind this entry, shown or folded.
    pub sources: Vec<HelpSource>,
}

impl HelpEntry {
    /// Whether this entry carries a binding declared by `context`.
    pub fn covers(&self, context: BindingContext) -> bool {
        self.sources.iter().any(|source| source.context == context)
    }

    /// The first chord shown for this behavior.
    ///
    /// The catalogue lists every chord that reaches a behavior; a one-line hint
    /// strip has room for one, and the first is the one the table declares
    /// first.
    pub fn primary_chord(&self) -> &str {
        self.chords.split(" / ").next().unwrap_or(&self.chords)
    }

    /// The description without the pane or field it opens with.
    ///
    /// Descriptions are written for the catalogue, where `Enter` appears for
    /// several surfaces at once and the qualifier is what separates them. A
    /// hint strip sits on one surface and has already established it, so the
    /// qualifier is noise there.
    pub fn detail(&self) -> &'static str {
        self.description
            .split_once(": ")
            .map_or(self.description, |(_, detail)| detail)
    }
}

/// One heading of the help overlay and the bindings filed under it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HelpSection {
    pub heading: &'static str,
    pub entries: Vec<HelpEntry>,
}

impl DisplaySection {
    fn heading(self) -> &'static str {
        match self {
            Self::Modes => "Modes",
            Self::Communication => "Communication Mode",
            Self::Interaction => "Interaction Mode",
            Self::Picker => "Picker",
            Self::Help => "Help Overlay",
        }
    }

    /// The sections in the order they are presented, which is the order they
    /// are declared in.
    const ALL: [DisplaySection; 5] = [
        DisplaySection::Modes,
        DisplaySection::Communication,
        DisplaySection::Interaction,
        DisplaySection::Picker,
        DisplaySection::Help,
    ];
}

/// The contexts help presents, in the order it presents them.
///
/// Distinct from [`super::binding_context`] in both respects. Every context
/// contributes, because a binding the operator cannot currently reach is still
/// one they need to read about; and taking no state is the point of the
/// signature rather than an omission, since it is what makes the catalogue the
/// same wherever help was opened from.
///
/// The order is presentation's own, not [`BindingContext::ALL`]'s. `ALL` is
/// declaration order and pairs each surface with its dispatch precedence; a
/// reader instead wants the two compose fields adjacent, the write pane before
/// the choice pane that replaces it, and the overlays last, since they
/// contribute only the mode-switching rows every surface already shares.
pub(crate) fn help_contexts() -> [BindingContext; 9] {
    [
        BindingContext::Global,
        BindingContext::ComposeTo,
        BindingContext::ComposeMessage,
        BindingContext::InteractionWrite,
        BindingContext::InteractionChoice,
        BindingContext::PickerBundles,
        BindingContext::PickerSessions,
        BindingContext::EventsOverlay,
        BindingContext::HelpOverlay,
    ]
}

/// The whole effective table as the help overlay presents it: sections in
/// declaration order, and within each, one entry per behavior in the order the
/// table first reaches it.
pub fn help_bindings(bindings: &EffectiveBindings) -> Vec<HelpSection> {
    DisplaySection::ALL
        .into_iter()
        .map(|section| HelpSection {
            heading: section.heading(),
            entries: entries_for(bindings, section),
        })
        .collect()
}

/// The catalogue the compiled defaults alone produce.
///
/// This is what generated operator documentation reads, and it takes no
/// effective table for that reason rather than by omission. The guide is
/// committed to the repository and read by operators who have not written a
/// configuration; a reference generated from whichever configuration the
/// generating machine happened to carry would document one operator's TUI as
/// though it were everyone's, and nothing about the emitted text would say so.
/// Having no parameter to pass is what makes that unwritable.
#[must_use]
pub fn default_help_bindings() -> Vec<HelpSection> {
    help_bindings(&EffectiveBindings::default())
}

/// Every binding one context declares, as presented.
///
/// Where [`help_bindings`] answers for the whole surface, this answers for a
/// single context. That asymmetry is deliberate and is the difference between
/// the help overlay and a pane hint strip: help is a catalogue of everything,
/// a strip annotates the one surface it sits on.
pub fn context_bindings(bindings: &EffectiveBindings, context: BindingContext) -> Vec<HelpEntry> {
    finish(fold_rows(
        rows_of(bindings, context)
            .into_iter()
            .map(|row| (context, row)),
    ))
}

/// The presented binding for one behavior in one context, or `None` where that
/// context does not bind it.
pub fn binding_for(
    bindings: &EffectiveBindings,
    context: BindingContext,
    action: Action,
) -> Option<HelpEntry> {
    one(bindings, context, BoundAction::Fixed(action))
}

/// Every behavior this context's compiled rows declare, without duplicates.
///
/// The table declares a behavior only in the contexts where it has an effect,
/// so this is exactly the set that does something on that surface. A
/// configuration is held to it: binding anything else would produce a row
/// generated help advertises and that does nothing when pressed.
///
/// Typing rows contribute nothing here. They carry the operator's own
/// character rather than a behavior named in advance, and are outside the
/// configurable vocabulary for the same reason.
///
/// The compiled rows and not the effective table, which is not an oversight in
/// the one function here that still reads them. The question is what a context
/// is *capable* of, which the shipped table decides; answering it from a table
/// an operator's own rows contributed to would let a configuration widen the
/// set it is being checked against.
///
/// Crate-internal: this answers a question the configuration loader asks while
/// validating a binding group, not one a host outside the crate has reason to
/// ask. Exporting it would commit the public surface to returning a `Vec` of
/// behaviors for a caller that does not exist.
pub(crate) fn context_actions(context: BindingContext) -> Vec<Action> {
    let mut actions = Vec::new();
    for row in default_rows(context) {
        if let BoundAction::Fixed(action) = row.action
            && !actions.contains(&action)
        {
            actions.push(action);
        }
    }
    actions
}

/// The presented binding for typing an ordinary character into this context's
/// draft, or `None` where the context takes no typed text.
pub fn typing_binding(bindings: &EffectiveBindings, context: BindingContext) -> Option<HelpEntry> {
    fold_rows(
        rows_of(bindings, context)
            .into_iter()
            .map(|row| (context, row)),
    )
    .into_iter()
    .find(|(action, _)| matches!(action, BoundAction::Typed(_)))
    .map(entry_of)
}

/// The bindings the picker's hint strip advertises, for the column that
/// currently holds focus.
///
/// Which few behaviors are worth a one-line strip is an editorial judgment and
/// stays declared here; their chords and their wording are not, and come from
/// the table.
///
/// Both columns contribute their own `Enter`, because `Enter` means something
/// different in each and conveying that is why the strip exists. The behaviors
/// that belong to the picker as a whole -- switching column, closing it -- are
/// read from `focused` instead, and that distinction is load-bearing rather
/// than tidiness.
///
/// A binding is scoped to the context that declares it. The compiled table
/// declares the same rows in both columns, so reading either answered for both
/// and the difference was invisible; a configuration can bind a chord in one
/// column alone, and then a strip that answered from the other would print a
/// chord that does nothing where the operator is standing. Reading from the
/// focused column is what keeps the strip's claim true of the surface it is
/// drawn on.
///
/// `focused` is expected to be a picker column. Anything else has no picker
/// rows to answer with, so the strip comes back empty rather than borrowing
/// another surface's chords.
pub fn picker_hint(bindings: &EffectiveBindings, focused: BindingContext) -> Vec<HelpEntry> {
    [
        binding_for(bindings, focused, Action::TogglePickerFocus),
        binding_for(
            bindings,
            BindingContext::PickerBundles,
            Action::CommitPickerBundle,
        ),
        binding_for(
            bindings,
            BindingContext::PickerSessions,
            Action::CommitPickerSession,
        ),
        binding_for(bindings, focused, Action::ClosePicker),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The bindings the interaction choice pane's hint advertises.
///
/// Only the two decisions, where the write pane's hint also advertises typing.
/// The pane names its bindings in a one-line block title, and the four
/// navigation rows would take it past the width of an ordinary terminal --
/// where a title does not wrap, it is cut. Navigation stays discoverable in the
/// help overlay, which has room to present a surface in full.
pub fn interaction_choice_hint(bindings: &EffectiveBindings) -> Vec<HelpEntry> {
    [
        binding_for(
            bindings,
            BindingContext::InteractionChoice,
            Action::ResolveChoiceSelected,
        ),
        binding_for(
            bindings,
            BindingContext::InteractionChoice,
            Action::ResolveChoiceCancelled,
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The bindings the interaction write pane's hint advertises.
pub fn interaction_write_hint(bindings: &EffectiveBindings) -> Vec<HelpEntry> {
    [
        typing_binding(bindings, BindingContext::InteractionWrite),
        binding_for(
            bindings,
            BindingContext::InteractionWrite,
            Action::DispatchRaww,
        ),
        binding_for(
            bindings,
            BindingContext::InteractionWrite,
            Action::InsertRawwNewline,
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// One row of the table in force in a context, as presentation reads it.
///
/// A compiled row already carries all three fields. A configured row carries
/// only the first two, and takes its heading from the context's compiled rows
/// for the same behavior -- which exist, because a configuration may bind only
/// a behavior the context already declares.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PresentedRow {
    chord: Chord,
    action: BoundAction,
    section: DisplaySection,
}

/// The rows one context presents: what the operator configured, then whatever
/// of the compiled table their configuration left standing.
///
/// Configured rows lead, because that is the order a reader wants. The chords
/// reaching one behavior fold onto a single line and a one-line hint strip
/// prints the first of them, so a rebinding that trailed its compiled
/// predecessor would be catalogued correctly and still leave every strip
/// advertising the chord it replaced.
///
/// A compiled row drops out where a higher tier has claimed the keystroke that
/// row is written as, which for every chord shape is exactly the keystroke its
/// display spells. That is the whole test, and it is what makes a presented
/// chord one that still reaches the behavior printed beside it.
///
/// It is the whole test because a row no longer matches more than it spells.
/// This once had a second half: a row matching a key under any modifier kept
/// answering for `Shift+Up` and `Alt+Up` after a configuration took plain `Up`,
/// so the row was dropped while those residuals went on reaching it
/// unadvertised — silence about a residual being a smaller fault than a false
/// line. Exact matching removed the residuals, so there is nothing left to be
/// silent about, and the test above is sufficient rather than merely the best
/// available.
///
/// The bare character is the one shape denoting two keystrokes, and both sides
/// denote the same two, so claiming it claims all of what the row answered.
fn rows_of(bindings: &EffectiveBindings, context: BindingContext) -> Vec<PresentedRow> {
    let mut rows: Vec<PresentedRow> = bindings
        .rows_for(context)
        .into_iter()
        .filter_map(|row| {
            // An explicit unbinding reaches nothing, so there is nothing to
            // present. It is not silent: the compiled row it emptied is dropped
            // below, which is the whole of what the operator asked for.
            let action = row.action?;
            Some(PresentedRow {
                chord: row.chord,
                action: BoundAction::Fixed(action),
                section: default_section(context, action)?,
            })
        })
        .collect();
    rows.extend(
        default_rows(context)
            .filter(|row| !superseded(bindings, context, row))
            .map(|row| PresentedRow {
                chord: row.chord,
                action: row.action,
                section: row.section,
            }),
    );
    rows
}

/// Whether a higher tier has taken the keystroke a compiled row is written as.
fn superseded(bindings: &EffectiveBindings, context: BindingContext, row: &ContextRow) -> bool {
    row.chord
        .denoted_keystroke()
        .is_some_and(|(code, modifiers)| bindings.is_configured(context, code, modifiers))
}

fn one(
    bindings: &EffectiveBindings,
    context: BindingContext,
    wanted: BoundAction,
) -> Option<HelpEntry> {
    fold_rows(
        rows_of(bindings, context)
            .into_iter()
            .map(|row| (context, row)),
    )
    .into_iter()
    .find(|(action, _)| *action == wanted)
    .map(entry_of)
}

fn entries_for(bindings: &EffectiveBindings, section: DisplaySection) -> Vec<HelpEntry> {
    finish(fold_rows(help_contexts().into_iter().flat_map(
        move |context| {
            rows_of(bindings, context)
                .into_iter()
                .filter(move |row| row.section == section)
                .map(move |row| (context, row))
        },
    )))
}

/// Groups rows by the behavior they reach, in the order the rows are first
/// reached, recording every row and marking which of them are printed.
///
/// A `Vec` rather than a map: the order rows are declared in is the order they
/// are presented in, and a hash map would discard it.
fn fold_rows(
    rows: impl Iterator<Item = (BindingContext, PresentedRow)>,
) -> Vec<(BoundAction, Vec<HelpSource>)> {
    let mut entries: Vec<(BoundAction, Vec<HelpSource>)> = Vec::new();
    for (context, row) in rows {
        let chord = row.chord.display();
        match entries.iter_mut().find(|(action, _)| *action == row.action) {
            Some((_, sources)) => {
                // Every row is recorded; only some are printed. A chord whose
                // text is already on the line is folded -- which is what
                // absorbs the same chord declared again by a second context.
                // It once also absorbed a modifier-agnostic `Enter` fallback
                // row, which rendered as plain "Enter"; no such row survives
                // exact matching, so the modified `Enter` forms are the only
                // remaining fold and they go through the test below.
                let shown = !already_shown(&chord, sources)
                    && !is_redundant_modified_enter(&chord, sources);
                sources.push(HelpSource {
                    context,
                    chord,
                    shown,
                    pattern: row.chord,
                });
            }
            None => entries.push((
                row.action,
                vec![HelpSource {
                    context,
                    chord,
                    shown: true,
                    pattern: row.chord,
                }],
            )),
        }
    }
    entries
}

fn entry_of((action, sources): (BoundAction, Vec<HelpSource>)) -> HelpEntry {
    HelpEntry {
        chords: sources
            .iter()
            .filter(|source| source.shown)
            .map(|source| source.chord.as_str())
            .collect::<Vec<_>>()
            .join(" / "),
        description: action.describe(),
        sources,
    }
}

fn finish(folded: Vec<(BoundAction, Vec<HelpSource>)>) -> Vec<HelpEntry> {
    folded.into_iter().map(entry_of).collect()
}

fn already_shown(chord: &str, sources: &[HelpSource]) -> bool {
    sources
        .iter()
        .any(|source| source.shown && source.chord == chord)
}

/// Whether a chord is a modified `Enter` on a line that already shows the bare
/// one.
///
/// Capability-neutral defaults mean a context's `Shift+Enter` and `Ctrl+Enter`
/// always reach the action it binds to `Enter`, so once `Enter` is on the line
/// the modified forms add no information -- only length, and they are the
/// longest chords in the table. Presenting all three on every affected line
/// tripled its width against the hand-written overlay this replaces. The fact
/// is stated once, as a standing note beside the generated bindings, rather
/// than eight times inside them.
///
/// The rule survives a configuration without being conditioned on one, because
/// what it folds into is the line rather than the defaults: an entry holds one
/// behavior, so a shown `Enter` on it is an `Enter` that reaches the same
/// behavior as the chord being folded. Where a configuration moves `Enter` to
/// something else, the modified forms it left alone form an entry of their own
/// with no bare `Enter` on it, and print in full.
///
/// This is presentation only. Both chords remain declared rows and both
/// resolve through the effective table, and the folded row is still recorded in
/// [`HelpEntry::sources`] with `shown` false.
fn is_redundant_modified_enter(chord: &str, sources: &[HelpSource]) -> bool {
    matches!(chord, "Shift+Enter" | "Ctrl+Enter") && already_shown("Enter", sources)
}
