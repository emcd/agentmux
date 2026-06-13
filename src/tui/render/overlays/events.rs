use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use super::super::super::state::AppState;
use super::super::geometry::centered_rect;

pub(in crate::tui::render) fn render_events_overlay(frame: &mut Frame, state: &mut AppState) {
    let popup = centered_rect(80, 70, frame.area());
    frame.render_widget(Clear, popup);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(6)])
        .split(popup);

    let pending_items = if state.pending_choices.is_empty() {
        vec![ListItem::new("(no pending choice requests)")]
    } else {
        state
            .pending_choices
            .iter()
            .map(|entry| {
                let target = entry.target_session.as_deref().unwrap_or("-");
                let kind = entry.requested_kind.as_deref().unwrap_or("-");
                let enqueued_at = entry.enqueued_at.as_deref().unwrap_or("-");
                ListItem::new(format!(
                    "{} target={} kind={} enqueued={}",
                    entry.choice_request_id, target, kind, enqueued_at
                ))
            })
            .collect::<Vec<_>>()
    };
    let pending_list = List::new(pending_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Pending Choices"),
        )
        .highlight_style(Style::default().bg(Color::Blue).fg(Color::White));
    frame.render_stateful_widget(pending_list, sections[0], &mut state.pending_choices_state);

    let lines = if state.event_history.is_empty() {
        vec![Line::from("(no delivery events captured yet)")]
    } else {
        state
            .event_history
            .iter()
            .take((sections[1].height.saturating_sub(2)) as usize)
            .map(|line| Line::from(Span::raw(line.clone())))
            .collect::<Vec<_>>()
    };
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title("Events"));
    frame.render_widget(paragraph, sections[1]);
}
