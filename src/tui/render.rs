use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use super::state::{AppState, ChatHistoryDirection, FocusField, StatusEntry};

pub(crate) fn render(frame: &mut Frame, state: &mut AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(12),
            Constraint::Length(7),
        ])
        .split(frame.area());

    render_header(frame, chunks[0], state);
    render_main(frame, chunks[1], state);
    render_footer(frame, chunks[2], state);

    if state.picker_open {
        render_picker_overlay(frame, state);
    }
    if state.events_overlay_open {
        render_events_overlay(frame, state);
    }
}

fn render_header(frame: &mut Frame, area: Rect, state: &AppState) {
    let focus = state.active_recipient_field_name();
    let selected = state
        .selected_recipient_id()
        .unwrap_or_else(|| "-".to_string());
    let text = vec![Line::from(vec![
        Span::styled(
            "agentmux tui",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "  bundle={} sender={} focus={} selected={} pending={}",
            state.bundle_name,
            state.sender_session,
            focus,
            selected,
            state.pending_deliveries_count()
        )),
    ])];
    let paragraph = Paragraph::new(text).block(Block::default().borders(Borders::ALL));
    frame.render_widget(paragraph, area);
}

fn render_main(frame: &mut Frame, area: Rect, state: &mut AppState) {
    render_workbench_panes(frame, area, state);
}

fn render_workbench_panes(frame: &mut Frame, area: Rect, state: &mut AppState) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(9),
            Constraint::Length(9),
            Constraint::Min(8),
        ])
        .split(area);
    render_compose(frame, rows[0], state);
    render_chat_history(frame, rows[1], state);
    render_snapshot(frame, rows[2], state);
}

fn render_compose(frame: &mut Frame, area: Rect, state: &AppState) {
    let to_style = if state.focus == FocusField::To {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let message_style = if state.focus == FocusField::Message {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled("To: ", to_style.add_modifier(Modifier::BOLD)),
            Span::raw(state.to_field.as_str()),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Message:",
            message_style.add_modifier(Modifier::BOLD),
        )),
    ];
    lines.extend(
        state
            .message_field
            .lines()
            .take(4)
            .map(|line| Line::from(Span::raw(line.to_string()))),
    );
    if state.message_field.lines().count() > 4 {
        lines.push(Line::from("…"));
    }

    let paragraph =
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(
            "Compose (Tab complete/focus, Enter accept completion, Ctrl+Space cycle, Ctrl+S send)",
        ));
    frame.render_widget(paragraph, area);
}

fn render_chat_history(frame: &mut Frame, area: Rect, state: &mut AppState) {
    let viewport_height = area.height.saturating_sub(2) as usize;
    state.set_chat_history_viewport_height(viewport_height);

    let lines = if state.chat_history.is_empty() {
        vec![Line::from("(no chat messages yet)")]
    } else {
        state
            .visible_chat_history_entries()
            .iter()
            .map(|entry| {
                let marker = match entry.direction {
                    ChatHistoryDirection::Outgoing => "out",
                    ChatHistoryDirection::Incoming => "in ",
                };
                let metadata = entry
                    .message_id
                    .as_ref()
                    .map(|message_id| format!(" [{}]", message_id))
                    .unwrap_or_default();
                Line::from(Span::raw(format!(
                    "{marker} {}{}: {}",
                    entry.peer_session, metadata, entry.body
                )))
            })
            .collect::<Vec<_>>()
    };
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Chat History (PgUp/PgDn)"),
    );
    frame.render_widget(paragraph, area);
}

fn render_snapshot(frame: &mut Frame, area: Rect, state: &AppState) {
    let title = match (&state.look_target, &state.look_captured_at) {
        (Some(target), Some(captured_at)) => {
            format!(
                "Look Snapshot target={} captured_at={}",
                target, captured_at
            )
        }
        (Some(target), None) => format!("Look Snapshot target={}", target),
        _ => "Look Snapshot (Ctrl+L)".to_string(),
    };
    let lines = if state.look_snapshot_lines.is_empty() {
        vec![Line::from("(no snapshot captured)")]
    } else {
        state
            .look_snapshot_lines
            .iter()
            .map(|line| Line::from(Span::raw(line.clone())))
            .collect::<Vec<_>>()
    };
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(paragraph, area);
}

fn render_footer(frame: &mut Frame, area: Rect, state: &AppState) {
    let lines = state
        .status_history
        .iter()
        .take(4)
        .map(render_status_line)
        .collect::<Vec<_>>();
    let footer = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default()
            .borders(Borders::ALL)
            .title(
                "Status (Ctrl+R refresh, Ctrl+L look, F2 picker, F3 events, PgUp/PgDn history, Esc/Ctrl+Q quit)",
            ),
    );
    frame.render_widget(footer, area);
}

fn render_status_line(entry: &StatusEntry) -> Line<'static> {
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

fn render_picker_overlay(frame: &mut Frame, state: &mut AppState) {
    let popup = centered_rect(70, 70, frame.area());
    frame.render_widget(Clear, popup);
    let items = if state.recipients.is_empty() {
        vec![ListItem::new("(no recipients)")]
    } else {
        state
            .recipients
            .iter()
            .map(|recipient| {
                if let Some(display_name) = recipient.display_name.as_ref() {
                    ListItem::new(format!("{} ({})", recipient.session_name, display_name))
                } else {
                    ListItem::new(recipient.session_name.clone())
                }
            })
            .collect::<Vec<_>>()
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Recipient Picker (Enter insert, Esc close)"),
        )
        .highlight_style(Style::default().bg(Color::Blue).fg(Color::White));
    frame.render_stateful_widget(list, popup, &mut state.picker_state);
}

fn render_events_overlay(frame: &mut Frame, state: &AppState) {
    let popup = centered_rect(80, 70, frame.area());
    frame.render_widget(Clear, popup);
    let lines = if state.event_history.is_empty() {
        vec![Line::from("(no delivery events captured yet)")]
    } else {
        state
            .event_history
            .iter()
            .take((popup.height.saturating_sub(2)) as usize)
            .map(|line| Line::from(Span::raw(line.clone())))
            .collect::<Vec<_>>()
    };
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Events (Esc/F3 close)"),
    );
    frame.render_widget(paragraph, popup);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}
