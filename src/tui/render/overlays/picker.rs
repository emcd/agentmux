use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};

use super::super::super::actions;
use super::super::super::state::{AppState, PickerColumn};
use super::super::super::status::{
    BundleStatusDisplay, BundleStatusSeverity, RecipientReadiness, bundle_status_severity,
    format_bundle_status_line, format_recipient_picker_label, format_startup_failure_lines,
};
use super::super::geometry::centered_rect;

/// Cap on per-session startup failure detail lines rendered beneath the bundle
/// status header. The header's `startup_failure_count` still carries the true
/// total, so this only bounds how much of the picker the detail can consume.
const STARTUP_FAILURE_PICKER_MAX_LINES: usize = 6;

/// Space between two bindings on a hint row.
const HINT_GAP: &str = "   ";

/// Renders the unified bundle+session picker. The bundle column (left) drives
/// active-bundle switching; the session column (right) lists the active
/// bundle's recipients. A column-scoped filter narrows whichever column has
/// focus. Two entry points open this overlay, one focused on each column.
pub(in crate::tui::render) fn render_picker_overlay(frame: &mut Frame, state: &mut AppState) {
    let popup = centered_rect(72, 72, frame.area());
    frame.render_widget(Clear, popup);
    let block = Block::default().borders(Borders::ALL).title("Picker");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let status_lines = bundle_status_lines(state.bundle_status.as_ref());
    let status_height = u16::try_from(status_lines.len()).unwrap_or(1).max(1);
    // The hint is laid out before the vertical split so its row count can size
    // its own section: generated wording needs more than the single row the
    // hand-written strip fitted in, and how many more depends on the width.
    //
    // Every packed row is reserved. A cap here would clip whichever binding
    // landed last, silently, and the session list below is what should give up
    // the space -- its `Min(1)` already says so.
    let hint_lines = picker_hint_lines(inner.width);
    let hint_height = u16::try_from(hint_lines.len()).unwrap_or(1).max(1);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(status_height),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(hint_height),
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

    // Reserved height and produced height agree above, so this takes every row
    // in practice. It is written as a bound rather than assumed because the
    // solver can still shrink the section on a terminal too short to satisfy
    // every constraint, and writing rows into a rect that cannot hold them is
    // the defect this replaced: the strip lost a whole binding with nothing to
    // show for it.
    let visible = hint_lines.len().min(usize::from(sections[3].height));
    frame.render_widget(Paragraph::new(hint_lines[..visible].to_vec()), sections[3]);
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

/// Builds the one-line keybinding hint strip from the binding table.
///
/// The strip advertises a chosen few of the picker's bindings; which few is
/// declared in `actions::picker_hint`, and their chords and wording come from
/// the table rather than from labels kept in step by hand. It stays filtered to
/// the picker's own contexts, which is the asymmetry with the help overlay:
/// help catalogues every surface, a strip annotates the one it sits on.
///
/// The mode-sensitive session label this replaced is gone. Committing a session
/// inserts into `To` or opens the look target by mode, and the table says so in
/// one description rather than the strip choosing a phrasing per mode.
fn picker_hint_lines(width: u16) -> Vec<Line<'static>> {
    // The scope qualifier is kept here, unlike the write pane's hint. This
    // strip spans both columns, so "Bundle col" and "Session col" are what
    // separate two entries that would otherwise both read as `Enter`.
    //
    // Entries are packed into as many rows as they need rather than truncated
    // at one. Generated wording is longer than the shorthand it replaces, and
    // a strip that silently loses its last binding is worse than one that
    // takes a second row. Breaking between entries, not inside them, is why
    // this packs by hand instead of wrapping the finished text.
    let width = usize::from(width).max(1);
    let mut lines = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for entry in actions::picker_hint() {
        let chord = entry.primary_chord().to_string();
        // The qualified description is preferred, because it is what separates
        // the two entries that both read as `Enter`. Where it cannot fit a row
        // at all, the unqualified one is used instead: an entry that no longer
        // says which column is worse than one clipped mid-word, and splitting
        // an entry across rows is not on offer.
        let qualified = format!(" {}", entry.description);
        let text = if chord.chars().count() + qualified.chars().count() > width {
            format!(" {}", entry.detail())
        } else {
            qualified
        };
        let cost = chord.chars().count() + text.chars().count();
        if !spans.is_empty() && used + HINT_GAP.len() + cost > width {
            lines.push(Line::from(std::mem::take(&mut spans)));
            used = 0;
        }
        if !spans.is_empty() {
            spans.push(Span::raw(HINT_GAP));
            used += HINT_GAP.len();
        }
        spans.push(picker_hint_key(&chord));
        spans.push(Span::raw(text));
        used += cost;
    }
    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }
    lines
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

// Inline by exception, under the project's three conditions. The picker
// renderer is crate-private by design and no public interface reaches it;
// making it externally testable would mean a render-to-buffer method on
// `Workbench` existing only for this test. One `#[test]` function, as the
// policy caps it.
//
// It earns the exception by covering what a test of the packing alone cannot:
// that the rows the strip reserves match the rows it produces. The defect this
// pins was a mismatch between those two numbers, not a packing error.
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    use crate::tui::state::TuiLaunchOptions;

    fn rendered(width: u16, height: u16) -> String {
        let mut state = AppState::new(TuiLaunchOptions {
            namespace: "agentmux".to_string(),
            sender_session: "tui".to_string(),
            relay_socket: std::path::PathBuf::from("/tmp/agentmux-picker-render.sock"),
            look_lines: None,
            available_bundles: vec!["agentmux".to_string()],
        });
        state.open_picker();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| render_picker_overlay(frame, &mut state))
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

    #[test]
    fn every_advertised_binding_survives_at_widths_that_need_extra_rows() {
        // 70 columns is the width at which the four entries pack into four
        // whole rows, one more than the strip used to reserve. The wider and
        // narrower cases bracket it so a change to the packing shows up as a
        // failure here rather than as a strip quietly losing its last binding.
        for (width, height) in [(120, 44), (90, 30), (70, 24), (60, 20), (50, 16)] {
            let text = rendered(width, height);
            for entry in actions::picker_hint() {
                // Either wording is acceptable; losing the entry is not. The
                // unqualified form is the documented degradation where the
                // qualified one cannot fit a row.
                let qualified = format!("{} {}", entry.primary_chord(), entry.description);
                let plain = format!("{} {}", entry.primary_chord(), entry.detail());
                assert!(
                    text.contains(&qualified) || text.contains(&plain),
                    "{qualified:?} is missing at {width}x{height}:\n{text}"
                );
            }
        }
    }
}
