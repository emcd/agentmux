//! The help overlay.
//!
//! Bindings are generated from the binding table rather than transcribed, so a
//! row added there appears here without anyone remembering to copy it. What
//! stays hand-written is the material the table cannot hold: the mouse wheel,
//! which is not a key binding; the conditions under which the interaction
//! region shows its write pane or its choice pane, which are
//! `binding_context`'s predicate rather than a row; the `To` field's address
//! grammar; and the keyboard-capability report, which reads probe state.
//!
//! Those notes are declared beside the section they annotate and rendered
//! after its generated bindings.
//!
//! The line naming the chord that inserts a newline under every probe outcome
//! is generated here rather than carried by the report it sits under. The
//! report's subject is delivery -- how a key reaches the TUI -- and that line
//! is about what a key does, which only the table can answer.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use super::super::super::actions::{
    Action, BindingContext, HelpSection, binding_for, help_bindings,
};
use super::super::super::keyboard::format_keyboard_enhancement_lines;
use super::super::super::state::AppState;
use super::super::geometry::centered_rect;

pub(in crate::tui::render) fn render_help_overlay(frame: &mut Frame, state: &AppState) {
    let popup = centered_rect(96, 92, frame.area());
    frame.render_widget(Clear, popup);
    let block = Block::default().borders(Borders::ALL).title("Help");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let columns = help_columns(inner);

    // Three columns rather than the two the hand-written overlay used. The
    // generated bindings are one line per behavior where the transcript
    // combined directions onto a line ("Arrows/Home/End: Move cursor"), so the
    // same content is taller; in two columns it overflowed at terminal sizes
    // the old overlay fitted, and the keyboard-capability report was the part
    // pushed off the bottom.
    let mut binding_columns: [Vec<Line<'static>>; 2] = [Vec::new(), Vec::new()];
    for section in &help_bindings() {
        let column = match section.heading {
            "Modes" | "Communication Mode" => 0,
            _ => 1,
        };
        push_section(&mut binding_columns[column], section);
    }

    let mut reference_lines = vec![help_section_heading("To Field Grammar")];
    for note in TO_FIELD_GRAMMAR {
        reference_lines.push(Line::from(*note));
    }
    reference_lines.push(Line::from(""));
    reference_lines.push(help_section_heading("Modified Enter"));
    for note in MODIFIED_ENTER_NOTES {
        reference_lines.push(Line::from(*note));
    }
    reference_lines.push(Line::from(""));
    reference_lines.push(help_section_heading("Keyboard Capability"));
    reference_lines.extend(
        format_keyboard_enhancement_lines(state.keyboard_enhancement)
            .into_iter()
            .map(Line::from),
    );
    // The probe report says how keys arrive; this says what one of them does,
    // which is the table's to answer and not the probe's. It is the same line
    // under every outcome, which is the whole of its point.
    if let Some(note) = portable_newline_note() {
        reference_lines.push(Line::from(note));
    }

    let [first, second] = binding_columns;
    for (lines, area) in [
        (first, columns[0]),
        (second, columns[1]),
        (reference_lines, columns[2]),
    ] {
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
    }
}

fn push_section(lines: &mut Vec<Line<'static>>, section: &HelpSection) {
    if !lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines.push(help_section_heading(section.heading));
    for entry in &section.entries {
        lines.push(Line::from(format!(
            "{}: {}",
            entry.chords, entry.description
        )));
    }
    for note in notes_for(section.heading) {
        lines.push(Line::from(*note));
    }
}

/// Behavior a section needs explained that no binding row carries.
fn notes_for(heading: &str) -> &'static [&'static str] {
    match heading {
        "Communication Mode" => &["Mouse wheel: Scroll chat history"],
        "Interaction Mode" => &[
            "Write pane: write has text, or none pending",
            "Choice pane: write empty and a request pending",
        ],
        "Picker" => &["Auto-opens entering Interaction w/o target"],
        _ => &[],
    }
}

/// The one binding whose reach does not depend on the probe outcome, named
/// beside the outcome that does not change it.
///
/// The claim is universal, so it may only be printed while it is true. Every
/// context that owns a text draft has to bind newline insertion to the same
/// chord; where they diverge the note disappears rather than narrowing to one
/// surface without saying so, since a line reading "in every case" beside a
/// chord that reaches one pane is worse than no line.
fn portable_newline_note() -> Option<String> {
    let mut chords = [
        binding_for(BindingContext::ComposeMessage, Action::InsertMessageNewline),
        binding_for(BindingContext::InteractionWrite, Action::InsertRawwNewline),
        binding_for(BindingContext::InteractionChoice, Action::InsertRawwNewline),
    ]
    .into_iter()
    .map(|entry| entry.map(|entry| entry.primary_chord().to_string()));
    let portable = chords.next()??;
    chords
        .all(|chord| chord.as_deref() == Some(portable.as_str()))
        .then(|| format!("{portable} inserts a newline in every case"))
}

const TO_FIELD_GRAMMAR: &[&str] = &[
    "session — route to active bundle",
    "session@bundle — route to named bundle",
    "session@GLOBAL — relay-wide user",
    "Comma-separate multiple recipients",
];

/// Stated once rather than on every `Enter` line. Capability-neutral defaults
/// make the modified forms redundant wherever `Enter` is bound, and repeating
/// them inline tripled the width of the lines that carry them.
///
/// Two facts, not one. The neutrality contract governs `Shift+Enter` and
/// `Ctrl+Enter` everywhere; the interaction panes and the picker additionally
/// carry a modifier-agnostic fallback row, so `Alt+Enter` reaches their `Enter`
/// action too, and compose deliberately does not. Saying only the first would
/// understate what the table binds.
const MODIFIED_ENTER_NOTES: &[&str] = &[
    "Shift+Enter and Ctrl+Enter match Enter wherever it is bound.",
    "In the write and choice panes and the picker, any modifier on Enter matches.",
    "Compose binds only the three.",
];

/// The three column areas, laid out with a gutter so they do not run together
/// when the terminal is narrow enough to wrap their lines.
fn help_columns(inner: Rect) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Percentage(40),
            Constraint::Percentage(20),
        ])
        .spacing(GUTTER)
        .split(inner)
}

/// Blank columns between the three panes.
const GUTTER: u16 = 2;

fn help_section_heading(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        ratatui::style::Style::default().add_modifier(Modifier::BOLD),
    ))
}

// Inline by exception, under the project's three conditions for it.
// `render_help_overlay` is crate-private by design and no public interface
// reaches it; making it externally testable would mean adding a
// render-to-buffer method to `Workbench` that exists only for this test, which
// is exactly the unintended API surface the policy is guarding against. One
// `#[test]` function, as the policy caps it.
//
// It earns the exception by covering what the catalogue tests cannot: that the
// generated bindings and the hand-written material actually fit the geometry
// task 4.6 claims, and that the three columns do not collide.
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    use crate::tui::keyboard::KeyboardEnhancement;
    use crate::tui::state::{AppState, TuiLaunchOptions};

    fn rendered(width: u16, height: u16) -> Vec<String> {
        rendered_under(width, height, KeyboardEnhancement::Unsupported)
    }

    fn rendered_under(width: u16, height: u16, enhancement: KeyboardEnhancement) -> Vec<String> {
        let mut state = AppState::new(TuiLaunchOptions {
            namespace: "agentmux".to_string(),
            sender_session: "tui".to_string(),
            relay_socket: std::path::PathBuf::from("/tmp/agentmux-help-render.sock"),
            look_lines: None,
            available_bundles: vec!["agentmux".to_string()],
        });
        state.keyboard_enhancement = enhancement;
        state.help_overlay_open = true;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| render_help_overlay(frame, &state))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|row| {
                (0..buffer.area.width)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    fn columns_of(width: u16, height: u16) -> (Rect, std::rc::Rc<[Rect]>) {
        let popup = centered_rect(96, 92, Rect::new(0, 0, width, height));
        let inner = Block::default().borders(Borders::ALL).inner(popup);
        let columns = help_columns(inner);
        (inner, columns)
    }

    /// One column's text with its wrapping undone, so an assertion is about
    /// what the column says rather than about where the terminal broke it.
    fn column_text(lines: &[String], area: Rect) -> String {
        let joined = (area.y..area.y + area.height)
            .filter_map(|row| lines.get(row as usize))
            .map(|line| {
                line.chars()
                    .skip(area.x as usize)
                    .take(area.width as usize)
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join(" ");
        joined.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn the_overlay_fits_its_geometry_and_ignores_the_probe_outcome() {
        let (inner, columns) = columns_of(120, 44);
        let lines = rendered(120, 44);
        let bindings = format!(
            "{} {}",
            column_text(&lines, columns[0]),
            column_text(&lines, columns[1])
        );
        let reference = column_text(&lines, columns[2]);

        // Material no binding row can carry, which survives only because this
        // module still declares it.
        for expected in [
            "Mouse wheel: Scroll chat history",
            "Write pane: write has text, or none pending",
            "Choice pane: write empty and a request pending",
            "Auto-opens entering Interaction w/o target",
        ] {
            assert!(
                bindings.contains(expected),
                "{expected:?} missing from the binding columns"
            );
        }
        for expected in [
            "session@GLOBAL",
            "Kitty keyboard protocol",
            "Shift+Enter and Ctrl+Enter match Enter wherever it is bound.",
            "any modifier on Enter matches",
            "Compose binds only the three.",
        ] {
            assert!(
                reference.contains(expected),
                "{expected:?} missing from the reference column"
            );
        }

        // Generated bindings from every section reached the buffer, including
        // the last entry of the second column, which is what overflowed while
        // this was a two-column layout.
        for expected in [
            "Ctrl+C: Quit from anywhere",
            "Ctrl+J: Message: insert newline",
            "Enter: Choice: resolve selected option",
            "Enter: Session col: insert or open look",
        ] {
            assert!(
                bindings.contains(expected),
                "{expected:?} missing from the binding columns"
            );
        }

        // Task 4.5, where the buffer is available to assert it. The generated
        // bindings must be byte-identical under every probe outcome, so no
        // capability conditioning can re-enter through the rendering path.
        // The capability report is the deliberate exception -- it reports what
        // the probe determined, which is not a binding -- so it is asserted to
        // differ, or this check would pass on a page that ignored the outcome
        // entirely and prove nothing about the separation.
        let mut reports = Vec::new();
        for enhancement in [
            KeyboardEnhancement::Active,
            KeyboardEnhancement::Unsupported,
            KeyboardEnhancement::ProbeFailed,
        ] {
            let under = rendered_under(120, 44, enhancement);
            assert_eq!(
                (
                    column_text(&under, columns[0]),
                    column_text(&under, columns[1])
                ),
                (
                    column_text(&lines, columns[0]),
                    column_text(&lines, columns[1])
                ),
                "the generated bindings changed under {enhancement:?}"
            );
            reports.push(column_text(&under, columns[2]));
        }
        assert_eq!(
            reports
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            reports.len(),
            "the capability report is the same under every outcome, so this test \
             cannot tell an ignored probe from a respected one"
        );

        // The portable-newline line sits beside that report and is the opposite
        // of it: generated rather than written down, and invariant rather than
        // outcome-dependent. Both halves are asserted, and the chord comes from
        // the table rather than from the function that prints it, which would
        // only ask that function to agree with itself.
        let newline = binding_for(BindingContext::ComposeMessage, Action::InsertMessageNewline)
            .expect("the message field binds inserting a newline");
        let portable = format!(
            "{} inserts a newline in every case",
            newline.primary_chord()
        );
        for (report, enhancement) in reports.iter().zip([
            KeyboardEnhancement::Active,
            KeyboardEnhancement::Unsupported,
            KeyboardEnhancement::ProbeFailed,
        ]) {
            assert!(
                report.contains(&portable),
                "the capability column under {enhancement:?} omits {portable:?}: {report}"
            );
        }

        // Columns are separated, and nothing is written into the separation.
        // The width is asserted against a literal rather than against `GUTTER`,
        // because deriving the bound from the constant under test makes the
        // check vacuous when the constant goes to zero.
        for (left, right) in [(columns[0], columns[1]), (columns[1], columns[2])] {
            let gap = right.x - (left.x + left.width);
            assert!(gap >= 1, "columns are flush: {left:?} then {right:?}");
            for row in inner.y..inner.y + inner.height {
                for column in left.x + left.width..right.x {
                    let cell = lines[row as usize]
                        .chars()
                        .nth(column as usize)
                        .expect("cell within the buffer");
                    assert_eq!(cell, ' ', "gutter at {column} occupied on row {row}");
                }
            }
        }
    }
}
