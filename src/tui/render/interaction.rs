use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use serde_json::Value;

use crate::transports::StructuredEntry;

use super::super::actions::{self, Action, BindingContext};
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

/// The chord this pane's own context binds to a behavior, for prose that tells
/// the operator to press it.
///
/// `None` where the context binds nothing to it, which callers turn into prose
/// naming the behavior instead. Naming a chord the table does not declare is
/// the transcription this module no longer does, and a prompt to press nothing
/// is worse than one that says what to reach for.
fn pane_chord(state: &AppState, context: BindingContext, action: Action) -> Option<String> {
    actions::binding_for(&state.bindings, context, action)
        .map(|entry| entry.primary_chord().to_string())
}

fn render_interaction_target_header(frame: &mut Frame, area: Rect, state: &AppState) {
    let label = match state.look_target.as_deref() {
        Some(target) => format!("  Interaction target: {target}  "),
        // With no target there are no pending choices, so the write pane is the
        // one this header sits above and the one whose binding it quotes.
        None => match pane_chord(state, BindingContext::InteractionWrite, Action::OpenPicker) {
            Some(chord) => {
                format!("  Interaction target: (none) — press {chord} to choose a session  ")
            }
            None => {
                "  Interaction target: (none) — open the picker to choose a session  ".to_string()
            }
        },
    };
    let paragraph = Paragraph::new(Line::from(Span::styled(
        label,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
    frame.render_widget(paragraph, area);
}

/// The empty write pane's prompt, generated from the binding table.
///
/// Filtered to the write pane's own context, which is the asymmetry with the
/// help overlay: help catalogues every surface, a hint annotates the one it
/// sits on. Which behaviors are worth advertising is declared in
/// `actions::interaction_write_hint`; their chords and wording are the table's.
fn write_pane_hint(state: &AppState) -> String {
    // The scope qualifier is dropped: every one of these is a write-pane
    // binding and the pane it is printed in has already said so.
    let advertised = actions::interaction_write_hint(&state.bindings)
        .into_iter()
        .map(|entry| {
            format!(
                "{} {}",
                entry.primary_chord(),
                entry.detail().to_lowercase()
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!("({advertised})")
}

fn render_interaction_raww_pane(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = raww_titled_block("  Write  ");
    let inner = block.inner(area);
    let lines: Vec<Line<'static>> = if state.raww_draft.is_empty() {
        // Wrapped rather than truncated: the generated wording is longer than
        // the sentence it replaces, and a prompt that loses its last binding
        // to the pane edge is worse than one that takes a second row.
        wrap_text(&write_pane_hint(state), inner.width as usize)
            .into_iter()
            .map(|line| Line::from(Span::styled(line, Style::default().fg(Color::DarkGray))))
            .collect()
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
        Some(super::super::state::LookSnapshotFormat::StructuredEntriesV1) => {
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

const CHOICE_PANE_TITLE: &str = "  Session Choices  ";

/// The choice pane's title, its advertised bindings generated from the table.
///
/// Filtered to the choice pane's own context, the same asymmetry with the help
/// overlay the write pane's hint follows. Which behaviors are worth a title is
/// declared in `actions::interaction_choice_hint`; their chords and wording are
/// the table's.
///
/// A block title does not wrap: what does not fit is cut where the border runs
/// out, mid-word. So the bindings drop whole rather than partially, the way the
/// picker strip drops a qualifier rather than splitting an entry. The
/// hand-written title this replaces was cut the same way and said nothing about
/// it; generated wording is only the reason the width came up.
fn choice_pane_title(state: &AppState, width: u16) -> String {
    // The scope qualifier is dropped: every one of these is a choice-pane
    // binding and the pane it is printed on has already said so.
    let advertised = actions::interaction_choice_hint(&state.bindings)
        .into_iter()
        .map(|entry| {
            format!(
                "{} {}",
                entry.primary_chord(),
                entry.detail().to_lowercase()
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    if advertised.is_empty() {
        return CHOICE_PANE_TITLE.to_string();
    }
    let full = format!("  Session Choices ({advertised})  ");
    if full.chars().count() <= usize::from(width) {
        full
    } else {
        CHOICE_PANE_TITLE.to_string()
    }
}

fn render_look_choice_section(frame: &mut Frame, area: Rect, state: &AppState) {
    let inner_width = area.width.saturating_sub(2);
    let lines = render_look_choice_lines(state, inner_width);
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default()
            .borders(Borders::ALL)
            .title(choice_pane_title(state, inner_width)),
    );
    frame.render_widget(paragraph, area);
}

fn render_acp_snapshot_entries(entries: &[StructuredEntry], width: usize) -> Vec<Line<'static>> {
    let mut rendered = Vec::<Line<'static>>::new();
    for entry in entries {
        match entry {
            StructuredEntry::User { lines } => {
                push_labeled_lines(&mut rendered, "user", Color::Green, lines, width);
            }
            StructuredEntry::Agent { lines } => {
                push_labeled_lines(&mut rendered, "agent", Color::Cyan, lines, width);
            }
            StructuredEntry::Cognition { lines } => {
                push_labeled_lines(&mut rendered, "cognition", Color::Yellow, lines, width);
            }
            StructuredEntry::Invocation {
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
            StructuredEntry::Update { update_kind, lines } => {
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

/// The only way out of a request that arrived with nothing to choose from.
///
/// The pane's title advertises this behavior too, but the title is dropped
/// whole at narrow widths, and this is the one place the operator is told that
/// cancelling is not merely available but the only move.
fn no_options_prompt(state: &AppState) -> String {
    match pane_chord(
        state,
        BindingContext::InteractionChoice,
        Action::ResolveChoiceCancelled,
    ) {
        Some(chord) => format!("No ACP options available; use {chord} to resolve cancelled."),
        None => "No ACP options available; this request resolves only as cancelled.".to_string(),
    }
}

/// Where the operator goes when a session has nothing to decide, generated from
/// the choice pane's own bindings.
fn empty_choice_prompt(state: &AppState) -> String {
    let choose = pane_chord(state, BindingContext::InteractionChoice, Action::OpenPicker)
        .map_or_else(
            || "Open the picker".to_string(),
            |chord| format!("Press {chord}"),
        );
    let switch = pane_chord(state, BindingContext::InteractionChoice, Action::ToggleMode)
        .map_or_else(
            || "switch modes".to_string(),
            |chord| format!("press {chord}"),
        );
    format!("{choose} to choose another session, or {switch} for Communication.")
}

fn render_look_choice_lines(state: &AppState, width: u16) -> Vec<Line<'static>> {
    let pending = state.look_pending_choices();
    if pending.is_empty() {
        return vec![
            Line::from("(no pending choice requests for this session)"),
            Line::from(empty_choice_prompt(state)),
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
        lines.push(Line::from(no_options_prompt(state)));
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    use crate::tui::state::{PendingChoiceEntry, TuiLaunchOptions};

    fn workbench() -> AppState {
        AppState::new(TuiLaunchOptions {
            namespace: "agentmux".to_string(),
            sender_session: "tui".to_string(),
            relay_socket: std::path::PathBuf::from("/tmp/agentmux-choice-render.sock"),
            look_lines: None,
            available_bundles: vec!["agentmux".to_string()],
            bindings: None,
        })
    }

    /// A target holding one request that arrived with nothing to choose from,
    /// which is the only path to the cancel-only prompt.
    fn awaiting_an_optionless_request() -> AppState {
        let mut state = workbench();
        state.look_target = Some("peer@agentmux".to_string());
        state.pending_choices = vec![PendingChoiceEntry {
            choice_request_id: "request-1".to_string(),
            message_id: None,
            target_session: Some("peer@agentmux".to_string()),
            requested_kind: Some("permission".to_string()),
            requested_details: None,
            enqueued_at: None,
            options: Vec::new(),
        }];
        state
    }

    fn draw(state: &AppState, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| render_look_choice_section(frame, frame.area(), state))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|row| {
                (0..buffer.area.width)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The pane's text with its wrapping undone: borders dropped and runs of
    /// whitespace collapsed onto one space.
    ///
    /// Body lines wrap at word boundaries, so a phrase in them can break
    /// anywhere. Searching the raw buffer would make an assertion about the
    /// prompt into an assertion about where the terminal happened to be
    /// narrow. A title is on a border row and does not wrap, so a cut one is
    /// still a fragment after this.
    fn flowed(text: &str) -> String {
        text.chars()
            .map(|character| {
                if "│┌┐└┘─".contains(character) {
                    ' '
                } else {
                    character
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn the_choice_pane_shows_a_whole_advertised_binding_or_none_of_it() {
        // A block title does not wrap, so the failure this guards is a title cut
        // mid-word rather than one that overflows visibly. Comparing the
        // rendered title against what `choice_pane_title` returns would only ask
        // the function to agree with itself; this asks whether any *start* of an
        // advertised binding reached the buffer without the rest of it.
        let idle = workbench();
        let advertised: Vec<String> = actions::interaction_choice_hint(&idle.bindings)
            .into_iter()
            .map(|entry| {
                format!(
                    "{} {}",
                    entry.primary_chord(),
                    entry.detail().to_lowercase()
                )
            })
            .collect();
        assert!(!advertised.is_empty(), "the choice pane advertises nothing");

        for (width, height) in [(120, 20), (90, 18), (80, 16), (70, 14), (60, 12), (50, 10)] {
            let text = flowed(&draw(&idle, width, height));
            for phrase in &advertised {
                let start: String = phrase.chars().take(8).collect();
                assert_eq!(
                    text.contains(phrase.as_str()),
                    text.contains(start.as_str()),
                    "the title shows the start of {phrase:?} without the whole of it \
                     at {width}x{height}:\n{text}"
                );
            }

            // The prompt names the chord the table declares for this context,
            // not one written here. Without this the pane could advertise
            // nothing at every width and the equality above would still hold.
            let opens_picker = actions::binding_for(
                &idle.bindings,
                BindingContext::InteractionChoice,
                Action::OpenPicker,
            )
            .expect("the choice pane binds opening the picker");
            assert!(
                text.contains(opens_picker.primary_chord()),
                "the empty-choices prompt does not name {:?} at {width}x{height}:\n{text}",
                opens_picker.primary_chord()
            );
        }

        // And the widest case really does carry them, or the equality above
        // would be satisfied everywhere by a title that advertises nothing.
        let wide = flowed(&draw(&idle, 120, 20));
        for phrase in &advertised {
            assert!(
                wide.contains(phrase.as_str()),
                "{phrase:?} is missing from the widest rendering:\n{wide}"
            );
        }

        // A request that arrived with no options is the one case where the pane
        // tells the operator that cancelling is the only move. The title drops
        // its bindings whole at narrow widths, so this line is what carries the
        // chord there, and it must be the chord the table declares.
        let optionless = awaiting_an_optionless_request();
        let cancels = actions::binding_for(
            &optionless.bindings,
            BindingContext::InteractionChoice,
            Action::ResolveChoiceCancelled,
        )
        .expect("the choice pane binds resolving as cancelled");
        for (width, height) in [(120, 20), (70, 14), (50, 10)] {
            let text = flowed(&draw(&optionless, width, height));
            assert!(
                text.contains(&format!(
                    "use {} to resolve cancelled",
                    cancels.primary_chord()
                )),
                "the cancel-only prompt does not name {:?} at {width}x{height}:\n{text}",
                cancels.primary_chord()
            );
        }
    }
}
