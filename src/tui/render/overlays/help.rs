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
//!
//! Each column is drawn through a viewport, because the whole surface is taller
//! than a short terminal can show and the overlay is required to keep all of it
//! reachable. The chords that move the viewport are table rows like any other,
//! declared under the help-overlay context; this module reads them and does not
//! name them.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use super::super::super::actions::{
    Action, BindingContext, EffectiveBindings, HelpSection, binding_for, help_bindings,
};
use super::super::super::keyboard::format_keyboard_enhancement_lines;
use super::super::super::state::AppState;
use super::super::geometry::centered_rect;

pub(in crate::tui::render) fn render_help_overlay(frame: &mut Frame, state: &mut AppState) {
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
    for section in &help_bindings(&state.bindings) {
        let column = match section.heading {
            "Modes" | "Communication Mode" => 0,
            _ => 1,
        };
        push_section(&mut binding_columns[column], section);
    }

    let [first, second] = binding_columns;
    let panes = [
        (first, columns[0]),
        (second, columns[1]),
        (reference_column(state), columns[2]),
    ];

    let viewports: Vec<Viewport> = panes
        .iter()
        .map(|(lines, area)| Viewport::measure(&state.bindings, lines, *area))
        .collect();
    // A page is the shortest column's content height, so paging cannot skip
    // past a row in any of them.
    let page_rows = viewports
        .iter()
        .map(|viewport| viewport.content_rows)
        .min()
        .unwrap_or_default();
    // ...and the scrollable extent is the longest column's, so every column
    // reaches its own last row.
    let maximum_scroll = viewports
        .iter()
        .map(|viewport| viewport.extent)
        .max()
        .unwrap_or_default();
    state.set_help_overlay_viewport(page_rows, maximum_scroll);
    let offset = state.help_overlay_scroll();

    for ((lines, area), viewport) in panes.into_iter().zip(viewports) {
        viewport.draw(frame, &state.bindings, lines, area, offset);
    }
}

/// The hand-written material, led by the capability report.
///
/// The report is ordered first because `tui-surface` asks for it to be visible
/// in the overlay rather than merely reachable through it, and a viewport shows
/// a column from its beginning. It was declared last here, which is exactly why
/// it was the first thing a short terminal lost.
fn reference_column(state: &AppState) -> Vec<Line<'static>> {
    let mut lines = vec![help_section_heading("Keyboard Capability")];
    lines.extend(
        format_keyboard_enhancement_lines(state.keyboard_enhancement())
            .into_iter()
            .map(Line::from),
    );
    // The probe report says how keys arrive; this says what one of them does,
    // which is the table's to answer and not the probe's. It is the same line
    // under every outcome, which is the whole of its point.
    if let Some(note) = portable_newline_note(&state.bindings) {
        lines.push(Line::from(note));
    }
    lines.push(Line::from(""));
    lines.push(help_section_heading("To Field Grammar"));
    for note in TO_FIELD_GRAMMAR {
        lines.push(Line::from(*note));
    }
    lines.push(Line::from(""));
    lines.push(help_section_heading("Modified Enter"));
    for note in MODIFIED_ENTER_NOTES {
        lines.push(Line::from(*note));
    }
    lines
}

fn wrapped(lines: Vec<Line<'static>>) -> Paragraph<'static> {
    Paragraph::new(lines).wrap(Wrap { trim: false })
}

/// How many rows a column's text needs at this width.
///
/// `Paragraph`'s own measurement, at the same width and with the same wrapping
/// the drawing uses. A second implementation of wrapping could disagree with
/// the renderer about where a line breaks, and a row budget computed from a
/// disagreeing measure would mis-place the very marker it is being computed
/// for. With `Wrap` each source line wraps independently, so a column's cost is
/// the sum of its lines'.
fn row_cost(line: &Line<'static>, width: u16) -> usize {
    wrapped(vec![line.clone()]).line_count(width).max(1)
}

/// What one column's content costs at the width it is drawn at, and how much of
/// it the terminal can show at once.
///
/// `Paragraph` draws the rows that fit and discards the rest with no trace on
/// screen. That is how three bindings -- committing a picker session among them
/// -- went missing from this overlay below 41 rows, while every test passed,
/// before there was a viewport. Drawing through one is what makes the rest
/// reachable; the marker is what keeps its absence from being silent.
#[derive(Clone, Copy)]
struct Viewport {
    /// Rows the whole column occupies at this width.
    total_rows: usize,
    /// Rows left for content once the marker's own row (or rows) are reserved.
    content_rows: usize,
    /// The largest offset that still moves this column. Zero where the column
    /// fits, which is also what says it needs no marker.
    extent: usize,
}

impl Viewport {
    fn measure(bindings: &EffectiveBindings, lines: &[Line<'static>], area: Rect) -> Self {
        let available = usize::from(area.height);
        if area.width == 0 || available == 0 {
            return Self {
                total_rows: 0,
                content_rows: 0,
                extent: 0,
            };
        }
        let total_rows = wrapped(lines.to_vec()).line_count(area.width);
        if total_rows <= available {
            return Self {
                total_rows,
                content_rows: available,
                extent: 0,
            };
        }
        // Reserved against the marker's widest form -- both directions, with
        // the whole column's row count in each -- so the number of content rows
        // does not change as the operator scrolls. A reservation that moved
        // would shift the content under the row they were reading.
        let reserved = row_cost(
            &overflow_marker(bindings, total_rows, total_rows),
            area.width,
        );
        let content_rows = available.saturating_sub(reserved);
        Self {
            total_rows,
            content_rows,
            extent: total_rows.saturating_sub(content_rows),
        }
    }

    fn draw(
        self,
        frame: &mut Frame,
        bindings: &EffectiveBindings,
        lines: Vec<Line<'static>>,
        area: Rect,
        offset: usize,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        if self.extent == 0 {
            frame.render_widget(wrapped(lines), area);
            return;
        }
        // Clamped to this column's own extent. The columns are unequal, and a
        // shared unclamped offset would empty the short reference column --
        // taking the capability report with it -- while a binding column was
        // being scrolled.
        let offset = offset.min(self.extent);
        let content_rows = self.content_rows.min(usize::from(area.height)) as u16;
        if content_rows > 0 {
            frame.render_widget(
                wrapped(lines).scroll((offset.min(usize::from(u16::MAX)) as u16, 0)),
                Rect {
                    height: content_rows,
                    ..area
                },
            );
        }
        let below = self.total_rows.saturating_sub(offset + self.content_rows);
        frame.render_widget(
            wrapped(vec![overflow_marker(bindings, offset, below)]),
            Rect {
                y: area.y + content_rows,
                height: area.height - content_rows,
                ..area
            },
        );
    }
}

/// Says what lies outside the viewport and which chords reach it.
///
/// Counted in rendered rows, which is what the viewport moves in and what makes
/// the two numbers account for everything off screen. The earlier marker
/// counted bindings, because the rows it named were lost rather than merely
/// elsewhere; now that they are reachable, the operator's question is how far,
/// not how many.
///
/// Where the table declares no scrolling this degrades to the resize advice the
/// marker carried before there was a viewport. That is what keeps it a safety
/// net: rows removed from the table leave a visible consequence rather than a
/// marker pointing at a chord that does nothing.
fn overflow_marker(bindings: &EffectiveBindings, above: usize, below: usize) -> Line<'static> {
    let text = match scroll_chords(bindings) {
        None => format!("… {} more (resize taller)", above + below),
        Some((up, down)) => match (above, below) {
            (0, _) => format!("… {below} below ({down})"),
            (_, 0) => format!("… {above} above ({up})"),
            _ => format!("… {above} above, {below} below ({up}/{down})"),
        },
    };
    Line::from(Span::styled(
        text,
        ratatui::style::Style::default().add_modifier(Modifier::BOLD),
    ))
}

/// The chords that move the viewport, from the table rather than from here.
fn scroll_chords(bindings: &EffectiveBindings) -> Option<(String, String)> {
    let up = binding_for(
        bindings,
        BindingContext::HelpOverlay,
        Action::ScrollHelpPageUp,
    )?;
    let down = binding_for(
        bindings,
        BindingContext::HelpOverlay,
        Action::ScrollHelpPageDown,
    )?;
    Some((
        up.primary_chord().to_string(),
        down.primary_chord().to_string(),
    ))
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
fn portable_newline_note(bindings: &EffectiveBindings) -> Option<String> {
    let mut chords = [
        binding_for(
            bindings,
            BindingContext::ComposeMessage,
            Action::InsertMessageNewline,
        ),
        binding_for(
            bindings,
            BindingContext::InteractionWrite,
            Action::InsertRawwNewline,
        ),
        binding_for(
            bindings,
            BindingContext::InteractionChoice,
            Action::InsertRawwNewline,
        ),
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
/// One fact now, where there were two. The note used to add that the
/// interaction panes and the picker carried a modifier-agnostic fallback row,
/// so `Alt+Enter` reached their `Enter` action while compose refused it. That
/// asymmetry is gone: every context binds the three declared chords and no
/// other modifier set, so the second sentence would now be false and the third
/// would imply a distinction that no longer exists.
///
/// Hand-written rather than generated, which is why an edit was needed here at
/// all. The lines below name chords outside the binding table, so nothing makes
/// them follow it — a gap recorded for the documentation group rather than
/// closed here.
const MODIFIED_ENTER_NOTES: &[&str] =
    &["Shift+Enter and Ctrl+Enter match Enter wherever it is bound."];

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
// It earns the exception by covering what the catalogue tests cannot: that
// every generated binding is reachable at the geometry the requirement names,
// that the capability report is on screen before any scrolling, and that the
// three columns do not collide. The scroll semantics that are state rather than
// geometry are asserted through the public facade, in `tests/unit`.
#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};

    use crate::tui::actions::default_binding;
    use crate::tui::keyboard::KeyboardEnhancement;
    use crate::tui::state::{AppState, TuiLaunchOptions};

    /// The heights the overlay is exercised at.
    ///
    /// 24 is the size the requirement names and the size the defect was found
    /// at. The middle four are the heights the pre-viewport overlay dropped
    /// bindings at. The last is above the height the whole overlay fits in, and
    /// is the control: if the property below held only where content overflows,
    /// an overlay that showed nothing and said so would satisfy it everywhere.
    const HEIGHTS: [u16; 6] = [24, 30, 36, 41, 44, 48];

    /// The height at and above which nothing scrolls, measured rather than
    /// chosen. Generation is one line per behavior, so this rises whenever the
    /// table gains rows; a failure here is that fact arriving, not a flake.
    const FITS_FROM: u16 = 48;

    /// One overlay across several frames, so a scroll position survives the
    /// redraw that publishes the bounds it is clamped against.
    struct Overlay {
        state: AppState,
        width: u16,
        height: u16,
    }

    impl Overlay {
        fn new(width: u16, height: u16, enhancement: KeyboardEnhancement) -> Self {
            let mut state = AppState::new(TuiLaunchOptions {
                namespace: "agentmux".to_string(),
                sender_session: "tui".to_string(),
                relay_socket: std::path::PathBuf::from("/tmp/agentmux-help-render.sock"),
                look_lines: None,
                available_bundles: vec!["agentmux".to_string()],
                bindings: None,
            });
            state.set_keyboard_enhancement(enhancement);
            state.help_overlay_open = true;
            Self {
                state,
                width,
                height,
            }
        }

        fn draw(&mut self) -> Vec<String> {
            let mut terminal =
                Terminal::new(TestBackend::new(self.width, self.height)).expect("terminal");
            terminal
                .draw(|frame| render_help_overlay(frame, &mut self.state))
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

        /// Presses a key the way dispatch does: resolved against the table in
        /// the overlay's own context, then applied. A chord the table does not
        /// declare there panics here, which is what makes this a test of the
        /// rows rather than of the state methods behind them.
        fn press(&mut self, code: KeyCode) {
            let action = default_binding(BindingContext::HelpOverlay, code, KeyModifiers::NONE)
                .unwrap_or_else(|| panic!("the help overlay declares no row for {code:?}"));
            action
                .apply(&mut self.state)
                .expect("moving the viewport reaches no relay");
        }

        fn scroll(&self) -> usize {
            self.state.help_overlay_scroll()
        }
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

    fn bindings_text(lines: &[String], columns: &[Rect]) -> String {
        format!(
            "{} {}",
            column_text(lines, columns[0]),
            column_text(lines, columns[1])
        )
    }

    fn whole_overlay(lines: &[String], columns: &[Rect]) -> String {
        format!(
            "{} {}",
            bindings_text(lines, columns),
            column_text(lines, columns[2])
        )
    }

    /// What one traversal of the overlay saw.
    struct Traversal {
        /// The overlay as it looks when opened, before anything is pressed.
        opening: String,
        /// Everything any frame showed, across every position the viewport
        /// reached.
        seen: String,
        /// How many times the viewport actually moved.
        steps: usize,
    }

    /// Walks the overlay from its first row to its last using only a chord the
    /// table declares for it, collecting every frame.
    ///
    /// A row at a time rather than a page: paging is reachability too, but a
    /// wrapped binding straddling a page boundary would never appear whole in
    /// any single frame, and the assertion would then be about where the pages
    /// happened to land rather than about whether the operator can read the
    /// binding.
    fn traverse(width: u16, height: u16) -> Traversal {
        let (_, columns) = columns_of(width, height);
        let mut overlay = Overlay::new(width, height, KeyboardEnhancement::Unsupported);
        // The first frame is what publishes the bounds the offset is clamped
        // against, so it has to happen before anything is pressed.
        let opening = whole_overlay(&overlay.draw(), &columns);
        let mut seen = opening.clone();
        let mut steps = 0usize;
        loop {
            let before = overlay.scroll();
            overlay.press(KeyCode::Down);
            seen.push(' ');
            seen.push_str(&whole_overlay(&overlay.draw(), &columns));
            if overlay.scroll() == before {
                break;
            }
            steps += 1;
            assert!(
                steps < 500,
                "the viewport never reached the end at {width}x{height}"
            );
        }
        Traversal {
            opening,
            seen,
            steps,
        }
    }

    #[test]
    fn the_overlay_reaches_every_binding_at_every_height() {
        // No configuration is in play in this module's fixtures, so the
        // effective table is the compiled defaults and the two agree by
        // construction. The configured case is asserted through the workbench,
        // in `tests/unit`.
        let defaults = EffectiveBindings::default();
        let expected: Vec<String> = help_bindings(&defaults)
            .iter()
            .flat_map(|section| section.entries.iter())
            .map(|entry| format!("{}: {}", entry.chords, entry.description))
            .collect();
        assert!(!expected.is_empty(), "the table presents no bindings");

        // Material no binding row can carry, which survives only because this
        // module still declares it.
        let notes = [
            "Mouse wheel: Scroll chat history",
            "Write pane: write has text, or none pending",
            "Choice pane: write empty and a request pending",
            "Auto-opens entering Interaction w/o target",
            "session@GLOBAL",
            "Shift+Enter and Ctrl+Enter match Enter wherever it is bound.",
        ];

        let mut opening_counts = Vec::new();
        for height in HEIGHTS {
            let walk = traverse(120, height);

            for entry in &expected {
                assert!(
                    walk.seen.contains(entry.as_str()),
                    "120x{height}: {entry:?} is unreachable"
                );
            }
            for note in notes {
                assert!(
                    walk.seen.contains(note),
                    "120x{height}: {note:?} is unreachable"
                );
            }

            // The one item `tui-surface` asks to be visible rather than merely
            // reachable. It leads its column for this reason, so no scrolling
            // may be needed to read it at any height in the sweep.
            assert!(
                walk.opening.contains("Kitty keyboard protocol"),
                "120x{height} opens without the capability report on screen:\n{}",
                walk.opening
            );

            if height >= FITS_FROM {
                assert_eq!(
                    walk.steps, 0,
                    "120x{height} is at or above the height everything fits at, yet it scrolls"
                );
                assert!(
                    !walk.opening.contains('…'),
                    "120x{height} fits, yet a marker reports content outside the viewport"
                );
            } else {
                assert!(
                    walk.steps > 0,
                    "120x{height} is below the height everything fits at, yet nothing scrolls; \
                     the sweep no longer exercises the viewport"
                );
                assert!(
                    walk.opening.contains('…'),
                    "120x{height} holds more than it shows and says nothing about it:\n{}",
                    walk.opening
                );
            }

            opening_counts.push(
                expected
                    .iter()
                    .filter(|entry| walk.opening.contains(entry.as_str()))
                    .count(),
            );
        }

        // A taller terminal never shows less before scrolling. A viewport that
        // miscounted rows could satisfy every check above and still regress
        // here.
        for pair in opening_counts.windows(2) {
            assert!(
                pair[1] >= pair[0],
                "a taller overlay opens on fewer bindings: {opening_counts:?}"
            );
        }

        // Asserted here rather than against the catalogue, because the rendered
        // buffer is where a difference would actually show. The generated
        // bindings must be byte-identical under every probe outcome, so no
        // capability conditioning can re-enter through the rendering path. The
        // capability report is the deliberate exception -- it reports what the
        // probe determined, which is not a binding -- so it is asserted to
        // differ, or this check would pass on a page that ignored the outcome
        // entirely and prove nothing about the separation.
        let (inner, columns) = columns_of(120, FITS_FROM);
        let outcomes = [
            KeyboardEnhancement::Active,
            KeyboardEnhancement::Unsupported,
            KeyboardEnhancement::ProbeFailed,
        ];
        let mut reports = Vec::new();
        let mut baseline = None;
        for enhancement in outcomes {
            let lines = Overlay::new(120, FITS_FROM, enhancement).draw();
            let bindings = bindings_text(&lines, &columns);
            match &baseline {
                None => baseline = Some(bindings),
                Some(first) => assert_eq!(
                    &bindings, first,
                    "the generated bindings changed under {enhancement:?}"
                ),
            }
            reports.push(column_text(&lines, columns[2]));
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
        let newline = binding_for(
            &defaults,
            BindingContext::ComposeMessage,
            Action::InsertMessageNewline,
        )
        .expect("the message field binds inserting a newline");
        let portable = format!(
            "{} inserts a newline in every case",
            newline.primary_chord()
        );
        for (report, enhancement) in reports.iter().zip(outcomes) {
            assert!(
                report.contains(&portable),
                "the capability column under {enhancement:?} omits {portable:?}: {report}"
            );
        }

        // Columns are separated, and nothing is written into the separation.
        // The width is asserted against a literal rather than against `GUTTER`,
        // because deriving the bound from the constant under test makes the
        // check vacuous when the constant goes to zero.
        let lines = Overlay::new(120, FITS_FROM, KeyboardEnhancement::Unsupported).draw();
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
