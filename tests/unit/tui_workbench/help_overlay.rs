//! The help overlay's viewport, where it is state rather than geometry.
//!
//! What the overlay actually draws at a given size is asserted against a
//! rendered buffer inside the crate; these cover the offset itself — that it
//! clamps at both ends, that opening the overlay returns it to the start, and
//! that the chords which dismiss the overlay still reach it from a scrolled
//! position.

use crossterm::event::{KeyCode, KeyModifiers};

use super::{key_event, make_state};
use agentmux::tui::workbench::Workbench;

/// Opens the overlay and publishes bounds, as a first frame would.
fn scrolled_overlay(page_rows: usize, maximum_scroll: usize) -> Workbench {
    let mut workbench = make_state();
    workbench
        .dispatch_event(key_event(KeyCode::F(1), KeyModifiers::NONE))
        .expect("open the help overlay");
    assert!(workbench.help_overlay_open());
    workbench.set_help_overlay_viewport(page_rows, maximum_scroll);
    workbench
}

fn press(workbench: &mut Workbench, code: KeyCode) {
    workbench
        .dispatch_event(key_event(code, KeyModifiers::NONE))
        .unwrap_or_else(|error| panic!("dispatch {code:?}: {error}"));
}

#[test]
fn the_viewport_stops_at_both_ends_of_the_content() {
    let mut workbench = scrolled_overlay(10, 25);
    assert_eq!(workbench.help_overlay_scroll(), 0);

    press(&mut workbench, KeyCode::Up);
    assert_eq!(
        workbench.help_overlay_scroll(),
        0,
        "the viewport moved above the start of the content"
    );

    press(&mut workbench, KeyCode::Down);
    assert_eq!(workbench.help_overlay_scroll(), 1);

    press(&mut workbench, KeyCode::PageDown);
    assert_eq!(workbench.help_overlay_scroll(), 11, "a page is ten rows");

    for _ in 0..5 {
        press(&mut workbench, KeyCode::PageDown);
    }
    assert_eq!(
        workbench.help_overlay_scroll(),
        25,
        "the viewport moved past the end of the content"
    );

    press(&mut workbench, KeyCode::PageUp);
    assert_eq!(workbench.help_overlay_scroll(), 15);

    press(&mut workbench, KeyCode::End);
    assert_eq!(workbench.help_overlay_scroll(), 25);
    press(&mut workbench, KeyCode::Home);
    assert_eq!(workbench.help_overlay_scroll(), 0);
}

/// A terminal resized smaller while the overlay is open publishes shorter
/// bounds, and the offset has to come back with them rather than leaving the
/// overlay scrolled past its own content.
#[test]
fn shrinking_the_content_pulls_the_viewport_back() {
    let mut workbench = scrolled_overlay(10, 25);
    press(&mut workbench, KeyCode::End);
    assert_eq!(workbench.help_overlay_scroll(), 25);

    workbench.set_help_overlay_viewport(10, 4);
    assert_eq!(workbench.help_overlay_scroll(), 4);

    workbench.set_help_overlay_viewport(10, 0);
    assert_eq!(workbench.help_overlay_scroll(), 0);
}

/// Help answers the same way wherever it was opened from. Where a previous
/// viewing had scrolled to is part of that answer, so it does not survive a
/// close.
#[test]
fn reopening_the_overlay_returns_to_the_start() {
    let mut workbench = scrolled_overlay(10, 25);
    press(&mut workbench, KeyCode::End);
    assert_eq!(workbench.help_overlay_scroll(), 25);

    press(&mut workbench, KeyCode::Esc);
    assert!(!workbench.help_overlay_open());
    press(&mut workbench, KeyCode::F(1));
    assert!(workbench.help_overlay_open());
    assert_eq!(
        workbench.help_overlay_scroll(),
        0,
        "the overlay reopened where the last viewing had left it"
    );
}

/// The chords that dismiss the overlay are not shadowed by the ones that move
/// it, at any position.
#[test]
fn dismissal_survives_a_scrolled_viewport() {
    for dismiss in [KeyCode::Esc, KeyCode::F(1)] {
        let mut workbench = scrolled_overlay(10, 25);
        press(&mut workbench, KeyCode::End);
        assert_eq!(workbench.help_overlay_scroll(), 25);
        press(&mut workbench, dismiss);
        assert!(
            !workbench.help_overlay_open(),
            "{dismiss:?} did not dismiss a scrolled overlay"
        );
    }
}

/// Scrolling moves nothing until a frame has published bounds. The overlay is
/// drawn on the frame that opens it, so no operator sequence reaches this — but
/// a host driving the workbench without drawing it would, and it must not run
/// away.
#[test]
fn the_viewport_does_not_move_before_bounds_are_published() {
    let mut workbench = make_state();
    press(&mut workbench, KeyCode::F(1));
    for code in [KeyCode::Down, KeyCode::PageDown, KeyCode::End] {
        press(&mut workbench, code);
        assert_eq!(
            workbench.help_overlay_scroll(),
            0,
            "{code:?} moved a viewport whose bounds are unknown"
        );
    }
}
