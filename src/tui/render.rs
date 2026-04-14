use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use super::state::{AppState, ChatHistoryDirection, FocusField, StatusEntry};

const WORKBENCH_MIN_CHAT_HEIGHT: u16 = 1;
const WORKBENCH_MIN_COMPOSE_HEIGHT: u16 = 4;

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
    render_compose_cursor(frame, chunks[1], state);

    if state.help_overlay_open {
        render_help_overlay(frame, state);
    }
    if state.picker_open {
        render_picker_overlay(frame, state);
    }
    if state.events_overlay_open {
        render_events_overlay(frame, state);
    }
    if state.look_overlay_open {
        render_look_overlay(frame, state);
    }
}

fn render_header(frame: &mut Frame, area: Rect, state: &AppState) {
    let text = vec![Line::from(vec![
        Span::styled(
            "Agentmux",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "  Bundle: {}  Sender: {}  Pending Deliveries: {}",
            state.bundle_name,
            state.sender_session,
            state.pending_deliveries_count()
        )),
    ])];
    let paragraph = Paragraph::new(text).style(Style::default().bg(Color::DarkGray));
    frame.render_widget(paragraph, area);
}

fn render_main(frame: &mut Frame, area: Rect, state: &mut AppState) {
    render_workbench_panes(frame, area, state);
}

fn render_compose_cursor(frame: &mut Frame, area: Rect, state: &AppState) {
    if state.help_overlay_open
        || state.picker_open
        || state.events_overlay_open
        || state.look_overlay_open
    {
        return;
    }
    let rows = split_workbench_rows(area, state);
    let compose_inner = compose_titled_block("  Compose  ").inner(rows[1]);
    let Some((x, y)) = compose_cursor_position(compose_inner, state) else {
        return;
    };
    frame.set_cursor_position((x, y));
}

fn compose_cursor_position(inner_area: Rect, state: &AppState) -> Option<(u16, u16)> {
    if inner_area.width == 0 || inner_area.height < 2 {
        return None;
    }
    let inner_left = inner_area.x;
    let inner_top = inner_area.y;
    let inner_right = inner_area
        .x
        .saturating_add(inner_area.width)
        .saturating_sub(1);
    let inner_bottom = inner_area
        .y
        .saturating_add(inner_area.height)
        .saturating_sub(1);
    let inner_width = inner_area.width;

    let (raw_x, raw_y) = match state.focus {
        FocusField::To => {
            let prefix_width = "To: ".chars().count() as u16;
            let field_width = inner_width.saturating_sub(prefix_width);
            let cursor_column = visible_cursor_column(state.to_field.as_str(), field_width);
            (
                inner_left
                    .saturating_add(prefix_width)
                    .saturating_add(cursor_column),
                inner_top,
            )
        }
        FocusField::Message => {
            let message_view_height = inner_area.height.saturating_sub(1) as usize;
            if message_view_height == 0 {
                return None;
            }
            let message_layout = compose_message_layout(
                state.message_field.as_str(),
                state.message_cursor_index(),
                inner_width as usize,
            );
            let start = compose_message_visible_start(
                message_layout.lines.len(),
                message_layout.cursor_row,
                message_view_height,
            );
            let cursor_row = message_layout
                .cursor_row
                .saturating_sub(start)
                .saturating_add(1);
            let cursor_column = visible_cursor_column_count(message_layout.cursor_col, inner_width);
            (
                inner_left.saturating_add(cursor_column),
                inner_top.saturating_add(cursor_row as u16),
            )
        }
    };

    Some((raw_x.min(inner_right), raw_y.min(inner_bottom)))
}

fn visible_cursor_column(value: &str, width: u16) -> u16 {
    if width == 0 {
        return 0;
    }
    let value_width = value.chars().count() as u16;
    value_width.min(width.saturating_sub(1))
}

fn visible_cursor_column_count(count: usize, width: u16) -> u16 {
    if width == 0 {
        return 0;
    }
    (count as u16).min(width.saturating_sub(1))
}

fn render_workbench_panes(frame: &mut Frame, area: Rect, state: &mut AppState) {
    let rows = split_workbench_rows(area, state);
    render_chat_history(frame, rows[0], state);
    render_compose(frame, rows[1], state);
}

fn render_compose(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = compose_titled_block("  Compose  ");
    let inner = block.inner(area);
    let to_style = if state.focus == FocusField::To {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let mut lines = vec![Line::from(vec![
        Span::styled("To: ", to_style.add_modifier(Modifier::BOLD)),
        Span::raw(state.to_field.as_str()),
    ])];
    let message_layout = compose_message_layout(
        state.message_field.as_str(),
        state.message_cursor_index(),
        inner.width.max(1) as usize,
    );
    let message_view_height = inner.height.saturating_sub(1) as usize;
    let start = compose_message_visible_start(
        message_layout.lines.len(),
        message_layout.cursor_row,
        message_view_height,
    );
    let end = (start + message_view_height).min(message_layout.lines.len());
    lines.extend(
        message_layout.lines[start..end]
            .iter()
            .map(|line| Line::from(Span::raw(line.clone()))),
    );

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

fn render_chat_history(frame: &mut Frame, area: Rect, state: &mut AppState) {
    let viewport_height = workbench_titled_block("  Chat History  ")
        .inner(area)
        .height as usize;
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
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(workbench_titled_block("  Chat History  "));
    frame.render_widget(paragraph, area);
}

fn render_look_overlay(frame: &mut Frame, state: &AppState) {
    let popup = centered_rect(90, 80, frame.area());
    frame.render_widget(Clear, popup);
    let title = match (&state.look_target, &state.look_captured_at) {
        (Some(target), Some(captured_at)) => {
            format!(
                "Look Snapshot target={} captured_at={}",
                target, captured_at
            )
        }
        (Some(target), None) => format!("Look Snapshot target={}", target),
        _ => "Look Snapshot".to_string(),
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
    frame.render_widget(paragraph, popup);
}

fn render_footer(frame: &mut Frame, area: Rect, state: &AppState) {
    let line = state
        .status_history
        .front()
        .map(render_status_line)
        .unwrap_or_else(|| Line::from("Ready."));
    let footer = Paragraph::new(vec![line])
        .wrap(Wrap { trim: false })
        .style(Style::default().bg(Color::DarkGray));
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
                .title("Recipient Picker"),
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
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title("Events"));
    frame.render_widget(paragraph, popup);
}

fn render_help_overlay(frame: &mut Frame, _state: &AppState) {
    let popup = centered_rect(72, 70, frame.area());
    frame.render_widget(Clear, popup);
    let lines = vec![
        Line::from(Span::styled(
            "Main Screen",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("F1: Toggle help"),
        Line::from("F2: Open recipient picker"),
        Line::from("F3: Open events"),
        Line::from("Ctrl+R: Refresh recipients"),
        Line::from("Tab / Shift+Tab: Focus next/previous"),
        Line::from("Ctrl+Space: Trigger recipient completion in To"),
        Line::from("Up/Down in To: Navigate active completion"),
        Line::from("Arrows/Home/End in Message: Move cursor"),
        Line::from("Ctrl+A/Ctrl+E in Message: Move to line start/end"),
        Line::from("Enter: Accept completion in To (adds ', ') / send in Message"),
        Line::from("Ctrl+J: Insert newline in Message"),
        Line::from("Esc in Message: Snap history to latest"),
        Line::from("PgUp/PgDn: Scroll chat history"),
        Line::from("Mouse wheel: Scroll chat history"),
        Line::from(""),
        Line::from(Span::styled(
            "Session Picker",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("Enter: Choose selected recipient into To"),
        Line::from("l: Capture look snapshot for selected recipient"),
        Line::from("Esc / F2: Close picker"),
        Line::from("Up/Down: Move picker selection"),
        Line::from(""),
        Line::from(Span::styled(
            "Look Overlay",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("Esc: Close look and return to picker"),
        Line::from("F2: Open picker"),
        Line::from("F3: Open events"),
        Line::from(""),
        Line::from(Span::styled(
            "Overlays",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("Esc closes active overlay"),
        Line::from(""),
        Line::from(Span::styled(
            "General",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("Ctrl+C: Quit from anywhere"),
    ];
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title("Help"));
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

fn split_workbench_rows(area: Rect, state: &AppState) -> [Rect; 2] {
    let compose_height = compute_compose_height(area.width, area.height, state);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(WORKBENCH_MIN_CHAT_HEIGHT),
            Constraint::Length(compose_height),
        ])
        .split(area);
    [rows[0], rows[1]]
}

fn compute_compose_height(available_width: u16, available_height: u16, state: &AppState) -> u16 {
    if available_height <= WORKBENCH_MIN_COMPOSE_HEIGHT {
        return available_height;
    }

    let message_line_count = compose_message_layout(
        state.message_field.as_str(),
        state.message_cursor_index(),
        available_width.max(1) as usize,
    )
    .lines
    .len()
    .max(1) as u16;
    let desired = message_line_count
        .saturating_add(1) // To row
        .saturating_add(2); // top + bottom borders
    let max_compose = available_height.saturating_sub(WORKBENCH_MIN_CHAT_HEIGHT);
    let min_compose = WORKBENCH_MIN_COMPOSE_HEIGHT.min(max_compose.max(1));
    desired.clamp(min_compose, max_compose.max(min_compose))
}

fn workbench_titled_block(title: &'static str) -> Block<'static> {
    Block::default()
        .borders(Borders::TOP)
        .title(title)
        .title_alignment(Alignment::Center)
}

fn compose_titled_block(title: &'static str) -> Block<'static> {
    Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .title(title)
        .title_alignment(Alignment::Center)
}

#[derive(Clone, Debug)]
struct MessageLayout {
    lines: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
}

fn compose_message_layout(value: &str, cursor_index: usize, width: usize) -> MessageLayout {
    let width = width.max(1);
    let clamped_cursor = cursor_index.min(value.len());

    let mut lines = Vec::<String>::new();
    let mut line = String::new();
    let mut line_width = 0usize;
    let mut line_index = 0usize;

    let mut cursor_row = 0usize;
    let mut cursor_col = 0usize;
    let mut cursor_set = false;

    for (index, character) in value.char_indices() {
        if index == clamped_cursor {
            cursor_row = line_index;
            cursor_col = line_width;
            cursor_set = true;
        }

        if character == '\n' {
            lines.push(line);
            line = String::new();
            line_width = 0;
            line_index += 1;
            continue;
        }

        if line_width + 1 > width && line_width > 0 {
            lines.push(line);
            line = String::new();
            line_width = 0;
            line_index += 1;
        }

        line.push(character);
        line_width += 1;
    }

    if !cursor_set {
        cursor_row = line_index;
        cursor_col = line_width;
    }

    lines.push(line);
    MessageLayout {
        lines,
        cursor_row,
        cursor_col,
    }
}

fn compose_message_visible_start(
    total_lines: usize,
    cursor_row: usize,
    view_height: usize,
) -> usize {
    if view_height == 0 || total_lines <= view_height {
        return 0;
    }
    let max_start = total_lines.saturating_sub(view_height);
    cursor_row
        .saturating_add(1)
        .saturating_sub(view_height)
        .min(max_start)
}
