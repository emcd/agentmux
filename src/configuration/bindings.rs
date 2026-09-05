//! Validation of the operator binding group in `ui.toml`.
//!
//! Separated from `loaders.rs`, which orchestrates reading each configuration
//! artifact; this answers the narrower question of whether one binding group
//! says anything the TUI can act on. Resolving names against the vocabulary
//! is why the configuration module reads `crate::tui`; see the README.

use std::path::Path;

use super::ConfigurationError;
use super::raw::{RawBindings, RawUiFile};
use crossterm::event::{KeyCode, KeyModifiers};

use crate::tui::{
    Action, BindingConfiguration, BindingContext, CapabilityClass, ChordPattern, ConfiguredAction,
    ConfiguredBinding, PrimaryModifier, context_actions, parse_chord, quit_unreachable,
};

/// One binding set this build ships: the name an operator writes under
/// `presets`, and the configuration text built into the binary that supplies
/// its rows.
///
/// The rows are not written in code. The text is the format an operator writes
/// and is read by the parser that reads their file, which makes each shipped
/// set a conformance test of the grammar — if the grammar cannot say what we
/// want to ship, our own checks report it — and a worked example that cannot
/// drift from what the parser accepts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShippedPreset {
    pub name: &'static str,
    pub text: &'static str,
}

/// The binding sets this build ships, in no significant order; a configuration
/// decides the order its own presets apply in by the order it names them.
///
/// Both sets confine themselves to terminals that report modified keys
/// distinctly, and each says so the only way the format has of saying it: every
/// row states the `enhanced` class and no other, so the rows contribute nothing
/// where the probe reports the other class. That makes the restriction
/// structural — there is no arrangement of a configuration that applies these
/// rows where the keystrokes they move sending onto cannot arrive.
static SHIPPED_PRESETS: &[ShippedPreset] = &[
    ShippedPreset {
        name: "enter-newline-shift-enter-sends",
        text: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/bindings/enter-newline-shift-enter-sends.toml"
        )),
    },
    ShippedPreset {
        name: "enter-newline-primary-enter-sends",
        text: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/data/bindings/enter-newline-primary-enter-sends.toml"
        )),
    },
];

/// Every binding set this build ships.
#[must_use]
pub fn shipped_binding_presets() -> &'static [ShippedPreset] {
    SHIPPED_PRESETS
}

/// Reads one binding set from configuration text built into this binary.
///
/// `artifact` names what is being read, and appears in the fault rather than a
/// path, because there is no operator file to name.
///
/// Public, and taking the text rather than only a preset name, so the
/// repository's checks can exercise both halves of the contract: that every
/// shipped set parses, and that a set which does not is reported as a defect in
/// our artifact rather than in the operator's file. The second cannot be
/// checked through a name alone without shipping the malformed set it exists to
/// keep from shipping.
///
/// # Errors
///
/// Returns [`ConfigurationError::MalformedEmbeddedArtifact`] — never a fault
/// against an operator's file — when the text is not a binding group this
/// build's own parser accepts.
pub fn embedded_binding_preset(
    artifact: &str,
    text: &str,
) -> Result<Vec<ConfiguredBinding>, ConfigurationError> {
    let internal = |message: String| ConfigurationError::malformed_embedded(artifact, message);
    let parsed =
        toml::from_str::<RawUiFile>(text).map_err(|source| internal(source.to_string()))?;
    let raw = parsed
        .bindings
        .ok_or_else(|| internal("it declares no bindings group".to_owned()))?;
    if !raw.presets.is_empty() {
        return Err(internal(
            "a binding set may not name binding sets".to_owned(),
        ));
    }
    if raw.primary_modifier_on_macos.is_some() {
        return Err(internal(
            "a binding set may not select the macOS primary modifier, which is the operator's"
                .to_owned(),
        ));
    }
    // The group is validated by the same code an operator's file goes through.
    // Its faults name a path, which this has none of, so the message is lifted
    // out and re-reported against the artifact instead.
    match validate_binding_group(&raw, Path::new(artifact)) {
        Ok(group) => Ok(group.rows),
        Err(ConfigurationError::InvalidConfiguration { message, .. }) => Err(internal(message)),
        Err(other) => Err(internal(other.to_string())),
    }
}

/// The rows the named binding sets contribute, concatenated in the order named
/// so that a later set supersedes an earlier one binding the same chord.
fn resolve_presets(
    names: &[String],
    path: &Path,
) -> Result<Vec<ConfiguredBinding>, ConfigurationError> {
    let mut rows = Vec::new();
    for name in names {
        let preset = SHIPPED_PRESETS
            .iter()
            .find(|preset| preset.name == name)
            .ok_or_else(|| {
                ConfigurationError::invalid(path, format!("unknown binding preset: {name}"))
            })?;
        rows.extend(embedded_binding_preset(
            &format!("binding set {}", preset.name),
            preset.text,
        )?);
    }
    Ok(rows)
}

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

    let preset_rows = resolve_presets(&raw.presets, path)?;

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

    let configuration = BindingConfiguration {
        presets: raw.presets.clone(),
        preset_rows,
        primary_modifier_on_macos,
        rows,
    };

    // Refused rather than reported, and refused on either capability class
    // rather than on the running terminal's. An operator whose configuration
    // cannot quit has no way to fix it from inside the TUI, and the class their
    // terminal falls into is not knowable when they write the file — so the
    // answer has to be the same here, where a probe has happened, and at
    // pre-flight, where none has.
    if let Some(classes) = quit_unreachable(Some(&configuration), cfg!(target_os = "macos")) {
        return Err(invalid(format!(
            "no chord quits the TUI under {}",
            classes.describe()
        )));
    }

    Ok(configuration)
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
            // Keyed off the classes themselves rather than off written-out
            // names, so the columns a file may declare and the classes a report
            // may name stay the same vocabulary.
            let mut resolved = [None, None];
            for (slot, class) in CapabilityClass::ALL.into_iter().enumerate() {
                let class = class.name();
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
            if let Some(unknown) = columns.keys().find(|key| {
                !CapabilityClass::ALL
                    .iter()
                    .any(|class| class.name() == key.as_str())
            }) {
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
