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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ResolvedRow {
    context: BindingContext,
    code: KeyCode,
    modifiers: KeyModifiers,
    /// `None` is an explicit unbinding, which answers "nothing" rather than
    /// deferring to a lower tier.
    action: Option<Action>,
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
    /// Rows contributed by named binding sets. No set ships yet, so this is
    /// empty — but the tier exists and is consulted, so shipping one populates
    /// this rather than changing how a lookup resolves.
    preset: Vec<ResolvedRow>,
}

impl EffectiveBindings {
    /// Builds the table for one capability class on one platform.
    ///
    /// The class and the platform arrive as arguments rather than being probed
    /// here, so a caller can build the table for either class and either
    /// platform without a terminal.
    ///
    /// `preset_rows` are the rows the named binding sets contribute, already
    /// selected. No set ships yet, so every caller passes an empty slice today
    /// — but the tier is a parameter rather than a hole, so shipping one fills
    /// it without touching how a lookup resolves.
    #[must_use]
    pub fn build(
        configuration: Option<&BindingConfiguration>,
        preset_rows: &[ConfiguredBinding],
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
            preset: resolve(preset_rows),
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
