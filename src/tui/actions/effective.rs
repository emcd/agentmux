//! What an operator configured, and the table their configuration produces.
//!
//! The configured types live here rather than with the file format that parses
//! them because they are described in this vocabulary: a configured row holds a
//! behavior and a chord, not the strings a file spelled them with. Keeping them
//! here also keeps the dependency between the two modules one way — the
//! configuration module reads this vocabulary, and nothing here reads the
//! configuration module.

use crossterm::event::{KeyCode, KeyModifiers};

use super::action::Action;
use super::bindings::default_binding;
use super::chord::{ChordPattern, PrimaryModifier, primary_modifier};
use super::context::BindingContext;

/// Which of a terminal's two classes a lookup is answering for.
///
/// Named rather than taken as a bare `bool` so a caller cannot silently pass
/// the wrong sense, and so the two classes read the same here as they do in an
/// operator's configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityClass {
    /// The terminal reports modified keys distinctly.
    Enhanced,
    /// It does not, so a modified chord arrives as its unmodified form.
    Standard,
}

impl CapabilityClass {
    /// The class a keyboard-enhancement probe outcome puts a terminal in.
    #[must_use]
    pub const fn of(disambiguates_modified_keys: bool) -> Self {
        if disambiguates_modified_keys {
            Self::Enhanced
        } else {
            Self::Standard
        }
    }
}

/// An operator's validated binding group.
///
/// Every name has already been resolved against this vocabulary and every chord
/// already parsed, so a consumer receives behaviors and chords rather than
/// strings to interpret.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BindingConfiguration {
    /// Named binding sets to apply, in the order given.
    pub presets: Vec<String>,
    /// The rows those named sets contribute, concatenated in the order the sets
    /// are named.
    ///
    /// Resolved as the group is validated, alongside every other name, rather
    /// than carried as names for a later reader to look up: a set naming a
    /// behavior this build does not have is a fault in the configuration, and
    /// it is reported where the operator's other faults are.
    pub preset_rows: Vec<ConfiguredBinding>,
    /// Which literal modifier the symbolic `primary` modifier resolves to on
    /// macOS. Absent leaves the default, which is `Ctrl`.
    pub primary_modifier_on_macos: Option<PrimaryModifier>,
    /// One entry per configured chord, in the order the file declares them.
    pub rows: Vec<ConfiguredBinding>,
}

/// One configured chord and what it invokes, per terminal capability class.
///
/// A class left `None` is one the configuration did not speak for, and keeps
/// whatever the compiled default says. That is distinct from a class bound to
/// [`ConfiguredAction::Unbound`], which the operator deliberately emptied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfiguredBinding {
    pub context: BindingContext,
    pub chord: ChordPattern,
    pub enhanced: Option<ConfiguredAction>,
    pub standard: Option<ConfiguredAction>,
}

impl ConfiguredBinding {
    /// What this row says for one capability class, if it speaks for it.
    #[must_use]
    pub const fn for_class(&self, class: CapabilityClass) -> Option<ConfiguredAction> {
        match class {
            CapabilityClass::Enhanced => self.enhanced,
            CapabilityClass::Standard => self.standard,
        }
    }
}

/// What a configured chord invokes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfiguredAction {
    /// The named behavior.
    Invoke(Action),
    /// Nothing: the chord is inert here, and does not fall through to the
    /// compiled default.
    Unbound,
}

/// One resolved row: a chord as a terminal will report it, and what it reaches.
///
/// Crate-visible because presentation reads these rows as well as resolution
/// does: the help overlay and the pane hint strips show what an operator
/// configured, and the only place that is written down is here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedRow {
    pub context: BindingContext,
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
    /// `None` is an explicit unbinding, which answers "nothing" rather than
    /// deferring to a lower tier.
    pub action: Option<Action>,
}

impl ResolvedRow {
    fn matches(self, context: BindingContext, code: KeyCode, modifiers: KeyModifiers) -> bool {
        self.context == context && self.code == code && self.modifiers == modifiers
    }
}

/// The bindings in force: what an operator configured, over what ships.
///
/// Built once, from the compiled defaults, any named binding sets, and the
/// operator's own rows, with the symbolic modifier already resolved for the
/// running platform and the capability class already chosen. A lookup therefore
/// needs neither the configuration nor the probe outcome, only a chord.
#[derive(Clone, Debug, Default)]
pub struct EffectiveBindings {
    configured: Vec<ResolvedRow>,
    /// Rows contributed by the named binding sets, in the order the
    /// configuration named them.
    preset: Vec<ResolvedRow>,
}

impl EffectiveBindings {
    /// Builds the table for one capability class on one platform.
    ///
    /// The class and the platform arrive as arguments rather than being probed
    /// here, so a caller can build the table for either class and either
    /// platform without a terminal.
    ///
    /// Both tiers above the compiled defaults come from the one configuration:
    /// the rows it declares itself, and the rows the sets it names contribute,
    /// which were resolved when it was validated.
    #[must_use]
    pub fn build(
        configuration: Option<&BindingConfiguration>,
        class: CapabilityClass,
        on_macos: bool,
    ) -> Self {
        let primary = primary_modifier(
            on_macos,
            configuration.and_then(|configuration| configuration.primary_modifier_on_macos),
        );
        let resolve = |rows: &[ConfiguredBinding]| {
            rows.iter()
                .filter_map(|row| {
                    let (code, modifiers) = row.chord.resolve(primary);
                    row.for_class(class).map(|action| ResolvedRow {
                        context: row.context,
                        code,
                        modifiers,
                        action: match action {
                            ConfiguredAction::Invoke(action) => Some(action),
                            ConfiguredAction::Unbound => None,
                        },
                    })
                })
                .collect()
        };
        Self {
            configured: configuration
                .map_or_else(Vec::new, |configuration| resolve(&configuration.rows)),
            preset: configuration.map_or_else(Vec::new, |configuration| {
                resolve(&configuration.preset_rows)
            }),
        }
    }

    /// The behavior a chord reaches in one context, or `None` where it reaches
    /// none.
    ///
    /// Tiers are consulted in declared order — configured, then any binding set,
    /// then the compiled defaults — and the first tier holding the chord
    /// answers. An explicit unbinding therefore answers "nothing" rather than
    /// letting a lower tier reply, which is what separates emptying a chord
    /// from never having spoken about it.
    ///
    /// Within a tier the last matching row answers, because binding sets are
    /// applied in the order they are named and a later one supersedes an
    /// earlier one binding the same chord. Selecting the last row here is what
    /// keeps that rule in the table rather than obliging whoever assembles the
    /// sets to reverse them before handing them over, which would put
    /// precedence in the wrong layer.
    ///
    /// The operator's own rows cannot contain the same chord twice — a
    /// configuration declaring one is refused — so the rule is invisible there
    /// and applies uniformly rather than as a special case for one tier.
    ///
    /// This answers for one context. Which contexts are consulted, and in what
    /// order, is the caller's: a global row still outranks a contextual one
    /// because the caller asks the global context first.
    #[must_use]
    pub fn action_for(
        &self,
        context: BindingContext,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Option<Action> {
        for tier in [&self.configured, &self.preset] {
            if let Some(row) = tier
                .iter()
                .rev()
                .find(|row| row.matches(context, code, modifiers))
            {
                return row.action;
            }
        }
        default_binding(context, code, modifiers)
    }

    /// The rows above the compiled defaults that a lookup in this context could
    /// actually reach, in the order presentation reads them: the operator's own
    /// first, then any binding set's.
    ///
    /// Only the rows that *win*. A row a lookup can never return is a row
    /// presentation must not advertise, or the overlay names a chord under a
    /// behavior it does not reach — which is the same defect as advertising a
    /// compiled row a configuration took over, arriving from the other
    /// direction. Two rows can lose that way: one superseded later in its own
    /// tier, since binding sets are applied in the order they are named, and a
    /// binding set's row whose chord the operator bound themselves.
    ///
    /// The operator's own rows cannot contain the same chord twice — a
    /// configuration declaring one is refused — so within that tier the filter
    /// removes nothing today. It is not written for that tier: `build` is
    /// public and takes binding-set rows that never passed the loader, and
    /// filtering the tiers uniformly is what keeps the rule a property of the
    /// projection rather than of which caller supplied the rows.
    ///
    /// Presentation's order is not resolution's. A lookup takes the last
    /// matching row in a tier; a reader wants the chord they wrote themselves
    /// at the head of the line, since that is the one a one-line hint strip has
    /// room for. The orders differ, but the *set* does not, and that is the
    /// half this method exists to keep true.
    ///
    /// Explicit unbindings are included. What they reach is `None`, and it is
    /// presentation's business rather than this method's that a chord reaching
    /// nothing is shown as nothing.
    pub(crate) fn rows_for(&self, context: BindingContext) -> Vec<ResolvedRow> {
        let mut rows = winning_rows(&self.configured, context, &[]);
        rows.extend(winning_rows(&self.preset, context, &self.configured));
        rows
    }

    /// Whether any tier above the compiled defaults speaks for this chord.
    ///
    /// Distinguishes a chord answering `None` because an operator emptied it
    /// from one answering `None` because nothing binds it.
    #[must_use]
    pub fn is_configured(
        &self,
        context: BindingContext,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> bool {
        [&self.configured, &self.preset]
            .into_iter()
            .any(|tier| tier.iter().any(|row| row.matches(context, code, modifiers)))
    }
}

/// The rows of one tier that a lookup in this context could return.
///
/// A row loses to a later row in its own tier binding the same chord, since
/// that is the row [`EffectiveBindings::action_for`] selects, and to any row in
/// `above`, which is the tier consulted first. Written as one rule over both
/// tiers so the projection cannot disagree with resolution about one of them.
fn winning_rows(
    tier: &[ResolvedRow],
    context: BindingContext,
    above: &[ResolvedRow],
) -> Vec<ResolvedRow> {
    tier.iter()
        .enumerate()
        .filter(|(_, row)| row.context == context)
        .filter(|(index, row)| {
            let claims = |other: &&ResolvedRow| other.matches(context, row.code, row.modifiers);
            !tier[index + 1..].iter().any(|later| claims(&later))
                && !above.iter().any(|higher| claims(&higher))
        })
        .map(|(_, row)| *row)
        .collect()
}
