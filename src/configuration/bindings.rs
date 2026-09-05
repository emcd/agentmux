//! Validation of the operator binding group in `ui.toml`.
//!
//! Separated from `loaders.rs`, which orchestrates reading each configuration
//! artifact; this answers the narrower question of whether one binding group
//! says anything the TUI can act on. Resolving names against the vocabulary
//! is why the configuration module reads `crate::tui`; see the README.

use std::path::Path;

use super::ConfigurationError;
use super::raw::RawBindings;
use crossterm::event::{KeyCode, KeyModifiers};

use crate::tui::{
    Action, BindingConfiguration, BindingContext, ChordPattern, ConfiguredAction,
    ConfiguredBinding, PrimaryModifier, context_actions, parse_chord,
};

/// Names of the binding sets this build ships.
///
/// Deliberately empty: the sets themselves are later work, and until they exist
/// every preset an operator names is unknown and is refused. Accepting names
/// provisionally would be a check with nothing behind it, and would quietly
/// pass a configuration that does nothing.
const SHIPPED_PRESETS: &[&str] = &[];

/// Validates the `[bindings]` group, resolving every name against the TUI's
/// vocabulary and parsing every chord.
///
/// Nothing is applied partially. The whole group is rejected on the first
/// fault, because a configuration half in force is one an operator cannot
/// reason about: the bindings that survived would depend on where in the file
/// the mistake happened to sit.
pub(super) fn validate_binding_group(
    raw: &RawBindings,
    path: &Path,
) -> Result<BindingConfiguration, ConfigurationError> {
    let invalid = |message: String| ConfigurationError::invalid(path, message);

    for preset in &raw.presets {
        if !SHIPPED_PRESETS.contains(&preset.as_str()) {
            return Err(invalid(format!("unknown binding preset: {preset}")));
        }
    }

    let primary_modifier_on_macos = match raw.primary_modifier_on_macos.as_deref() {
        None => None,
        Some("control") => Some(PrimaryModifier::Control),
        Some("command") => Some(PrimaryModifier::Command),
        Some(other) => {
            return Err(invalid(format!(
                "primary-modifier-on-macos must be control or command, not {other}"
            )));
        }
    };

    let mut rows = Vec::new();
    for (context_name, chords) in &raw.contexts {
        let context = BindingContext::from_configuration_name(context_name)
            .ok_or_else(|| invalid(format!("unknown binding context: {context_name}")))?;
        // Two spellings can denote one keystroke -- "ctrl+j" and "control+j",
        // or a control chord written in either case -- and a symbolic chord can
        // land on a literal one once resolved. Left alone, which of them takes
        // effect would fall to the order the file's keys happen to sort in,
        // which is an accidental precedence rule rather than a declared one.
        let mut claimed: Vec<(ResolvedChord, &str)> = Vec::new();
        for (chord_text, value) in chords {
            let chord = parse_chord(chord_text)
                .map_err(|error| invalid(format!("in {context_name}: {error}")))?;
            for resolved in resolutions_of(chord) {
                if let Some((_, earlier)) = claimed.iter().find(|(seen, _)| *seen == resolved) {
                    return Err(invalid(format!(
                        "in {context_name}: {chord_text} and {earlier} denote the same chord"
                    )));
                }
                claimed.push((resolved, chord_text));
            }
            let (enhanced, standard) =
                capability_columns(value, chord_text, context, context_name, &invalid)?;
            if enhanced.is_none() && standard.is_none() {
                return Err(invalid(format!(
                    "in {context_name}: {chord_text} names no action for either terminal class"
                )));
            }
            rows.push(ConfiguredBinding {
                context,
                chord,
                enhanced,
                standard,
            });
        }
    }

    Ok(BindingConfiguration {
        presets: raw.presets.clone(),
        primary_modifier_on_macos,
        rows,
    })
}

/// One keystroke as a terminal reports it.
type ResolvedChord = (KeyCode, KeyModifiers);

/// Every keystroke a written chord could denote, across the platforms it may be
/// read on.
///
/// A literal chord denotes one. A chord using the symbolic modifier denotes two,
/// since that modifier resolves differently per platform and per operator
/// selection. Both are claimed, so a file is accepted or refused the same way
/// wherever it is read rather than colliding only on the machine that resolves
/// the symbol onto a literal chord the file also names.
fn resolutions_of(chord: ChordPattern) -> Vec<ResolvedChord> {
    let with_control = chord.resolve(KeyModifiers::CONTROL);
    let with_command = chord.resolve(KeyModifiers::SUPER);
    if with_control == with_command {
        vec![with_control]
    } else {
        vec![with_control, with_command]
    }
}

/// Reads what one chord entry maps to, per terminal capability class.
///
/// A bare string speaks for both classes; a table speaks for the classes it
/// names and leaves the other on its compiled default. Anything else, and any
/// key other than the two class names, is refused by name — which is why this
/// interprets the value rather than letting serde match an untagged enum that
/// could only report that nothing matched.
fn capability_columns(
    value: &toml::Value,
    chord_text: &str,
    context: BindingContext,
    context_name: &str,
    invalid: &impl Fn(String) -> ConfigurationError,
) -> Result<(Option<ConfiguredAction>, Option<ConfiguredAction>), ConfigurationError> {
    match value {
        toml::Value::String(name) => {
            let action = resolve_configured_action(name, context, context_name, invalid)?;
            Ok((Some(action), Some(action)))
        }
        toml::Value::Table(columns) => {
            let mut resolved = [None, None];
            for (class, slot) in [("enhanced", 0), ("standard", 1)] {
                if let Some(entry) = columns.get(class) {
                    let name = entry.as_str().ok_or_else(|| {
                        invalid(format!(
                            "in {context_name}: {chord_text} names a non-string action for {class}"
                        ))
                    })?;
                    resolved[slot] = Some(resolve_configured_action(
                        name,
                        context,
                        context_name,
                        invalid,
                    )?);
                }
            }
            if let Some(unknown) = columns
                .keys()
                .find(|key| key.as_str() != "enhanced" && key.as_str() != "standard")
            {
                return Err(invalid(format!(
                    "in {context_name}: {chord_text} names an unknown terminal class: {unknown}"
                )));
            }
            Ok((resolved[0], resolved[1]))
        }
        other => Err(invalid(format!(
            "in {context_name}: {chord_text} maps to {}, not an action name or a table of them",
            other.type_str()
        ))),
    }
}

/// Resolves one action name, refusing a behavior the context cannot perform.
///
/// The compiled table declares a behavior only where it has an effect, so the
/// actions a context declares are exactly the ones that do something there.
/// Binding any other would produce a row generated help advertises and that
/// does nothing when pressed, which is the defect the table's own rule exists
/// to prevent.
fn resolve_configured_action(
    name: &str,
    context: BindingContext,
    context_name: &str,
    invalid: &impl Fn(String) -> ConfigurationError,
) -> Result<ConfiguredAction, ConfigurationError> {
    if name == "none" {
        return Ok(ConfiguredAction::Unbound);
    }
    let action = Action::from_configuration_name(name)
        .ok_or_else(|| invalid(format!("unknown action: {name}")))?;
    if !context_actions(context).contains(&action) {
        return Err(invalid(format!(
            "in {context_name}: {name} has no effect there"
        )));
    }
    Ok(ConfiguredAction::Invoke(action))
}
