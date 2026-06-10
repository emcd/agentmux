use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};

use super::super::super::state::{AppState, ScreenMode};
use super::super::super::status::{RecipientReadiness, format_recipient_picker_label};
use super::super::geometry::centered_rect;

pub(in crate::tui::render) fn render_picker_overlay(frame: &mut Frame, state: &mut AppState) {
    let popup = centered_rect(70, 70, frame.area());
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Recipient Picker");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let status_line = super::bundle_picker::bundle_status_header_line(state.bundle_status.as_ref());
    frame.render_widget(Paragraph::new(status_line), sections[0]);

    let items = if state.recipients.is_empty() {
        vec![ListItem::new("(no recipients)")]
    } else {
        state
            .recipients
            .iter()
            .map(|recipient| {
                let readiness = RecipientReadiness::from_ready(recipient.ready);
                let label = format_recipient_picker_label(
                    recipient.session_name.as_str(),
                    recipient.display_name.as_deref(),
                    readiness,
                );
                ListItem::new(Line::from(Span::styled(
                    label,
                    recipient_readiness_style(readiness),
                )))
            })
            .collect::<Vec<_>>()
    };
    let list = List::new(items).highlight_style(Style::default().bg(Color::Blue).fg(Color::White));
    frame.render_stateful_widget(list, sections[1], &mut state.picker_state);

    frame.render_widget(Paragraph::new(picker_hint_line(state.mode)), sections[2]);
}

/// Builds the one-line keybinding hint strip shown at the foot of the recipient
/// picker overlay. The `Enter` action is context-sensitive: in Communication
/// mode it inserts the selection into the To field; in Interaction mode it opens
/// the selected session in the look+raww view.
fn picker_hint_line(mode: ScreenMode) -> Line<'static> {
    let choose_action = match mode {
        ScreenMode::Communication => "Insert into To",
        ScreenMode::Interaction => "Open (look+raww)",
    };
    Line::from(vec![
        picker_hint_key("Enter"),
        Span::raw(format!(" {choose_action}")),
        Span::raw("   "),
        picker_hint_key("Esc"),
        Span::raw(" Close"),
        Span::raw("   "),
        picker_hint_key("Up/Down"),
        Span::raw(" Move"),
    ])
}

fn picker_hint_key(label: &str) -> Span<'static> {
    Span::styled(
        label.to_string(),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )
}

fn recipient_readiness_style(readiness: RecipientReadiness) -> Style {
    match readiness {
        RecipientReadiness::Ready => Style::default(),
        RecipientReadiness::NotReady => Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    }
}
