//! Which behaviors a configuration leaves reachable, per terminal capability
//! class.
//!
//! Separate from `help.rs`, which asks what to *present*. The questions differ
//! where a chord is claimed: presentation drops a compiled row a configuration
//! took over, while this asks whether anything at all still reaches the
//! behavior that row carried — possibly a chord in another row entirely.
//!
//! Both answers are needed because losing a binding does not require an
//! explicit unbinding. Binding a chord that already carried a behavior displaces
//! it, so a configuration can leave a behavior unreachable without ever naming
//! it.

use crossterm::event::{KeyCode, KeyModifiers};

use super::action::Action;
use super::bindings::default_rows;
use super::context::{BindingContext, lookup_order};
use super::effective::{BindingConfiguration, CapabilityClass, EffectiveBindings};
use super::help::context_actions;

/// Which capability classes a finding holds under.
///
/// A finding holding under both says so once rather than being reported twice:
/// an operator reading a pre-flight report is being told about one condition,
/// and splitting it in two reads as two problems.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AffectedClasses {
    Both,
    Only(CapabilityClass),
}

impl AffectedClasses {
    /// How these classes are named to an operator.
    #[must_use]
    pub fn describe(self) -> String {
        match self {
            Self::Both => "both terminal classes".to_owned(),
            Self::Only(class) => format!("the {} terminal class", class.name()),
        }
    }
}

/// A behavior a context's compiled rows declare that no chord reaches there.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnreachableAction {
    pub context: BindingContext,
    pub action: Action,
    pub classes: AffectedClasses,
}

impl UnreachableAction {
    /// This finding as an operator reads it.
    ///
    /// Named in the vocabulary they write — the action's and the context's
    /// configuration names — because the finding exists to be acted on, and the
    /// thing they act on is the file. The human description rides along, since
    /// a name alone does not say what was lost.
    #[must_use]
    pub fn describe(&self) -> String {
        let action = self.action.configuration_name().map_or_else(
            || self.action.describe().to_owned(),
            |name| format!("{name} ({})", self.action.describe()),
        );
        format!(
            "{action} is unreachable in {} under {}",
            self.context.configuration_name(),
            self.classes.describe()
        )
    }
}

/// Every behavior a configuration leaves unreachable in a context that declares
/// it, under either capability class.
///
/// The platform arrives as an argument rather than through `cfg!` because a
/// chord written with the symbolic modifier resolves differently on each, so
/// what a configuration displaces is platform-dependent — and both arms have to
/// stay reachable from a test.
///
/// Contexts are swept rather than named, and each context's behaviors are read
/// from its own compiled rows, so a context or a behavior added later is
/// inspected without this being revisited.
#[must_use]
pub fn unreachable_actions(
    configuration: Option<&BindingConfiguration>,
    on_macos: bool,
) -> Vec<UnreachableAction> {
    let tables = EffectiveBindings::for_each_class(configuration, on_macos);
    let mut findings = Vec::new();
    for context in BindingContext::ALL {
        let reachable: Vec<(CapabilityClass, Vec<Action>)> = tables
            .iter()
            .map(|(class, bindings)| (*class, reachable_actions(bindings, *class, context)))
            .collect();
        for action in context_actions(context) {
            let lost: Vec<CapabilityClass> = reachable
                .iter()
                .filter(|(_, reached)| !reached.contains(&action))
                .map(|(class, _)| *class)
                .collect();
            let classes = match lost.as_slice() {
                [] => continue,
                [only] => AffectedClasses::Only(*only),
                _ => AffectedClasses::Both,
            };
            findings.push(UnreachableAction {
                context,
                action,
                classes,
            });
        }
    }
    findings
}

/// The classes under which no chord quits the TUI, if any.
///
/// Read out of the same sweep that produces every other finding, rather than
/// computed alongside it, so the two cannot disagree about whether quit is
/// reachable — which would be the worst pair of answers to hold at once, since
/// one of them rejects a configuration and the other merely mentions it.
///
/// Quit is the only behavior whose loss is refused rather than reported,
/// because it is the only one an operator cannot recover from inside the
/// running application: every other binding can be restored by quitting and
/// editing the file.
#[must_use]
pub fn quit_unreachable(
    configuration: Option<&BindingConfiguration>,
    on_macos: bool,
) -> Option<AffectedClasses> {
    unreachable_actions(configuration, on_macos)
        .into_iter()
        .find(|finding| finding.action == Action::Quit)
        .map(|finding| finding.classes)
}

/// Every behavior some keystroke a terminal in this class can deliver reaches
/// while this context is the surface.
///
/// The surface rather than the context, because a keystroke arriving here is
/// resolved against the global rows first: what a behavior is reachable
/// *through* is a row of this context, but whether anything arrives at it
/// depends on the contexts consulted ahead of it too.
///
/// Computed once per context and class rather than per behavior asked about:
/// the candidate set does not depend on the behavior, and building it inside the
/// question would rebuild it for every row the context declares.
fn reachable_actions(
    bindings: &EffectiveBindings,
    class: CapabilityClass,
    context: BindingContext,
) -> Vec<Action> {
    let mut reached = Vec::new();
    for (code, modifiers) in candidate_keystrokes(bindings, class, context) {
        if let Some(action) = resolved_action(bindings, context, code, modifiers)
            && !reached.contains(&action)
        {
            reached.push(action);
        }
    }
    reached
}

/// The behavior a keystroke reaches on a surface, resolved as dispatch resolves
/// it: through the contexts that own the keystroke, in precedence order, taking
/// the first that answers.
///
/// Asking the surface alone would count a chord the global rows have taken as
/// still reaching the surface's row, so a configuration could displace a
/// behavior without this seeing it — and binding nothing in the context that
/// declares the behavior is enough to do it.
///
/// The order is read from [`lookup_order`] rather than restated here. A sweep
/// disagreeing with dispatch about precedence is the defect this function
/// exists to remove, and restating the order is how that disagreement gets
/// reintroduced.
fn resolved_action(
    bindings: &EffectiveBindings,
    context: BindingContext,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Option<Action> {
    lookup_order(context)
        .into_iter()
        .find_map(|context| bindings.action_for(context, code, modifiers))
}

/// Every keystroke worth asking about in this context, restricted to those the
/// class can deliver.
///
/// Derived from the rows themselves rather than enumerated: a behavior is
/// reachable only through a row that binds it, so the keystrokes those rows
/// match are the whole of the search. Enumerating a keystroke space instead
/// would have to be revised whenever the table gained a key, and would go
/// stale silently — the sweep would still pass, having stopped covering the
/// row that was added.
///
/// Gathered from every context resolution consults, not from this one alone.
/// An action can be declared on a surface and in the global rows both, and the
/// global chord reaches it while that surface is active — so a keystroke no row
/// of the surface matches can still be the thing that keeps its behavior
/// reachable. Drawing candidates from the surface alone would leave that
/// keystroke unasked about and report the behavior lost while it answers.
///
/// The same list [`resolved_action`] resolves against, so the two halves cannot
/// disagree about which contexts are in play.
fn candidate_keystrokes(
    bindings: &EffectiveBindings,
    class: CapabilityClass,
    context: BindingContext,
) -> Vec<(KeyCode, KeyModifiers)> {
    let mut keystrokes: Vec<(KeyCode, KeyModifiers)> = Vec::new();
    let mut push = |code, modifiers| {
        if class.delivers(code, modifiers) && !keystrokes.contains(&(code, modifiers)) {
            keystrokes.push((code, modifiers));
        }
    };
    for consulted in lookup_order(context) {
        for row in bindings.rows_for(consulted) {
            for (code, modifiers) in row.chord.denoted_keystrokes() {
                push(code, modifiers);
            }
        }
        for row in default_rows(consulted) {
            for (code, modifiers) in row.chord.denoted_keystrokes() {
                push(code, modifiers);
            }
        }
    }
    keystrokes
}
