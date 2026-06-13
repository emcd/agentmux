use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use super::super::super::state::AppState;
use super::super::geometry::centered_rect;

pub(in crate::tui::render) fn render_help_overlay(frame: &mut Frame, _state: &AppState) {
    let popup = centered_rect(72, 80, frame.area());
    frame.render_widget(Clear, popup);
    let block = Block::default().borders(Borders::ALL).title("Help");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(inner);

    let left_lines = vec![
        help_section_heading("Modes"),
        Line::from("F4: Toggle Communication / Interaction"),
        Line::from("F1: Toggle help"),
        Line::from("F2: Open recipient picker"),
        Line::from("F3: Open events overlay"),
        Line::from("F5: Open bundle picker"),
        Line::from("Ctrl+R: Refresh recipients"),
        Line::from("Ctrl+C: Quit from anywhere"),
        Line::from(""),
        help_section_heading("Communication Mode (default)"),
        Line::from("Tab / Shift+Tab: Focus next/previous"),
        Line::from("Ctrl+Space: Trigger completion in To"),
        Line::from("Up/Down in To: Navigate completion"),
        Line::from("Arrows/Home/End in Message: Move cursor"),
        Line::from("Ctrl+A/Ctrl+E in Message: Line start/end"),
        Line::from("Enter: Accept completion / send"),
        Line::from("Ctrl+J: Insert newline in Message"),
        Line::from("Esc in Message: Snap history to latest"),
        Line::from("PgUp/PgDn: Scroll chat history"),
        Line::from("Mouse wheel: Scroll chat history"),
        Line::from(""),
        help_section_heading("To Field Grammar"),
        Line::from("session — route to active bundle"),
        Line::from("session@bundle — route to named bundle"),
        Line::from("session@GLOBAL — relay-wide user"),
        Line::from("Comma-separate multiple recipients"),
    ];
    let right_lines = vec![
        help_section_heading("Interaction Mode"),
        Line::from("PgUp/PgDn: Scroll look snapshot"),
        Line::from("Write input (write has text or no pending):"),
        Line::from("  Arrows/Home/End: Move write cursor"),
        Line::from("  Enter: Dispatch write to active target"),
        Line::from("  Ctrl+J: Insert newline"),
        Line::from("  Backspace: Backspace write input"),
        Line::from("Choose (write empty and pending exists):"),
        Line::from("  Left/Right: Previous/next request"),
        Line::from("  Up/Down: Previous/next ACP option"),
        Line::from("  Enter: Resolve selected option"),
        Line::from("  c: Resolve as cancelled"),
        Line::from(""),
        help_section_heading("Session Picker (F2)"),
        Line::from("Enter (Communication): Insert into To"),
        Line::from("Enter (Interaction): Open with look"),
        Line::from("Esc / F2: Close picker"),
        Line::from("Up/Down: Move picker selection"),
        Line::from(""),
        help_section_heading("Bundle Picker (F5)"),
        Line::from("Enter: Switch active bundle"),
        Line::from("Esc / F5: Close picker"),
        Line::from("Up/Down: Move bundle selection"),
    ];

    frame.render_widget(
        Paragraph::new(left_lines).wrap(Wrap { trim: false }),
        columns[0],
    );
    frame.render_widget(
        Paragraph::new(right_lines).wrap(Wrap { trim: false }),
        columns[1],
    );
}

fn help_section_heading(text: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        text,
        ratatui::style::Style::default().add_modifier(Modifier::BOLD),
    ))
}
