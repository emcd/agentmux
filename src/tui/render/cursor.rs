use ratatui::{Frame, layout::Rect};

use super::super::state::{AppState, FocusField, ScreenMode};
use super::frame::INTERACTION_RAWW_PANE_HEIGHT;
use super::geometry::{
    compose_message_layout, compose_message_visible_start, compose_titled_block, raww_titled_block,
    split_workbench_rows,
};

pub(super) fn render_active_cursor(frame: &mut Frame, area: Rect, state: &AppState) {
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
            let cursor_column = visible_cursor_column_count(state.to_cursor_column(), field_width);
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

fn visible_cursor_column_count(count: usize, width: u16) -> u16 {
    if width == 0 {
        return 0;
    }
    (count as u16).min(width.saturating_sub(1))
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
