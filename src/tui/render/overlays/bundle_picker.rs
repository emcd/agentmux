use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};

use super::super::super::state::AppState;
use super::super::super::status::BundleStatusDisplay;

pub(in crate::tui::render) fn render_bundle_picker_overlay(
    frame: &mut Frame,
    state: &mut AppState,
) {
    let popup = super::super::geometry::centered_rect(50, 50, frame.area());
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Bundle Picker");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);

    let active_line = Line::from(vec![
        Span::raw("active: "),
        Span::styled(
            state.bundle_name.clone(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    frame.render_widget(Paragraph::new(active_line), sections[0]);

    let items = if state.available_bundles.is_empty() {
        vec![ListItem::new("(no bundles configured)")]
    } else {
        state
            .available_bundles
            .iter()
            .map(|name| {
                let is_active = name == &state.bundle_name;
                let label = if is_active {
                    format!("{name} [active]")
                } else {
                    name.clone()
                };
                let style = if is_active {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(Line::from(Span::styled(label, style)))
            })
            .collect::<Vec<_>>()
    };
    let list = List::new(items).highlight_style(Style::default().bg(Color::Blue).fg(Color::White));
    frame.render_stateful_widget(list, sections[1], &mut state.bundle_picker_state);
}

pub(super) fn bundle_status_header_line(status: Option<&BundleStatusDisplay>) -> Line<'static> {
    let Some(status) = status else {
        return Line::from(Span::styled(
            "(bundle status pending list refresh)",
            Style::default().fg(Color::DarkGray),
        ));
    };
    let style =
        bundle_status_severity_style(super::super::super::status::bundle_status_severity(status));
    Line::from(Span::styled(
        super::super::super::status::format_bundle_status_line(status),
        style,
    ))
}

fn bundle_status_severity_style(
    severity: super::super::super::status::BundleStatusSeverity,
) -> Style {
    use super::super::super::status::BundleStatusSeverity;
    match severity {
        BundleStatusSeverity::Healthy => Style::default().fg(Color::Green),
        BundleStatusSeverity::Degraded => Style::default().fg(Color::Yellow),
        BundleStatusSeverity::HostedDown => {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        }
        BundleStatusSeverity::Unhosted => Style::default().fg(Color::DarkGray),
    }
}
