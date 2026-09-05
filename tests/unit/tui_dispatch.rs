//! Equivalence between the two paths into a workbench behavior.
//!
//! A key reaches a behavior by dispatch: `Workbench::dispatch_event` resolves
//! the chord against the binding table and applies what it names. A host with
//! its own event loop reaches the same behavior by naming the action outright,
//! through `Workbench::apply_action`. Nothing keeps those two honest except a
//! test that drives both and compares what they leave behind, which is what
//! this module is.
//!
//! It is also where the table's exclusivity is enforced. Dispatch resolving a
//! chord anywhere but the table -- an arm answered before lookup, a condition
//! the table never absorbed -- shows up here as divergence from the action the
//! table names for that chord.

use std::path::PathBuf;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use agentmux::runtime::error::RuntimeError;
use agentmux::tui::{
    Action, BindingConfiguration, BindingContext, ConfiguredAction, ConfiguredBinding,
    TuiLaunchOptions, default_binding, parse_chord,
    workbench::{Workbench, WorkbenchField, WorkbenchMode, WorkbenchPickerColumn},
};

/// Builds a workbench from public launch options alone. The relay socket is
/// never served: the actions that reach it fail identically on both paths,
/// which is itself part of what this module asserts.
fn dispatch_workbench() -> Workbench {
    Workbench::new(TuiLaunchOptions {
        namespace: "agentmux".to_string(),
        sender_session: "tui".to_string(),
        relay_socket: PathBuf::from("/tmp/agentmux-dispatch-test-relay.sock"),
        look_lines: None,
        available_bundles: vec!["agentmux".to_string()],
        bindings: None,
    })
}

/// Everything a caller can observe about a workbench through its public read
/// surface. The projection is deliberately wide: a field left out of it is a
/// way the two paths to an action could diverge without the sweep noticing.
#[derive(Debug, Eq, PartialEq)]
struct Observed {
    focus: WorkbenchField,
    mode: WorkbenchMode,
    binding_context: BindingContext,
    to_field: String,
    to_cursor_column: usize,
    message_field: String,
    message_cursor: (usize, usize),
    raww_draft: String,
    interaction_shows_raww: bool,
    interaction_snapshot_scroll: usize,
    interaction_target: Option<String>,
    picker_open: bool,
    picker_column: WorkbenchPickerColumn,
    picker_filter: String,
    picker_selected_index: Option<usize>,
    bundle_picker_selected_index: Option<usize>,
    events_overlay_open: bool,
    help_overlay_open: bool,
    should_quit: bool,
    chat_history_scroll: usize,
    chat_history_bodies: Vec<String>,
    event_history_entries: Vec<String>,
    recipients: Vec<String>,
    last_selected_recipient: Option<String>,
    pending_choice_request_ids: Vec<String>,
}

fn observe(workbench: &Workbench) -> Observed {
    Observed {
        focus: workbench.focus(),
        mode: workbench.mode(),
        binding_context: workbench.binding_context(),
        to_field: workbench.to_field().to_string(),
        to_cursor_column: workbench.to_cursor_column(),
        message_field: workbench.message_field().to_string(),
        message_cursor: workbench.message_cursor_line_and_column(),
        raww_draft: workbench.raww_draft().to_string(),
        interaction_shows_raww: workbench.interaction_shows_raww(),
        interaction_snapshot_scroll: workbench.interaction_snapshot_scroll(),
        interaction_target: workbench.interaction_target().map(str::to_string),
        picker_open: workbench.picker_open(),
        picker_column: workbench.picker_column(),
        picker_filter: workbench.picker_filter().to_string(),
        picker_selected_index: workbench.picker_selected_index(),
        bundle_picker_selected_index: workbench.bundle_picker_selected_index(),
        events_overlay_open: workbench.events_overlay_open(),
        help_overlay_open: workbench.help_overlay_open(),
        should_quit: workbench.should_quit(),
        chat_history_scroll: workbench.chat_history_scroll(),
        chat_history_bodies: workbench
            .chat_history_bodies()
            .into_iter()
            .map(str::to_string)
            .collect(),
        event_history_entries: workbench
            .event_history_entries()
            .into_iter()
            .map(str::to_string)
            .collect(),
        recipients: workbench
            .recipients()
            .into_iter()
            .map(str::to_string)
            .collect(),
        last_selected_recipient: workbench.last_selected_recipient().map(str::to_string),
        pending_choice_request_ids: workbench
            .pending_choice_request_ids()
            .into_iter()
            .map(str::to_string)
            .collect(),
    }
}

/// Reduces a result to something comparable across two runs. The paths must
/// agree on failure as well as on state, or an action that reaches the relay
/// could differ in what it reports while leaving state alike.
fn outcome(result: Result<(), RuntimeError>) -> Result<(), String> {
    result.map_err(|error| error.to_string())
}

/// One dispatch-versus-apply case: the surface to put the workbench on, and the
/// chord to reach it with.
struct EquivalenceCase {
    surface: &'static str,
    arrange: fn(&mut Workbench),
    code: KeyCode,
    modifiers: KeyModifiers,
}

fn compose_to(workbench: &mut Workbench) {
    workbench.set_recipients(&["master", "worker"]);
    workbench.insert_text("mas");
}

fn compose_message(workbench: &mut Workbench) {
    workbench.set_recipients(&["master"]);
    workbench.set_focus(WorkbenchField::Message);
    workbench.insert_text("hello");
}

fn interaction_write(workbench: &mut Workbench) {
    workbench.set_recipients(&["master"]);
    workbench.set_interaction_target("master");
    // Entering Interaction re-captures the look snapshot, which fails against
    // the socket path these tests never serve. The mode still switches, which
    // is all the arrangement needs.
    let _ = workbench.apply_action(Action::ToggleMode);
}

fn interaction_write_with_draft(workbench: &mut Workbench) {
    interaction_write(workbench);
    let _ = workbench.apply_action(Action::InsertRawwCharacter('a'));
}

fn interaction_choice(workbench: &mut Workbench) {
    interaction_write(workbench);
    workbench.inject_pending_choice("master");
}

fn picker_sessions(workbench: &mut Workbench) {
    workbench.set_recipients(&["master", "worker"]);
    let _ = workbench.apply_action(Action::OpenPicker);
}

fn picker_bundles(workbench: &mut Workbench) {
    workbench.set_recipients(&["master", "worker"]);
    let _ = workbench.apply_action(Action::OpenBundlePicker);
}

fn events_overlay(workbench: &mut Workbench) {
    let _ = workbench.apply_action(Action::ToggleEventsOverlay);
}

fn help_overlay(workbench: &mut Workbench) {
    let _ = workbench.apply_action(Action::ToggleHelpOverlay);
}

fn equivalence_cases() -> Vec<EquivalenceCase> {
    let mut cases = Vec::new();
    let mut case = |surface, arrange: fn(&mut Workbench), code, modifiers| {
        cases.push(EquivalenceCase {
            surface,
            arrange,
            code,
            modifiers,
        });
    };

    // Every surface, reached by a chord its own rows own.
    case("compose to", compose_to, KeyCode::Tab, KeyModifiers::NONE);
    case(
        "compose to",
        compose_to,
        KeyCode::Char('u'),
        KeyModifiers::CONTROL,
    );
    case("compose to", compose_to, KeyCode::Enter, KeyModifiers::NONE);
    case(
        "compose to",
        compose_to,
        KeyCode::Char('x'),
        KeyModifiers::NONE,
    );
    case(
        "compose message",
        compose_message,
        KeyCode::Enter,
        KeyModifiers::NONE,
    );
    case(
        "compose message",
        compose_message,
        KeyCode::Enter,
        KeyModifiers::SHIFT,
    );
    case(
        "compose message",
        compose_message,
        KeyCode::Char('j'),
        KeyModifiers::CONTROL,
    );
    case(
        "compose message",
        compose_message,
        KeyCode::Char('z'),
        KeyModifiers::SHIFT,
    );
    case(
        "compose message",
        compose_message,
        KeyCode::Esc,
        KeyModifiers::NONE,
    );
    case(
        "interaction write",
        interaction_write,
        KeyCode::Up,
        KeyModifiers::NONE,
    );
    case(
        "interaction write with draft",
        interaction_write_with_draft,
        KeyCode::Up,
        KeyModifiers::NONE,
    );
    case(
        "interaction write",
        interaction_write,
        KeyCode::Enter,
        KeyModifiers::NONE,
    );
    case(
        "interaction write",
        interaction_write,
        KeyCode::Char('j'),
        KeyModifiers::CONTROL,
    );
    case(
        "interaction choice",
        interaction_choice,
        KeyCode::Char('c'),
        KeyModifiers::NONE,
    );
    case(
        "interaction choice",
        interaction_choice,
        KeyCode::Left,
        KeyModifiers::NONE,
    );
    case(
        "interaction choice",
        interaction_choice,
        KeyCode::Down,
        KeyModifiers::NONE,
    );
    case(
        "picker sessions",
        picker_sessions,
        KeyCode::Down,
        KeyModifiers::NONE,
    );
    case(
        "picker sessions",
        picker_sessions,
        KeyCode::Enter,
        KeyModifiers::NONE,
    );
    case(
        "picker sessions",
        picker_sessions,
        KeyCode::Char('m'),
        KeyModifiers::NONE,
    );
    case(
        "picker bundles",
        picker_bundles,
        KeyCode::Tab,
        KeyModifiers::NONE,
    );
    case(
        "picker bundles",
        picker_bundles,
        KeyCode::Esc,
        KeyModifiers::NONE,
    );
    case(
        "events overlay",
        events_overlay,
        KeyCode::F(4),
        KeyModifiers::NONE,
    );
    case(
        "events overlay",
        events_overlay,
        KeyCode::F(2),
        KeyModifiers::NONE,
    );
    case(
        "help overlay",
        help_overlay,
        KeyCode::F(3),
        KeyModifiers::NONE,
    );
    case(
        "help overlay",
        help_overlay,
        KeyCode::Esc,
        KeyModifiers::NONE,
    );

    // The global chords, under each surface that could have shadowed them.
    for (surface, arrange) in [
        ("compose message", compose_message as fn(&mut Workbench)),
        ("interaction choice", interaction_choice),
        ("picker sessions", picker_sessions),
        ("events overlay", events_overlay),
        ("help overlay", help_overlay),
    ] {
        case(surface, arrange, KeyCode::Char('c'), KeyModifiers::CONTROL);
        case(surface, arrange, KeyCode::F(1), KeyModifiers::NONE);
    }

    // Modifier sets the handlers treated loosely, and chords nothing binds.
    case(
        "interaction write",
        interaction_write,
        KeyCode::Enter,
        KeyModifiers::ALT,
    );
    case(
        "compose message",
        compose_message,
        KeyCode::Enter,
        KeyModifiers::ALT,
    );
    case(
        "interaction write",
        interaction_write,
        KeyCode::Char('j'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    case(
        "compose message",
        compose_message,
        KeyCode::Char('q'),
        KeyModifiers::CONTROL,
    );
    case(
        "picker sessions",
        picker_sessions,
        KeyCode::Char('r'),
        KeyModifiers::CONTROL,
    );

    cases
}

#[test]
fn dispatching_a_chord_and_applying_its_bound_action_leave_the_same_state() {
    // The two paths into a behavior must not drift apart. One workbench is
    // driven by the terminal event; the other resolves the same chord through
    // the public table and applies the action it names. Anything dispatch does
    // that the table does not account for -- a chord answered ahead of lookup,
    // an arm the table never absorbed -- shows up here as divergence.
    for case in equivalence_cases() {
        let label = format!(
            "{} / {:?} with {:?}",
            case.surface, case.code, case.modifiers
        );

        let mut dispatched = dispatch_workbench();
        (case.arrange)(&mut dispatched);
        let mut applied = dispatch_workbench();
        (case.arrange)(&mut applied);
        assert_eq!(
            observe(&dispatched),
            observe(&applied),
            "arrangement is not reproducible for {label}"
        );

        // Resolution as dispatch performs it, done here out of public parts:
        // the lookup order, then the first context in it that binds the chord.
        let resolved = applied
            .binding_lookup_order()
            .into_iter()
            .find_map(|context| default_binding(context, case.code, case.modifiers));

        let dispatched_outcome = outcome(
            dispatched.dispatch_event(Event::Key(KeyEvent::new(case.code, case.modifiers))),
        );
        let applied_outcome = match resolved {
            Some(action) => outcome(applied.apply_action(action)),
            // Nothing binds the chord, so the applying side does nothing and
            // dispatch is held to the same standard.
            None => Ok(()),
        };

        assert_eq!(
            dispatched_outcome, applied_outcome,
            "outcome differs between dispatch and application for {label}"
        );
        assert_eq!(
            observe(&dispatched),
            observe(&applied),
            "state differs between dispatch and application for {label}"
        );
    }
}

/// A workbench whose message field binds one extra chord to inserting a
/// newline, as an operator's `ui.toml` would.
fn workbench_with_configured_newline() -> Workbench {
    let inserts = ConfiguredAction::Invoke(Action::InsertMessageNewline);
    Workbench::new(TuiLaunchOptions {
        namespace: "agentmux".to_string(),
        sender_session: "tui".to_string(),
        relay_socket: PathBuf::from("/tmp/agentmux-dispatch-configured-relay.sock"),
        look_lines: None,
        available_bundles: vec!["agentmux".to_string()],
        bindings: Some(BindingConfiguration {
            presets: Vec::new(),
            preset_rows: Vec::new(),
            primary_modifier_on_macos: None,
            rows: vec![ConfiguredBinding {
                context: BindingContext::ComposeMessage,
                chord: parse_chord("ctrl+n").expect("the fixture chord parses"),
                enhanced: Some(inserts),
                standard: Some(inserts),
            }],
        }),
    })
}

#[test]
fn dispatch_resolves_against_the_configured_table_rather_than_the_compiled_one() {
    const CONFIGURED: KeyEvent = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);

    // The compiled table binds nothing to this chord in the message field, and
    // that is the premise the rest of the test rests on: without it, the chord
    // could reach the behavior through a row that was always there.
    assert_eq!(
        default_binding(
            BindingContext::ComposeMessage,
            CONFIGURED.code,
            CONFIGURED.modifiers
        ),
        None
    );

    // An unconfigured workbench is unmoved by it, so what follows is the
    // configuration's doing and not the keystroke's.
    let mut unconfigured = dispatch_workbench();
    compose_message(&mut unconfigured);
    unconfigured
        .dispatch_event(Event::Key(CONFIGURED))
        .expect("an unbound chord reaches no relay");
    assert_eq!(unconfigured.message_field(), "hello");

    let mut configured = workbench_with_configured_newline();
    compose_message(&mut configured);
    configured
        .dispatch_event(Event::Key(CONFIGURED))
        .expect("inserting a newline reaches no relay");
    assert_eq!(configured.message_field(), "hello\n");

    // The chord the compiled row already declared still reaches the behavior:
    // a configured row adds to the context rather than replacing what it did
    // not name.
    configured
        .dispatch_event(Event::Key(KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::CONTROL,
        )))
        .expect("inserting a newline reaches no relay");
    assert_eq!(configured.message_field(), "hello\n\n");
}
