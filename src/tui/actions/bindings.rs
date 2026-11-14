//! Default chord-to-action bindings, declared per binding context.
//!
//! The table this module owns is the single place a chord's action is
//! associated with the context that owns it, and the source dispatch, the help
//! overlay, the pane hint strips, and the operator usage guide all read.
//!
//! The table ships as embedded configuration, parsed by the operator's own
//! chord grammar and vocabulary rather than written as Rust. That makes the
//! defaults inspectable and copyable by operators for the first time, and
//! gives them the property every shipped binding set already has: they cannot
//! drift from what the parser accepts, because the parser is what reads them.
//!
//! The parse lives here rather than going through the configuration loader so
//! the dependency between the two modules stays one way: the configuration
//! module reads this vocabulary, and nothing here reads it. Only the chord
//! grammar and the two operator-facing vocabularies are shared, which is the
//! whole of what the defaults need from a parser. The shipped-set parse check
//! reads the same text through the full loader path, so a grammar change the
//! defaults cannot express fails there rather than reaching a release.
//!
//! Rows are grouped under their context rather than repeating it: the context
//! is the key a group is filed under, and within a group document order is the
//! order presentation reads them in. The global group holds the rows that
//! survive any open surface, and dispatch consults it first.
//!
//! Rows carry no capability field. Nothing varies by keyboard-enhancement probe
//! outcome: a modified `Enter` invokes whatever its context binds to bare
//! `Enter`, and invokes nothing in the two overlays, which bind none of the
//! three. A per-row flag would therefore be unused machinery. Terminal classes
//! are meant to diverge through a binding configuration, not here.
//!
//! A behavior is declared only in the contexts where it has an effect. Several
//! `AppState` methods guard on the focused field and do nothing elsewhere --
//! `insert_newline_if_message` outside `Message`, and
//! `autocomplete_active_recipient_field` outside `To` -- while the handlers
//! reach them from the whole screen mode. Declaring those inert rows would
//! preserve no behavior and would make generated help offer bindings that do
//! nothing, so they are omitted.

use std::fmt;
use std::marker::PhantomData;
use std::sync::OnceLock;

use crossterm::event::{KeyCode, KeyModifiers};
use serde::de::{MapAccess, Visitor};

use super::action::{Action, DisplaySection};
use super::chord::{Chord, parse_chord, typed_character};
use super::context::BindingContext;

/// The draft a context's typed characters land in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TextSink {
    Compose,
    Raww,
    PickerFilter,
}

impl TextSink {
    fn action(self, character: char) -> Action {
        match self {
            Self::Compose => Action::InsertComposeCharacter(character),
            Self::Raww => Action::InsertRawwCharacter(character),
            Self::PickerFilter => {
                Action::AppendPickerFilterCharacter(character)
            },
        }
    }

    /// What typing into this sink is said under in presentation. The sink
    /// travels with the context rather than with a row, so this is what
    /// files the context's typing entry once the typing rows are gone.
    pub(crate) const fn section(self) -> DisplaySection {
        match self {
            Self::Compose => DisplaySection::Communication,
            Self::Raww => DisplaySection::Interaction,
            Self::PickerFilter => DisplaySection::Picker,
        }
    }

    /// What typing into this sink does, said without naming a character. The
    /// per-character actions carry the operator's own keystroke, so they have
    /// no description of their own worth presenting.
    fn describe(self) -> &'static str {
        match self {
            Self::Compose => "Insert into focused field",
            Self::Raww => "Insert into write input",
            Self::PickerFilter => "Filter focused column",
        }
    }
}

/// What a row produces. Most rows name one action outright; the typing
/// variant names where the character goes and takes the character from the
/// key. No table row carries it — typing lives on the context, and
/// presentation synthesizes it to show the context's typing entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BoundAction {
    Fixed(Action),
    Typed(TextSink),
}

impl BoundAction {
    fn action_for(self, code: KeyCode) -> Option<Action> {
        match self {
            Self::Fixed(action) => Some(action),
            Self::Typed(sink) => match code {
                KeyCode::Char(character) => Some(sink.action(character)),
                _ => None,
            },
        }
    }

    /// What this row does, for presentation. This is also the key presentation
    /// groups rows by, which is why it is defined on `BoundAction` rather than
    /// on `Action`: the typing rows have no single action to describe.
    pub(crate) fn describe(self) -> &'static str {
        match self {
            Self::Fixed(action) => action.describe(),
            Self::Typed(sink) => sink.describe(),
        }
    }
}

/// One chord's binding within its context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ContextRow {
    pub chord: Chord,
    pub action: BoundAction,
}

/// Every row one context declares, in the order they are consulted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContextBindings {
    pub context: BindingContext,
    pub rows: Vec<ContextRow>,
}

/// The embedded default table, in the order its groups declare them.
fn parsed_base() -> &'static [ContextBindings] {
    static BASE: OnceLock<Vec<ContextBindings>> = OnceLock::new();
    BASE.get_or_init(|| parse_base_table(BASE_TEXT))
}

/// The default table as it ships, in the operator's own format.
const BASE_TEXT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/data/configuration/bindings/base.toml"
));

/// The table as written: one context's chord table, in document order.
///
/// A `Vec` of pairs rather than a map, so document order survives parsing:
/// presentation reads rows in declaration order, and `BTreeMap` would sort
/// them by chord text instead. The pairs are collected by a map visitor
/// rather than derived from a map type, because the TOML backend visits
/// entries in document order. The operator-facing loader sorts the other way
/// and is order-independent by construction (no two default rows share a
/// keystroke); what matters here is that both readers agree on the row set,
/// which the shipped-set parse check holds them to.
#[derive(Debug)]
struct BaseTableFile {
    bindings: Vec<(String, Vec<(String, String)>)>,
}

/// Collects one TOML table's entries in the order the backend visits them.
struct Pairs<T>(Vec<(String, T)>);

impl<'de, T> serde::Deserialize<'de> for Pairs<T>
where
    T: serde::Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct PairsVisitor<T>(PhantomData<T>);

        impl<'de, T> Visitor<'de> for PairsVisitor<T>
        where
            T: serde::Deserialize<'de>,
        {
            type Value = Pairs<T>;

            fn expecting(
                &self,
                formatter: &mut fmt::Formatter,
            ) -> fmt::Result {
                formatter.write_str("a TOML table")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut pairs = Vec::new();
                while let Some(entry) = map.next_entry::<String, T>()? {
                    pairs.push(entry);
                }
                Ok(Pairs(pairs))
            }
        }

        deserializer.deserialize_map(PairsVisitor(PhantomData))
    }
}

impl<'de> serde::Deserialize<'de> for BaseTableFile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawFile {
            bindings: Pairs<Pairs<String>>,
        }

        let raw = RawFile::deserialize(deserializer)?;
        Ok(BaseTableFile {
            bindings: raw
                .bindings
                .0
                .into_iter()
                .map(|(context, chords)| (context, chords.0))
                .collect(),
        })
    }
}

/// Reads the embedded default table through the operator's chord grammar and
/// vocabularies.
///
/// Only bare action names are accepted: the defaults speak for both terminal
/// classes, so a class-qualified table has nothing to say here, and the
/// symbolic modifier resolves per platform while the defaults must read the
/// same everywhere. Anything else is a defect in the shipped artifact, and
/// panics rather than silently misbinding: the shipped-set parse check runs
/// the same text through the full loader path on every commit, so a release
/// cannot reach this with text that fails.
fn parse_base_table(text: &str) -> Vec<ContextBindings> {
    let file: BaseTableFile = toml::from_str(text).unwrap_or_else(|error| {
        panic!("embedded base bindings do not parse: {error}")
    });
    file.bindings
        .into_iter()
        .map(|(context_name, chords)| {
            let context = BindingContext::from_configuration_name(&context_name)
                .unwrap_or_else(|| {
                    panic!("embedded base bindings name an unknown context: {context_name}")
                });
            let rows = chords
                .into_iter()
                .map(|(chord_text, action_name)| {
                    let pattern = parse_chord(&chord_text).unwrap_or_else(|error| {
                        panic!(
                            "embedded base bindings do not parse in {context_name}: {error}"
                        )
                    });
                    if pattern.uses_primary_modifier() {
                        panic!(
                            "embedded base bindings use the symbolic modifier in {context_name}: {chord_text}"
                        );
                    }
                    // Resolved with `Control` for the symbolic modifier the
                    // refusal above has already ruled out, so the choice is
                    // immaterial; what matters is that a bare character keeps
                    // its two-keystroke shape, exactly as the retired static
                    // row did.
                    let chord = pattern.resolve_to_chord(KeyModifiers::CONTROL);
                    let action = Action::from_configuration_name(&action_name)
                        .unwrap_or_else(|| {
                            panic!(
                                "embedded base bindings name an unknown action in {context_name}: {action_name}"
                            )
                        });
                    ContextRow {
                        chord,
                        action: BoundAction::Fixed(action),
                    }
                })
                .collect();
            ContextBindings { context, rows }
        })
        .collect()
}

/// Resolves a key against one context's default rows, taking the first row that
/// matches.
///
/// A character no row names falls through to the context's sink, where it has
/// one: the sink used to sit last in each group as a `Chord::Text` row, so
/// consulting it after every row is the same precedence without the one row
/// shape the chord grammar cannot spell. Contexts without a sink — the global
/// rows and the two overlays that take no text — answer nothing for it.
///
/// This reports the embedded **base**, and is the bottom tier of
/// [`super::EffectiveBindings`] rather than what dispatch or the help overlay
/// call directly: an operator's configuration resolves ahead of it, and answers
/// this question differently where it speaks. It remains the answer for a host
/// asking what a chord means before any configuration, and for the generated
/// usage guide, which has none to read.
pub fn default_binding(
    context: BindingContext,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Option<Action> {
    if let Some(action) = default_rows(context)
        .find(|row| row.chord.matches(code, modifiers))
        .and_then(|row| row.action.action_for(code))
    {
        return Some(action);
    }
    let character = typed_character(code, modifiers)?;
    context.text_sink().map(|sink| sink.action(character))
}

/// Every row one context declares, in the order they are consulted.
pub(crate) fn default_rows(
    context: BindingContext,
) -> impl Iterator<Item = &'static ContextRow> {
    parsed_base()
        .iter()
        .find(move |group| group.context == context)
        .map(|group| group.rows.iter())
        .unwrap_or_default()
}
