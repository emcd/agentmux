use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};
use serde_json::Value;

use crate::acp::AcpSnapshotEntry;

use super::state::{
    AppState, ChatHistoryDirection, FocusField, LookSnapshotFormat, Recipient, ScreenMode,
    StatusEntry,
};
use super::status::{
    BundleStatusDisplay, BundleStatusSeverity, RecipientReadiness, bundle_status_severity,
    format_bundle_status_line, format_recipient_picker_label,
};

const WORKBENCH_MIN_CHAT_HEIGHT: u16 = 1;
const WORKBENCH_MIN_COMPOSE_HEIGHT: u16 = 4;
const INTERACTION_RAWW_PANE_HEIGHT: u16 = 8;
const INTERACTION_TARGET_HEADER_HEIGHT: u16 = 1;

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
    render_active_cursor(frame, chunks[1], state);

    if state.help_overlay_open {
        render_help_overlay(frame, state);
    }
    if state.picker_open {
        render_picker_overlay(frame, state);
    }
    if state.events_overlay_open {
        render_events_overlay(frame, state);
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
    match state.mode {
        ScreenMode::Communication => render_communication_mode(frame, area, state),
        ScreenMode::Interaction => render_interaction_mode(frame, area, state),
    }
}

fn render_active_cursor(frame: &mut Frame, area: Rect, state: &AppState) {
    if state.help_overlay_open || state.picker_open || state.events_overlay_open {
        return;
    }
    match state.mode {
        ScreenMode::Communication => render_compose_cursor(frame, area, state),
        ScreenMode::Interaction => render_raww_cursor(frame, area, state),
    }
}

fn render_compose_cursor(frame: &mut Frame, area: Rect, state: &AppState) {
    let rows = split_workbench_rows(area, state);
    let compose_inner = compose_titled_block("  Compose  ").inner(rows[1]);
    let Some((x, y)) = compose_cursor_position(compose_inner, state) else {
        return;
    };
    frame.set_cursor_position((x, y));
}

fn render_raww_cursor(frame: &mut Frame, area: Rect, state: &AppState) {
    if !state.interaction_raww_region_visible() {
        return;
    }
    let raww_area = interaction_raww_pane_area(area);
    let inner = raww_titled_block("  Write  ").inner(raww_area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let (cursor_line, cursor_column) = state.raww_cursor_line_and_column();
    let visible_x = (cursor_column as u16).min(inner.width.saturating_sub(1));
    let visible_y = (cursor_line as u16).min(inner.height.saturating_sub(1));
    frame.set_cursor_position((
        inner.x.saturating_add(visible_x),
        inner.y.saturating_add(visible_y),
    ));
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

fn render_communication_mode(frame: &mut Frame, area: Rect, state: &mut AppState) {
    let rows = split_workbench_rows(area, state);
    render_chat_history(frame, rows[0], state);
    render_compose(frame, rows[1], state);
}

fn render_interaction_mode(frame: &mut Frame, area: Rect, state: &mut AppState) {
    let raww_visible = state.interaction_raww_region_visible();
    let region_height = if raww_visible {
        INTERACTION_RAWW_PANE_HEIGHT
    } else {
        interaction_permission_pane_height(area.height)
    };
    let region_height = region_height.min(area.height.saturating_sub(2).max(1));

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(INTERACTION_TARGET_HEADER_HEIGHT),
            Constraint::Min(3),
            Constraint::Length(region_height),
        ])
        .split(area);

    render_interaction_target_header(frame, rows[0], state);
    render_look_snapshot(frame, rows[1], state);
    if raww_visible {
        render_interaction_raww_pane(frame, rows[2], state);
    } else {
        render_look_permission_section(frame, rows[2], state);
    }
}

fn interaction_permission_pane_height(available_height: u16) -> u16 {
    12u16.min(available_height.saturating_sub(3).max(1))
}

fn interaction_raww_pane_area(area: Rect) -> Rect {
    let raww_height = INTERACTION_RAWW_PANE_HEIGHT.min(area.height.saturating_sub(2).max(1));
    let raww_y = area
        .y
        .saturating_add(area.height)
        .saturating_sub(raww_height);
    Rect {
        x: area.x,
        y: raww_y,
        width: area.width,
        height: raww_height,
    }
}

fn render_interaction_target_header(frame: &mut Frame, area: Rect, state: &AppState) {
    let label = match state.look_target.as_deref() {
        Some(target) => format!("  Interaction target: {target}  "),
        None => {
            "  Interaction target: (none) — press F2 to choose a session, then l or w  ".to_string()
        }
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

fn raww_titled_block(title: &'static str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_alignment(Alignment::Center)
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
        Some(LookSnapshotFormat::AcpEntriesV1) => {
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

fn render_look_permission_section(frame: &mut Frame, area: Rect, state: &AppState) {
    let inner_width = area.width.saturating_sub(2);
    let lines = render_look_permission_lines(state, inner_width);
    let paragraph =
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(
            "  Session Permissions (Left/Right request, Up/Down option, Enter select, c cancel)  ",
        ));
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

fn render_look_permission_lines(state: &AppState, width: u16) -> Vec<Line<'static>> {
    let pending = state.look_pending_permissions();
    if pending.is_empty() {
        return vec![
            Line::from("(no pending permission requests for this session)"),
            Line::from("Press F2 to choose another session, or F4 for Communication."),
        ];
    }

    let request_index = state
        .look_permission_request_index
        .min(pending.len().saturating_sub(1));
    let request = pending[request_index];
    let options = request.options.as_slice();
    let option_index = state
        .look_permission_option_index
        .min(options.len().saturating_sub(1));
    let mut lines = vec![Line::from(Span::styled(
        format!(
            "Request {}/{}: {}",
            request_index + 1,
            pending.len(),
            request.permission_request_id
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

fn wrap_text(text: &str, width: usize) -> Vec<String> {
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

fn render_footer(frame: &mut Frame, area: Rect, state: &AppState) {
    let (mode_label, toggle_hint) = match state.mode {
        ScreenMode::Communication => ("[Communication]", "F4 → Interaction"),
        ScreenMode::Interaction => ("[Interaction]", "F4 → Communication"),
    };
    let status_line = state
        .status_history
        .front()
        .map(render_status_line)
        .unwrap_or_else(|| Line::from("Ready."));
    let mut spans = vec![
        Span::styled(
            mode_label,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(toggle_hint, Style::default().fg(Color::Yellow)),
        Span::raw("  "),
    ];
    spans.extend(status_line.spans);
    let footer = Paragraph::new(Line::from(spans))
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
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Recipient Picker");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);

    let status_line = bundle_status_header_line(state.bundle_status.as_ref());
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
}

fn bundle_status_header_line(status: Option<&BundleStatusDisplay>) -> Line<'static> {
    let Some(status) = status else {
        return Line::from(Span::styled(
            "(bundle status pending list refresh)",
            Style::default().fg(Color::DarkGray),
        ));
    };
    let style = bundle_status_severity_style(bundle_status_severity(status));
    Line::from(Span::styled(format_bundle_status_line(status), style))
}

fn recipient_readiness_style(readiness: RecipientReadiness) -> Style {
    match readiness {
        RecipientReadiness::Ready => Style::default(),
        RecipientReadiness::NotReady => Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    }
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

fn render_events_overlay(frame: &mut Frame, state: &mut AppState) {
    let popup = centered_rect(80, 70, frame.area());
    frame.render_widget(Clear, popup);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(6)])
        .split(popup);

    let pending_items = if state.pending_permissions.is_empty() {
        vec![ListItem::new("(no pending permission requests)")]
    } else {
        state
            .pending_permissions
            .iter()
            .map(|entry| {
                let target = entry.target_session.as_deref().unwrap_or("-");
                let kind = entry.requested_kind.as_deref().unwrap_or("-");
                let enqueued_at = entry.enqueued_at.as_deref().unwrap_or("-");
                ListItem::new(format!(
                    "{} target={} kind={} enqueued={}",
                    entry.permission_request_id, target, kind, enqueued_at
                ))
            })
            .collect::<Vec<_>>()
    };
    let pending_list = List::new(pending_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Pending Permissions"),
        )
        .highlight_style(Style::default().bg(Color::Blue).fg(Color::White));
    frame.render_stateful_widget(
        pending_list,
        sections[0],
        &mut state.pending_permissions_state,
    );

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

fn help_section_heading(text: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        text,
        Style::default().add_modifier(Modifier::BOLD),
    ))
}

fn render_help_overlay(frame: &mut Frame, _state: &AppState) {
    let popup = centered_rect(72, 70, frame.area());
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
    ];
    let right_lines = vec![
        help_section_heading("Interaction Mode"),
        Line::from("PgUp/PgDn: Scroll look snapshot"),
        Line::from("Write input (write has text or no pending):"),
        Line::from("  Arrows/Home/End: Move write cursor"),
        Line::from("  Enter: Dispatch write to active target"),
        Line::from("  Ctrl+J: Insert newline"),
        Line::from("  Backspace: Backspace write input"),
        Line::from("Permission (write empty and pending exists):"),
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
