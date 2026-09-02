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

use crossterm::event::{KeyCode, KeyModifiers};

use super::action::Action;
use super::bindings::{BINDINGS, BoundAction, Chord, ContextRow, DisplaySection};
use super::context::BindingContext;

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
        }
    }

    /// The sections in the order they are presented, which is the order they
    /// are declared in.
    const ALL: [DisplaySection; 4] = [
        DisplaySection::Modes,
        DisplaySection::Communication,
        DisplaySection::Interaction,
        DisplaySection::Picker,
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

/// The whole binding table as the help overlay presents it: sections in
/// declaration order, and within each, one entry per behavior in the order the
/// table first reaches it.
///
/// This reports the compiled defaults, the same as
/// [`super::default_binding`] does, and carries no capability conditioning --
/// nothing here reads the keyboard-enhancement probe outcome.
pub fn help_bindings() -> Vec<HelpSection> {
    DisplaySection::ALL
        .into_iter()
        .map(|section| HelpSection {
            heading: section.heading(),
            entries: entries_for(section),
        })
        .collect()
}

/// Every binding one context declares, as presented.
///
/// Where [`help_bindings`] answers for the whole surface, this answers for a
/// single context. That asymmetry is deliberate and is the difference between
/// the help overlay and a pane hint strip: help is a catalogue of everything,
/// a strip annotates the one surface it sits on.
pub fn context_bindings(context: BindingContext) -> Vec<HelpEntry> {
    finish(fold_rows(rows_of(context).map(|row| (context, row))))
}

/// The presented binding for one behavior in one context, or `None` where that
/// context does not bind it.
pub fn binding_for(context: BindingContext, action: Action) -> Option<HelpEntry> {
    one(context, BoundAction::Fixed(action))
}

/// The presented binding for typing an ordinary character into this context's
/// draft, or `None` where the context takes no typed text.
pub fn typing_binding(context: BindingContext) -> Option<HelpEntry> {
    fold_rows(rows_of(context).map(|row| (context, row)))
        .into_iter()
        .find(|(action, _)| matches!(action, BoundAction::Typed(_)))
        .map(entry_of)
}

/// The bindings the picker's hint strip advertises.
///
/// Which few behaviors are worth a one-line strip is an editorial judgment and
/// stays declared here; their chords and their wording are not, and come from
/// the table. Both picker columns contribute, because the strip annotates the
/// picker rather than whichever column currently holds focus -- and because
/// `Enter` means something different in each, which is the fact the strip
/// exists to convey.
pub fn picker_hint() -> Vec<HelpEntry> {
    [
        binding_for(BindingContext::PickerBundles, Action::TogglePickerFocus),
        binding_for(BindingContext::PickerBundles, Action::CommitPickerBundle),
        binding_for(BindingContext::PickerSessions, Action::CommitPickerSession),
        binding_for(BindingContext::PickerBundles, Action::ClosePicker),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The bindings the interaction write pane's hint advertises.
pub fn interaction_write_hint() -> Vec<HelpEntry> {
    [
        typing_binding(BindingContext::InteractionWrite),
        binding_for(BindingContext::InteractionWrite, Action::DispatchRaww),
        binding_for(BindingContext::InteractionWrite, Action::InsertRawwNewline),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn rows_of(context: BindingContext) -> impl Iterator<Item = &'static ContextRow> {
    BINDINGS
        .iter()
        .find(move |group| group.context == context)
        .map(|group| group.rows.iter())
        .unwrap_or_default()
}

fn one(context: BindingContext, wanted: BoundAction) -> Option<HelpEntry> {
    fold_rows(rows_of(context).map(|row| (context, row)))
        .into_iter()
        .find(|(action, _)| *action == wanted)
        .map(entry_of)
}

fn entries_for(section: DisplaySection) -> Vec<HelpEntry> {
    finish(fold_rows(help_contexts().into_iter().flat_map(
        move |context| {
            rows_of(context)
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
fn fold_rows<'a>(
    rows: impl Iterator<Item = (BindingContext, &'a ContextRow)>,
) -> Vec<(BoundAction, Vec<HelpSource>)> {
    let mut entries: Vec<(BoundAction, Vec<HelpSource>)> = Vec::new();
    for (context, row) in rows {
        let chord = row.chord.display();
        match entries.iter_mut().find(|(action, _)| *action == row.action) {
            Some((_, sources)) => {
                // Every row is recorded; only some are printed. A chord whose
                // text is already on the line is folded -- which is what
                // absorbs the modifier-agnostic `Enter` fallback, since it
                // renders as plain "Enter", and the same chord declared again
                // by a second context.
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
/// This is presentation only. Both chords remain declared rows and both
/// resolve through [`super::default_binding`], and the folded row is still
/// recorded in [`HelpEntry::sources`] with `shown` false. A context that bound
/// them differently would show them, because the bare `Enter` would then belong
/// to a different behavior and never share this line.
fn is_redundant_modified_enter(chord: &str, sources: &[HelpSource]) -> bool {
    matches!(chord, "Shift+Enter" | "Ctrl+Enter") && already_shown("Enter", sources)
}
