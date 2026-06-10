use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    widgets::{Block, Borders},
};

use super::super::state::AppState;
use super::frame::{WORKBENCH_MIN_CHAT_HEIGHT, WORKBENCH_MIN_COMPOSE_HEIGHT};

pub(super) fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
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

pub(super) fn split_workbench_rows(area: Rect, state: &AppState) -> [Rect; 2] {
    let bottom_height = compute_compose_height(area.width, area.height, state);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(WORKBENCH_MIN_CHAT_HEIGHT),
            Constraint::Length(bottom_height),
        ])
        .split(area);
    [rows[0], rows[1]]
}

pub(super) fn compute_compose_height(
    available_width: u16,
    available_height: u16,
    state: &AppState,
) -> u16 {
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

pub(super) fn workbench_titled_block(title: &'static str) -> Block<'static> {
    Block::default()
        .borders(Borders::TOP)
        .title(title)
        .title_alignment(Alignment::Center)
}

pub(super) fn compose_titled_block(title: &'static str) -> Block<'static> {
    Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .title(title)
        .title_alignment(Alignment::Center)
}

#[derive(Clone, Debug)]
pub(super) struct MessageLayout {
    pub lines: Vec<String>,
    pub cursor_row: usize,
    pub cursor_col: usize,
}

pub(super) fn compose_message_layout(
    value: &str,
    cursor_index: usize,
    width: usize,
) -> MessageLayout {
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

pub(super) fn compose_message_visible_start(
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

pub(super) fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 || text.is_empty() {
        return vec![text.to_string()];
    }
    if text.chars().count() <= width {
        return vec![text.to_string()];
    }
    let mut wrapped = Vec::<String>::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for character in text.chars() {
        if current_width >= width {
            wrapped.push(current);
            current = String::new();
            current_width = 0;
        }
        current.push(character);
        current_width += 1;
    }
    if !current.is_empty() {
        wrapped.push(current);
    }
    wrapped
}

pub(super) fn raww_titled_block(title: &'static str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_alignment(Alignment::Center)
}
