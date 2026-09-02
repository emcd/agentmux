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

use super::bindings::{BINDINGS, BoundAction, Chord, DisplaySection};
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

fn entries_for(section: DisplaySection) -> Vec<HelpEntry> {
    // Behaviors in first-reached order, each accumulating the rows that reach
    // it. A Vec rather than a map: the order rows are declared in is the order
    // they are presented in, and a hash map would discard it.
    let mut entries: Vec<(BoundAction, Vec<HelpSource>)> = Vec::new();

    for context in help_contexts() {
        let Some(group) = BINDINGS.iter().find(|group| group.context == context) else {
            continue;
        };
        for row in group.rows.iter().filter(|row| row.section == section) {
            let chord = row.chord.display();
            match entries.iter_mut().find(|(action, _)| *action == row.action) {
                Some((_, sources)) => {
                    // Every row is recorded; only some are printed. A chord
                    // whose text is already on the line is folded -- which is
                    // what absorbs the modifier-agnostic `Enter` fallback,
                    // since it renders as plain "Enter", and the same chord
                    // declared again by a second context.
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
    }

    entries
        .into_iter()
        .map(|(action, sources)| HelpEntry {
            chords: sources
                .iter()
                .filter(|source| source.shown)
                .map(|source| source.chord.as_str())
                .collect::<Vec<_>>()
                .join(" / "),
            description: action.describe(),
            sources,
        })
        .collect()
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
