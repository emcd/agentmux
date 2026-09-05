use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::actions::{self, Action, BindingContext};
use super::super::sender_bound_bundle;
use super::super::state::AppState;
use super::cursor::render_active_cursor;
use super::overlays::{
    events::render_events_overlay, help::render_help_overlay, picker::render_picker_overlay,
};

pub(super) const WORKBENCH_MIN_CHAT_HEIGHT: u16 = 1;
pub(super) const WORKBENCH_MIN_COMPOSE_HEIGHT: u16 = 4;
pub(super) const INTERACTION_RAWW_PANE_HEIGHT: u16 = 8;
pub(super) const INTERACTION_TARGET_HEADER_HEIGHT: u16 = 1;

pub(crate) fn render(frame: &mut Frame, state: &mut AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(12),
            Constraint::Length(1),
        ])
        .split(frame.area());

    render_header(frame, chunks[0], state);
    render_main(frame, chunks[1], state);
    render_footer(frame, chunks[2], state);
    render_active_cursor(frame, chunks[1], state);

    if state.help_overlay_open {
        render_help_overlay(frame, state);
    }
    if state.picker_open {
        render_picker_overlay(frame, state);
    }
    if state.events_overlay_open {
        render_events_overlay(frame, state);
    }
}

fn render_header(frame: &mut Frame, area: Rect, state: &AppState) {
    let mut spans = vec![Span::styled(
        "Agentmux",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )];
    // A relay-wide (`@GLOBAL`) sender is bound to no bundle, so the `Bundle:`
    // field is meaningless — the principal id already encodes the namespace. The
    // browsing bundle survives only as the `List` enumeration target, not as a
    // sender binding, so it is not surfaced in the header for these principals.
    if sender_bound_bundle(&state.sender_session, &state.namespace).is_some() {
        spans.push(Span::raw("  Bundle: "));
        spans.push(Span::styled(
            state.namespace.clone(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::raw(format!(
        "  Sender: {}  Pending Deliveries: {}",
        state.sender_session,
        state.pending_deliveries_count()
    )));
    let paragraph =
        Paragraph::new(vec![Line::from(spans)]).style(Style::default().bg(Color::DarkGray));
    frame.render_widget(paragraph, area);
}

pub(super) fn render_main(frame: &mut Frame, area: Rect, state: &mut AppState) {
    match state.mode {
        super::super::state::ScreenMode::Communication => {
            super::communication::render_communication_mode(frame, area, state)
        }
        super::super::state::ScreenMode::Interaction => {
            super::interaction::render_interaction_mode(frame, area, state)
        }
    }
}

/// The footer's mode-switch hint, generated from whichever surface owns the
/// chord at this instant.
///
/// The dispatch context rather than a fixed one, which is the opposite of what
/// a pane hint does. A pane hint annotates the surface it sits on; the footer
/// spans the whole workbench and says what pressing a key would do right now,
/// which is the question `binding_context` answers. Where nothing binds the
/// switch, the destination is still named and no chord is invented.
fn mode_switch_hint(state: &AppState, destination: &str) -> String {
    match actions::binding_for(
        &state.bindings,
        actions::binding_context(state),
        Action::ToggleMode,
    ) {
        Some(entry) => format!("{} → {destination}", entry.primary_chord()),
        None => format!("→ {destination}"),
    }
}

/// The status line before anything has happened.
///
/// It is composed here rather than seeded into `AppState` because a seeded
/// string is fixed at construction, and the chord it names is not known until
/// the probe outcome has settled the effective table. Reading the table at
/// render time is what keeps the line naming the chord actually in force. Help
/// is a global row, so the global context is where its chord is declared.
fn startup_status(state: &AppState) -> String {
    match actions::binding_for(
        &state.bindings,
        BindingContext::Global,
        Action::ToggleHelpOverlay,
    ) {
        Some(entry) => format!("Ready. Press {} for help.", entry.primary_chord()),
        None => "Ready. The help overlay lists every binding.".to_string(),
    }
}

fn render_footer(frame: &mut Frame, area: Rect, state: &AppState) {
    let (mode_label, toggle_hint) = match state.mode {
        super::super::state::ScreenMode::Communication => {
            ("[Communication]", mode_switch_hint(state, "Interaction"))
        }
        super::super::state::ScreenMode::Interaction => {
            ("[Interaction]", mode_switch_hint(state, "Communication"))
        }
    };
    let status_line = state
        .status_history
        .front()
        .map(render_status_line)
        .unwrap_or_else(|| Line::from(startup_status(state)));
    let mut spans = vec![
        Span::styled(
            mode_label,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(toggle_hint, Style::default().fg(Color::Yellow)),
        Span::raw("  "),
    ];
    spans.extend(status_line.spans);
    let footer = Paragraph::new(Line::from(spans))
        .wrap(ratatui::widgets::Wrap { trim: false })
        .style(Style::default().bg(Color::DarkGray));
    frame.render_widget(footer, area);
}

fn render_status_line(entry: &super::super::state::StatusEntry) -> Line<'static> {
    match entry.code.as_ref() {
        Some(code) => Line::from(vec![
            Span::styled(
                format!("[{code}] "),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(entry.message.clone()),
        ]),
        None => Line::from(Span::raw(entry.message.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    use crate::tui::state::{ScreenMode, TuiLaunchOptions};

    fn workbench() -> AppState {
        AppState::new(TuiLaunchOptions {
            namespace: "agentmux".to_string(),
            sender_session: "tui".to_string(),
            relay_socket: std::path::PathBuf::from("/tmp/agentmux-frame-render.sock"),
            look_lines: None,
            available_bundles: vec!["agentmux".to_string()],
            bindings: None,
        })
    }

    fn rendered(state: &mut AppState) -> String {
        let mut terminal = Terminal::new(TestBackend::new(110, 30)).expect("terminal");
        terminal.draw(|frame| render(frame, state)).expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|row| {
                (0..buffer.area.width)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_footer_names_the_chords_the_table_declares_rather_than_literals() {
        // The chords are read from the table here, not from the functions that
        // compose the footer: asking those to agree with themselves would pass
        // over any stale literal they happened to return. A row moved to a
        // different chord fails this without anyone remembering the footer
        // exists.
        let mut state = workbench();
        let help = actions::binding_for(
            &state.bindings,
            BindingContext::Global,
            Action::ToggleHelpOverlay,
        )
        .expect("the global context binds toggling help");

        // Nothing has happened yet, so the startup line is what the footer
        // falls back to. It is the only place the operator is told how to
        // reach the catalogue at all.
        let text = rendered(&mut state);
        assert!(
            text.contains(&format!("Press {} for help.", help.primary_chord())),
            "the startup status does not name {:?}:\n{text}",
            help.primary_chord()
        );

        // Both modes, because the destination differs and the chord must not.
        for (mode, destination) in [
            (ScreenMode::Communication, "Interaction"),
            (ScreenMode::Interaction, "Communication"),
        ] {
            state.mode = mode;
            let switch = actions::binding_for(
                &state.bindings,
                actions::binding_context(&state),
                Action::ToggleMode,
            )
            .expect("every surface binds switching modes");
            let text = rendered(&mut state);
            assert!(
                text.contains(&format!("{} → {destination}", switch.primary_chord())),
                "the footer does not offer {:?} → {destination}:\n{text}",
                switch.primary_chord()
            );
        }
    }
}
