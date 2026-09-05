use agentmux::configuration::ConfigurationRoots;
use std::fs;

use tempfile::TempDir;

use agentmux::configuration::{ConfigurationError, UiConfiguration, load_ui_configuration};
use agentmux::tui::{Action, BindingContext, ConfiguredAction, PrimaryModifier, parse_chord};

#[test]
fn loads_default_bundle_from_ui_configuration() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");
    fs::write(
        root.join("ui.toml"),
        r#"
default-bundle = "agentmux"
"#,
    )
    .expect("write ui.toml");

    let loaded = load_ui_configuration(&ConfigurationRoots::single(&root))
        .expect("load ui configuration")
        .expect("existing config");
    assert_eq!(loaded.default_bundle.as_deref(), Some("agentmux"));
}

#[test]
fn ignores_missing_ui_configuration() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");

    let loaded = load_ui_configuration(&ConfigurationRoots::single(&root)).expect("load ui config");
    assert!(loaded.is_none(), "missing file should be ignored");
}

#[test]
fn empty_ui_configuration_resolves_no_default_bundle() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");
    fs::write(root.join("ui.toml"), "").expect("write ui.toml");

    let loaded = load_ui_configuration(&ConfigurationRoots::single(&root))
        .expect("load ui config")
        .expect("existing config");
    assert!(loaded.default_bundle.is_none());
}

#[test]
fn rejects_malformed_ui_configuration() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");
    fs::write(root.join("ui.toml"), "default-bundle = ").expect("write ui.toml");

    let error = load_ui_configuration(&ConfigurationRoots::single(&root))
        .expect_err("malformed ui.toml should fail");
    assert!(
        error.to_string().contains("ui.toml"),
        "error should name the offending file: {error}"
    );
}

/// Writes a `ui.toml` holding `body` and loads it.
fn load_bindings(body: &str) -> Result<Option<UiConfiguration>, ConfigurationError> {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().join("config");
    fs::create_dir_all(&root).expect("create config root");
    fs::write(root.join("ui.toml"), body).expect("write ui.toml");
    load_ui_configuration(&ConfigurationRoots::single(&root))
}

/// The shape the specification documents, character for character.
///
/// Its purpose is that the published shape cannot drift from what the loader
/// accepts: an edit to either that the other does not follow shows up here.
/// That only holds if this is the whole published text, including the preset
/// line, so it is reproduced complete even though no binding set ships yet.
const DOCUMENTED_SHAPE: &str = r#"[bindings]
# Binding sets applied before individually configured rows, in the order named.
presets = ["enter-newline-primary-enter-sends"]
# Which literal modifier the symbolic `primary` resolves to on macOS.
primary-modifier-on-macos = "control"

# One sub-table per binding context, keyed by the context's operator name.
[bindings.compose-message]
# A chord mapped to one action applies on both terminal capability classes.
"ctrl+w" = "send-message"
# A chord mapped to a class-qualified table applies only where stated; the
# omitted class keeps its compiled default.
"shift+enter" = { enhanced = "insert-message-newline" }
# An explicitly unbound chord is inert, and does not fall through.
"ctrl+j" = "none"

[bindings.picker-sessions]
"primary+enter" = "commit-picker-session"
"#;

/// The documented shape without its preset line, which is the part of it this
/// build can accept.
///
/// Derived from [`DOCUMENTED_SHAPE`] rather than written out again, so drift in
/// any other line reaches both tests rather than only the first.
fn documented_shape_without_presets() -> String {
    DOCUMENTED_SHAPE
        .lines()
        .filter(|line| !line.starts_with("presets =") && !line.starts_with("# Binding sets"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The published example names a binding set, and no binding set ships yet, so
/// the documented shape is currently refused — for that reason and no other.
///
/// This is what holds the whole published text under test while the registry is
/// empty. Asserting the sole reason means a drift anywhere else in the example
/// surfaces here as a different error rather than passing unnoticed, which
/// dropping the preset line from the fixture would have allowed.
///
/// Once the named set ships, this expectation becomes a successful load and the
/// two fixtures can become one.
#[test]
fn the_documented_shape_is_refused_only_for_its_unshipped_preset() {
    let error = load_bindings(DOCUMENTED_SHAPE).expect_err("no binding set ships yet");
    let rendered = error.to_string();
    assert!(
        rendered.contains("unknown binding preset: enter-newline-primary-enter-sends"),
        "the documented shape was refused for something other than its preset: {rendered}"
    );
}

/// Everything in the documented shape that this build supports, loaded and
/// asserted row by row.
#[test]
fn the_documented_configuration_shape_loads_as_written() {
    let loaded = load_bindings(&documented_shape_without_presets())
        .expect("the documented shape loads")
        .expect("existing config");
    let bindings = loaded.bindings.expect("a binding group");

    assert_eq!(
        bindings.primary_modifier_on_macos,
        Some(PrimaryModifier::Control)
    );
    assert!(bindings.presets.is_empty());
    assert_eq!(bindings.rows.len(), 4, "{:?}", bindings.rows);

    let row = |context, chord: &str| {
        bindings
            .rows
            .iter()
            .find(|row| row.context == context && row.chord == parse_chord(chord).expect("chord"))
            .unwrap_or_else(|| panic!("no row for {chord} in {context:?}"))
    };

    // A chord mapped to one action applies on both terminal capability classes.
    let both = row(BindingContext::ComposeMessage, "ctrl+w");
    assert_eq!(
        both.enhanced,
        Some(ConfiguredAction::Invoke(Action::SendMessage))
    );
    assert_eq!(both.standard, both.enhanced);

    // A class-qualified row speaks for one class and leaves the other alone.
    let qualified = row(BindingContext::ComposeMessage, "shift+enter");
    assert_eq!(
        qualified.enhanced,
        Some(ConfiguredAction::Invoke(Action::InsertMessageNewline))
    );
    assert_eq!(
        qualified.standard, None,
        "the omitted class should keep its compiled default"
    );

    // An explicit unbinding is a value, not an absence.
    let unbound = row(BindingContext::ComposeMessage, "ctrl+j");
    assert_eq!(unbound.enhanced, Some(ConfiguredAction::Unbound));
    assert_eq!(unbound.standard, Some(ConfiguredAction::Unbound));

    // The symbolic modifier survives loading unresolved.
    let symbolic = row(BindingContext::PickerSessions, "primary+enter");
    assert!(symbolic.chord.uses_primary_modifier());
    assert_eq!(
        symbolic.enhanced,
        Some(ConfiguredAction::Invoke(Action::CommitPickerSession))
    );
}

#[test]
fn a_ui_configuration_without_bindings_resolves_none() {
    let loaded = load_bindings("default-bundle = \"agentmux\"\n")
        .expect("load")
        .expect("existing config");
    assert!(
        loaded.bindings.is_none(),
        "an absent binding group is absence, not an empty configuration"
    );
}

#[test]
fn an_empty_binding_group_is_accepted_and_configures_nothing() {
    let loaded = load_bindings("[bindings]\n")
        .expect("load")
        .expect("existing config");
    let bindings = loaded.bindings.expect("a binding group");
    assert!(bindings.rows.is_empty());
    assert!(bindings.presets.is_empty());
    assert!(bindings.primary_modifier_on_macos.is_none());
}

/// Every way a binding group can be wrong, and the fragment of the message that
/// tells the operator which of them it was.
#[test]
fn an_invalid_binding_group_is_refused_with_its_reason() {
    for (body, expected) in [
        (
            "[bindings]\n[bindings.compose-message]\n\"ctrl+w\" = \"fly-to-the-moon\"\n",
            "unknown action",
        ),
        (
            "[bindings]\n[bindings.nowhere]\n\"ctrl+w\" = \"send-message\"\n",
            "unknown binding context",
        ),
        (
            "[bindings]\n[bindings.compose-message]\n\"Hyper+w\" = \"send-message\"\n",
            "unknown modifier",
        ),
        (
            "[bindings]\npresets = [\"enter-newline-primary-enter-sends\"]\n",
            "unknown binding preset",
        ),
        (
            "[bindings]\nprimary-modifier-on-macos = \"meta\"\n",
            "primary-modifier-on-macos must be control or command",
        ),
        (
            "[bindings]\n[bindings.not-a-context]\n\"ctrl+w\" = \"send-message\"\n",
            "unknown binding context",
        ),
        (
            "[bindings]\n[bindings.compose-message]\n\"ctrl+w\" = { enhanced = \"send-message\", unknown = \"x\" }\n",
            "unknown terminal class: unknown",
        ),
        (
            "[bindings]\n[bindings.compose-message]\n\"ctrl+w\" = 3\n",
            "not an action name or a table of them",
        ),
    ] {
        let error = load_bindings(body).expect_err(&format!("expected refusal for {body:?}"));
        let rendered = error.to_string();
        assert!(
            rendered.contains(expected),
            "{body:?} reported {rendered:?}, which does not mention {expected:?}"
        );
    }
}

/// A key under `[bindings]` that is neither an option nor a context is refused
/// whatever shape its value takes. A scalar is caught by the shape check before
/// the context vocabulary is consulted, so it is refused for a different stated
/// reason than a table is — but refused, which is what matters: a misspelled key
/// that loaded silently would leave an operator believing a configuration is in
/// force that does nothing.
#[test]
fn an_unrecognized_key_is_refused_whatever_its_shape() {
    assert!(
        load_bindings("[bindings]\nnot-a-context = 3\n").is_err(),
        "a scalar under [bindings] was accepted"
    );
    assert!(
        load_bindings("[bindings]\nnot-a-context = { a = \"b\" }\n").is_err(),
        "a table under an unknown key was accepted"
    );
}

/// Two spellings can denote one keystroke. Left accepted, which of them took
/// effect would fall to the order the file's keys happen to sort in, which is a
/// precedence rule nobody declared — so a configuration naming a chord twice is
/// refused rather than silently resolved.
#[test]
fn two_spellings_of_one_chord_in_a_context_are_refused() {
    for (body, expected) in [
        // Modifier aliases.
        (
            "[bindings.compose-message]\n\"ctrl+j\" = \"send-message\"\n\"control+j\" = \"toggle-mode\"\n",
            "denote the same chord",
        ),
        // A control chord written in either case: both fold to the character a
        // terminal reports.
        (
            "[bindings.compose-message]\n\"ctrl+j\" = \"send-message\"\n\"Ctrl+J\" = \"toggle-mode\"\n",
            "denote the same chord",
        ),
        // The symbolic modifier landing on a literal chord the file also names.
        // Refused everywhere rather than only where it resolves onto it, so one
        // file is read the same way on every machine.
        (
            "[bindings.picker-sessions]\n\"ctrl+enter\" = \"commit-picker-session\"\n\"primary+enter\" = \"toggle-picker-focus\"\n",
            "denote the same chord",
        ),
    ] {
        let error = load_bindings(body).expect_err(&format!("expected refusal for {body:?}"));
        let rendered = error.to_string();
        assert!(
            rendered.contains(expected),
            "{body:?} reported {rendered:?}, which does not mention {expected:?}"
        );
    }
}

/// The same chord in two different contexts is not a duplicate: contexts are
/// separate surfaces and a chord means what each of them says it means.
#[test]
fn one_chord_may_appear_in_two_contexts() {
    let loaded = load_bindings(
        "[bindings.compose-message]\n\"ctrl+w\" = \"send-message\"\n\n[bindings.compose-to]\n\"ctrl+w\" = \"clear-to-field\"\n",
    )
    .expect("distinct contexts are not a collision")
    .expect("existing config");
    assert_eq!(loaded.bindings.expect("a binding group").rows.len(), 2);
}

/// A configuration is applied whole or not at all. The rows before a mistake
/// must not survive it, or which bindings took effect would depend on where in
/// the file the operator's error happened to sit.
#[test]
fn an_invalid_group_applies_no_binding_from_that_configuration() {
    let body = r#"
[bindings.compose-message]
"ctrl+w" = "send-message"
"ctrl+g" = "fly-to-the-moon"
"#;
    let error = load_bindings(body).expect_err("the group is refused");
    assert!(error.to_string().contains("unknown action"));

    // The valid row above the mistake is not reachable through any successful
    // load, because there is no successful load.
    assert!(
        load_bindings(body).is_err(),
        "a configuration carrying a fault must not resolve"
    );
}

/// The compiled table declares a behavior only where it has an effect, so a
/// configuration may not bind one anywhere else: `clear-to-field` acts on the
/// `To` field and does nothing in the message field.
#[test]
fn a_behavior_the_context_does_not_declare_is_refused() {
    let error = load_bindings("[bindings.compose-message]\n\"ctrl+g\" = \"clear-to-field\"\n")
        .expect_err("an inert binding is refused");
    let rendered = error.to_string();
    assert!(
        rendered.contains("clear-to-field") && rendered.contains("no effect"),
        "{rendered:?}"
    );
}

/// The other half of that rule: a behavior the context does declare may be
/// given a chord it did not have, which is the main thing a configuration is
/// for.
#[test]
fn a_new_chord_for_a_declared_behavior_is_accepted() {
    let loaded = load_bindings("[bindings.compose-to]\n\"ctrl+g\" = \"clear-to-field\"\n")
        .expect("the binding is accepted")
        .expect("existing config");
    let rows = loaded.bindings.expect("a binding group").rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].context, BindingContext::ComposeTo);
    assert_eq!(
        rows[0].enhanced,
        Some(ConfiguredAction::Invoke(Action::ClearToField))
    );
}
