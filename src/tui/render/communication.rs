use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

use super::super::state::{AppState, ChatHistoryDirection, Recipient};
use super::geometry::{
    compose_message_layout, compose_message_visible_start, compose_titled_block,
    split_workbench_rows, workbench_titled_block, wrap_text,
};

pub(super) fn render_communication_mode(frame: &mut Frame, area: Rect, state: &mut AppState) {
    let rows = split_workbench_rows(area, state);
    render_chat_history(frame, rows[0], state);
    render_compose(frame, rows[1], state);
}

fn render_compose(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = compose_titled_block("  Compose  ");
    let inner = block.inner(area);
    let to_style = if state.focus == super::super::state::FocusField::To {
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
    let block = workbench_titled_block("  Chat History  ");
    let inner = block.inner(area);
    state.set_chat_history_viewport_height(inner.height as usize);

    let lines = if state.chat_history.is_empty() {
        state.set_chat_history_total_lines(0);
        vec![Line::from("(no chat messages yet)")]
    } else {
        let all_lines = build_chat_history_lines(state, inner.width as usize);
        state.set_chat_history_total_lines(all_lines.len());
        let (start, end) = state.chat_history_line_window();
        all_lines[start..end].to_vec()
    };
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(block);
    frame.render_widget(paragraph, area);
}

/// Render the full chat history, oldest turn first, as a flat line list.
///
/// Each turn is a coloured sender-label line followed by the message body
/// indented two columns; a blank line separates consecutive turns. Outgoing
/// turns are labelled `you -> {peers}` (green); incoming turns `{peer} -> you`
/// (cyan). The body and any wrapped continuations stay indented so turn
/// boundaries remain unambiguous even in narrow terminals.
fn build_chat_history_lines(state: &AppState, content_width: usize) -> Vec<Line<'static>> {
    let label_width = content_width.max(1);
    let body_width = content_width.saturating_sub(2).max(1);
    let mut lines = Vec::<Line<'static>>::new();
    for (index, entry) in state.chat_history.iter().rev().enumerate() {
        if index > 0 {
            lines.push(Line::raw(""));
        }
        let (label_text, label_color) = match entry.direction {
            ChatHistoryDirection::Outgoing => (
                format!(
                    "you -> {}",
                    resolve_peer_list(entry.peer_session.as_str(), &state.recipients)
                ),
                Color::Green,
            ),
            ChatHistoryDirection::Incoming => (
                format!(
                    "{} -> you",
                    resolve_peer(entry.peer_session.as_str(), &state.recipients)
                ),
                Color::Cyan,
            ),
        };
        let label_style = Style::default()
            .fg(label_color)
            .add_modifier(Modifier::BOLD);
        for wrapped in wrap_text(label_text.as_str(), label_width) {
            lines.push(Line::from(Span::styled(wrapped, label_style)));
        }
        for body_line in entry.body.split('\n') {
            for wrapped in wrap_text(body_line, body_width) {
                lines.push(Line::from(vec![Span::raw("  "), Span::raw(wrapped)]));
            }
        }
    }
    lines
}

/// Resolve a comma-joined outgoing target list to display labels, one per peer.
fn resolve_peer_list(peer_session: &str, recipients: &[Recipient]) -> String {
    peer_session
        .split(", ")
        .map(|session_id| resolve_peer(session_id, recipients))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolve a canonical session id to `Display Name <session>`, falling back to
/// `<session>` when the recipient roster carries no display name.
fn resolve_peer(session_id: &str, recipients: &[Recipient]) -> String {
    let display_name = recipients
        .iter()
        .find(|recipient| recipient.session_name == session_id)
        .and_then(|recipient| recipient.display_name.as_deref());
    match display_name {
        Some(name) => format!("{name} <{session_id}>"),
        None => format!("<{session_id}>"),
    }
}
