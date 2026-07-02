use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};

use super::super::super::state::{AppState, PickerColumn, ScreenMode};
use super::super::super::status::{
    BundleStatusDisplay, BundleStatusSeverity, RecipientReadiness, bundle_status_severity,
    format_bundle_status_line, format_recipient_picker_label, format_startup_failure_lines,
};
use super::super::geometry::centered_rect;

/// Cap on per-session startup failure detail lines rendered beneath the bundle
/// status header. The header's `startup_failure_count` still carries the true
/// total, so this only bounds how much of the picker the detail can consume.
const STARTUP_FAILURE_PICKER_MAX_LINES: usize = 6;

/// Renders the unified bundle+session picker. The bundle column (left) drives
/// active-bundle switching; the session column (right) lists the active
/// bundle's recipients. A column-scoped filter narrows whichever column has
/// focus. Both `F2` (session focus) and `F5` (bundle focus) open this overlay.
pub(in crate::tui::render) fn render_picker_overlay(frame: &mut Frame, state: &mut AppState) {
    let popup = centered_rect(72, 72, frame.area());
    frame.render_widget(Clear, popup);
    let block = Block::default().borders(Borders::ALL).title("Picker");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let status_lines = bundle_status_lines(state.bundle_status.as_ref());
    let status_height = u16::try_from(status_lines.len()).unwrap_or(1).max(1);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(status_height),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    frame.render_widget(Paragraph::new(status_lines), sections[0]);
    frame.render_widget(Paragraph::new(picker_filter_line(state)), sections[1]);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(sections[2]);

    render_bundle_column(frame, columns[0], state);
    render_session_column(frame, columns[1], state);

    frame.render_widget(Paragraph::new(picker_hint_line(state.mode)), sections[3]);
}

fn render_bundle_column(frame: &mut Frame, area: ratatui::layout::Rect, state: &mut AppState) {
    let focused = state.picker_focus == PickerColumn::Bundles;
    let block = column_block("Bundles", focused);
    let items = if state.available_bundles.is_empty() {
        vec![ListItem::new("(no bundles configured)")]
    } else {
        state
            .visible_bundle_indices()
            .into_iter()
            .filter_map(|index| state.available_bundles.get(index))
            .map(|name| {
                let is_active = name == &state.namespace;
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
    let list = List::new(items)
        .block(block)
        .highlight_style(column_highlight_style(focused));
    frame.render_stateful_widget(list, area, &mut state.picker_bundle_state);
}

fn render_session_column(frame: &mut Frame, area: ratatui::layout::Rect, state: &mut AppState) {
    let focused = state.picker_focus == PickerColumn::Sessions;
    let block = column_block("Sessions", focused);
    let items = if state.recipients.is_empty() {
        vec![ListItem::new("(no recipients)")]
    } else {
        state
            .visible_session_indices()
            .into_iter()
            .filter_map(|index| state.recipients.get(index))
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
    let list = List::new(items)
        .block(block)
        .highlight_style(column_highlight_style(focused));
    frame.render_stateful_widget(list, area, &mut state.picker_session_state);
}

fn column_block(title: &str, focused: bool) -> Block<'static> {
    let (marker, style) = if focused {
        (
            "▶ ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        ("  ", Style::default().fg(Color::DarkGray))
    };
    Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(format!("{marker}{title}"), style))
}

fn column_highlight_style(focused: bool) -> Style {
    if focused {
        Style::default().bg(Color::Blue).fg(Color::White)
    } else {
        Style::default().bg(Color::DarkGray)
    }
}

fn picker_filter_line(state: &AppState) -> Line<'static> {
    let scope = match state.picker_focus {
        PickerColumn::Bundles => "bundles",
        PickerColumn::Sessions => "sessions",
    };
    Line::from(vec![
        Span::styled(
            format!("filter ({scope}): "),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(state.picker_filter.clone()),
        Span::styled("▌", Style::default().fg(Color::Yellow)),
    ])
}

/// Builds the one-line keybinding hint strip. The `Enter` action is
/// context-sensitive on the session column: Communication inserts into the To
/// field, Interaction opens the look+raww view.
fn picker_hint_line(mode: ScreenMode) -> Line<'static> {
    let session_action = match mode {
        ScreenMode::Communication => "session→To",
        ScreenMode::Interaction => "session→look",
    };
    Line::from(vec![
        picker_hint_key("Tab"),
        Span::raw(" Bundles/Sessions"),
        Span::raw("   "),
        picker_hint_key("Enter"),
        Span::raw(format!(" bundle→switch / {session_action}")),
        Span::raw("   "),
        picker_hint_key("Esc"),
        Span::raw(" Close"),
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

/// Builds the bundle status block: the k=v header line styled by severity,
/// followed by one detail line per recent per-session startup failure (capped
/// by [`STARTUP_FAILURE_PICKER_MAX_LINES`]). The failures carry the real cause
/// each session failed with, so the operator sees them here rather than relying
/// on the generic error path.
fn bundle_status_lines(status: Option<&BundleStatusDisplay>) -> Vec<Line<'static>> {
    let Some(status) = status else {
        return vec![Line::from(Span::styled(
            "(bundle status pending list refresh)",
            Style::default().fg(Color::DarkGray),
        ))];
    };
    let mut lines = vec![Line::from(Span::styled(
        format_bundle_status_line(status),
        bundle_status_severity_style(bundle_status_severity(status)),
    ))];
    for failure in format_startup_failure_lines(status)
        .into_iter()
        .take(STARTUP_FAILURE_PICKER_MAX_LINES)
    {
        lines.push(Line::from(Span::styled(
            failure,
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
    }
    lines
}

fn bundle_status_severity_style(severity: BundleStatusSeverity) -> Style {
    match severity {
        BundleStatusSeverity::Healthy => Style::default().fg(Color::Green),
        BundleStatusSeverity::Degraded => Style::default().fg(Color::Yellow),
        BundleStatusSeverity::HostedDown => {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        }
        BundleStatusSeverity::Unhosted => Style::default().fg(Color::DarkGray),
    }
}
