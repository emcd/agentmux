use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use serde_json::Value;

use crate::acp::AcpSnapshotEntry;

use super::super::state::AppState;
use super::frame::INTERACTION_RAWW_PANE_HEIGHT;
use super::geometry::{raww_titled_block, wrap_text};

pub(super) fn render_interaction_mode(frame: &mut Frame, area: Rect, state: &mut AppState) {
    let raww_visible = state.interaction_raww_region_visible();
    let region_height = if raww_visible {
        INTERACTION_RAWW_PANE_HEIGHT
    } else {
        interaction_choice_pane_height(area.height)
    };
    let region_height = region_height.min(area.height.saturating_sub(2).max(1));

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(super::frame::INTERACTION_TARGET_HEADER_HEIGHT),
            Constraint::Min(3),
            Constraint::Length(region_height),
        ])
        .split(area);

    render_interaction_target_header(frame, rows[0], state);
    render_look_snapshot(frame, rows[1], state);
    if raww_visible {
        render_interaction_raww_pane(frame, rows[2], state);
    } else {
        render_look_choice_section(frame, rows[2], state);
    }
}

fn render_interaction_target_header(frame: &mut Frame, area: Rect, state: &AppState) {
    let label = match state.look_target.as_deref() {
        Some(target) => format!("  Interaction target: {target}  "),
        None => "  Interaction target: (none) — press F2 to choose a session  ".to_string(),
    };
    let paragraph = Paragraph::new(Line::from(Span::styled(
        label,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
    frame.render_widget(paragraph, area);
}

fn render_interaction_raww_pane(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = raww_titled_block("  Write  ");
    let inner = block.inner(area);
    let lines: Vec<Line<'static>> = if state.raww_draft.is_empty() {
        vec![Line::from(Span::styled(
            "(type to compose write; Enter dispatches, Ctrl+J inserts newline)",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        state
            .raww_draft
            .split('\n')
            .map(|line| Line::from(Span::raw(line.to_string())))
            .collect()
    };
    let viewport_height = inner.height as usize;
    let visible = if lines.len() > viewport_height {
        lines[lines.len().saturating_sub(viewport_height)..].to_vec()
    } else {
        lines
    };
    let paragraph = Paragraph::new(visible).block(block);
    frame.render_widget(paragraph, area);
}

fn render_look_snapshot(frame: &mut Frame, area: Rect, state: &AppState) {
    let base_title = match (&state.look_target, &state.look_captured_at) {
        (Some(target), Some(captured_at)) => {
            format!(
                "Look Snapshot target={} captured_at={}",
                target, captured_at
            )
        }
        (Some(target), None) => format!("Look Snapshot target={}", target),
        _ => "Look Snapshot".to_string(),
    };
    let content_width = area.width.saturating_sub(2) as usize;
    let all_lines = match state.look_snapshot_format {
        Some(super::super::state::LookSnapshotFormat::AcpEntriesV1) => {
            let rendered =
                render_acp_snapshot_entries(state.look_snapshot_entries.as_slice(), content_width);
            if rendered.is_empty() {
                vec![Line::from("(no snapshot captured)")]
            } else {
                rendered
            }
        }
        _ => {
            if state.look_snapshot_lines.is_empty() {
                vec![Line::from("(no snapshot captured)")]
            } else {
                let mut lines = Vec::<Line>::new();
                for line in &state.look_snapshot_lines {
                    for wrapped in wrap_text(line, content_width.max(1)) {
                        lines.push(Line::from(Span::raw(wrapped)));
                    }
                }
                lines
            }
        }
    };
    let viewport_height = area.height.saturating_sub(2) as usize;
    let effective_scroll = if all_lines.is_empty() {
        0
    } else {
        state
            .look_overlay_scroll
            .min(all_lines.len().saturating_sub(1))
    };
    let end = all_lines.len().saturating_sub(effective_scroll);
    let start = end.saturating_sub(viewport_height);
    let visible_lines = all_lines[start..end].to_vec();
    let title = if effective_scroll == 0 {
        base_title
    } else {
        format!("{base_title} (scroll {effective_scroll})")
    };
    let paragraph = Paragraph::new(visible_lines)
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(paragraph, area);
}

fn render_look_choice_section(frame: &mut Frame, area: Rect, state: &AppState) {
    let inner_width = area.width.saturating_sub(2);
    let lines = render_look_choice_lines(state, inner_width);
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default().borders(Borders::ALL).title(
            "  Session Choices (Left/Right request, Up/Down option, Enter select, c cancel)  ",
        ),
    );
    frame.render_widget(paragraph, area);
}

fn render_acp_snapshot_entries(entries: &[AcpSnapshotEntry], width: usize) -> Vec<Line<'static>> {
    let mut rendered = Vec::<Line<'static>>::new();
    for entry in entries {
        match entry {
            AcpSnapshotEntry::User { lines } => {
                push_labeled_lines(&mut rendered, "user", Color::Green, lines, width);
            }
            AcpSnapshotEntry::Agent { lines } => {
                push_labeled_lines(&mut rendered, "agent", Color::Cyan, lines, width);
            }
            AcpSnapshotEntry::Cognition { lines } => {
                push_labeled_lines(&mut rendered, "cognition", Color::Yellow, lines, width);
            }
            AcpSnapshotEntry::Invocation {
                call_id,
                status,
                invocation,
                result,
            } => {
                let status_label = format!("{:?}", status);
                let label = format!("tool_call {} [{}]", call_id, status_label);
                push_labeled_json(&mut rendered, &label, Color::Magenta, invocation, width);
                if let Some(result) = result {
                    push_labeled_json(&mut rendered, "result", Color::Blue, result, width);
                }
            }
            AcpSnapshotEntry::Update { update_kind, lines } => {
                let mut update_lines = vec![format!("kind: {update_kind}")];
                update_lines.extend(lines.iter().cloned());
                push_labeled_lines(&mut rendered, "update", Color::White, &update_lines, width);
            }
        }
    }
    rendered
}

fn push_labeled_json(
    rendered: &mut Vec<Line<'static>>,
    label: &str,
    color: Color,
    value: &Value,
    width: usize,
) {
    let payload = serde_json::to_string_pretty(value)
        .unwrap_or_else(|_| serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string()));
    let lines = payload.lines().map(ToString::to_string).collect::<Vec<_>>();
    push_labeled_lines(rendered, label, color, lines.as_slice(), width);
}

fn push_labeled_lines(
    rendered: &mut Vec<Line<'static>>,
    label: &str,
    color: Color,
    lines: &[String],
    width: usize,
) {
    let label_style = Style::default().fg(color).add_modifier(Modifier::BOLD);
    rendered.push(Line::from(Span::styled(format!("[{label}]"), label_style)));
    let body_style = Style::default().fg(Color::White);
    let body_width = width.saturating_sub(2).max(1);
    if lines.is_empty() {
        rendered.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("(empty)", body_style),
        ]));
        rendered.push(Line::raw(""));
        return;
    }
    for line in lines {
        for wrapped in wrap_text(line, body_width) {
            rendered.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(wrapped, body_style),
            ]));
        }
    }
    rendered.push(Line::raw(""));
}

fn render_look_choice_lines(state: &AppState, width: u16) -> Vec<Line<'static>> {
    let pending = state.look_pending_choices();
    if pending.is_empty() {
        return vec![
            Line::from("(no pending choice requests for this session)"),
            Line::from("Press F2 to choose another session, or F4 for Communication."),
        ];
    }

    let request_index = state
        .look_choice_request_index
        .min(pending.len().saturating_sub(1));
    let request = pending[request_index];
    let options = request.options.as_slice();
    let option_index = state
        .look_choice_option_index
        .min(options.len().saturating_sub(1));
    let mut lines = vec![Line::from(Span::styled(
        format!(
            "Request {}/{}: {}",
            request_index + 1,
            pending.len(),
            request.choice_request_id
        ),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    lines.push(Line::from(format!(
        "target={} kind={} enqueued={}",
        request.target_session.as_deref().unwrap_or("-"),
        request.requested_kind.as_deref().unwrap_or("-"),
        request.enqueued_at.as_deref().unwrap_or("-"),
    )));

    if options.is_empty() {
        lines.push(Line::from(
            "No ACP options available; use c to resolve cancelled.",
        ));
        return lines;
    }

    lines.push(Line::from("Options:"));
    let body_width = width.saturating_sub(4).max(1) as usize;
    for (index, option) in options.iter().enumerate() {
        let marker = if index == option_index { ">" } else { " " };
        let mut descriptor = format!(
            "{marker} {}  id={}",
            option.name.as_deref().unwrap_or("(unnamed option)"),
            option.option_id
        );
        if let Some(kind) = option.kind.as_deref() {
            descriptor.push_str(format!("  kind={kind}").as_str());
        }
        for wrapped in wrap_text(descriptor.as_str(), body_width) {
            lines.push(Line::from(wrapped));
        }
    }
    lines
}

fn interaction_choice_pane_height(available_height: u16) -> u16 {
    12u16.min(available_height.saturating_sub(3).max(1))
}
