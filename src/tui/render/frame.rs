use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::sender_bound_bundle;
use super::super::state::AppState;
use super::cursor::render_active_cursor;
use super::overlays::{
    bundle_picker::render_bundle_picker_overlay, events::render_events_overlay,
    help::render_help_overlay, recipient_picker::render_picker_overlay,
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
    if state.bundle_picker_open {
        render_bundle_picker_overlay(frame, state);
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
    if sender_bound_bundle(&state.sender_session, &state.bundle_name).is_some() {
        spans.push(Span::raw("  Bundle: "));
        spans.push(Span::styled(
            state.bundle_name.clone(),
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

fn render_footer(frame: &mut Frame, area: Rect, state: &AppState) {
    let (mode_label, toggle_hint) = match state.mode {
        super::super::state::ScreenMode::Communication => ("[Communication]", "F4 → Interaction"),
        super::super::state::ScreenMode::Interaction => ("[Interaction]", "F4 → Communication"),
    };
    let status_line = state
        .status_history
        .front()
        .map(render_status_line)
        .unwrap_or_else(|| Line::from("Ready."));
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
