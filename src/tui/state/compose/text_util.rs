pub(super) fn wrap_index(index: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let len = len as isize;
    ((index as isize + delta).rem_euclid(len)) as usize
}

pub(super) fn previous_char_boundary(value: &str, cursor_index: usize) -> usize {
    value[..cursor_index]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

pub(super) fn next_char_boundary(value: &str, cursor_index: usize) -> usize {
    value
        .char_indices()
        .find_map(|(index, _)| (index > cursor_index).then_some(index))
        .unwrap_or(value.len())
}

pub(super) fn line_ranges(value: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::<(usize, usize)>::new();
    let mut line_start = 0usize;
    for (index, character) in value.char_indices() {
        if character == '\n' {
            ranges.push((line_start, index));
            line_start = index + character.len_utf8();
        }
    }
    ranges.push((line_start, value.len()));
    ranges
}

pub(super) fn line_range_for_cursor(value: &str, cursor_index: usize) -> (usize, usize) {
    let cursor_index = cursor_index.min(value.len());
    let ranges = line_ranges(value);
    for (line_index, (line_start, line_end)) in ranges.iter().enumerate() {
        if cursor_index <= *line_end || line_index + 1 == ranges.len() {
            return (*line_start, *line_end);
        }
    }
    (0, value.len())
}

pub(super) fn line_and_column_for_index(value: &str, cursor_index: usize) -> (usize, usize) {
    let ranges = line_ranges(value);
    for (line_index, (line_start, line_end)) in ranges.iter().enumerate() {
        if cursor_index <= *line_end || line_index + 1 == ranges.len() {
            let column_end = cursor_index.min(*line_end);
            let column = value[*line_start..column_end].chars().count();
            return (line_index, column);
        }
    }
    (0, 0)
}

pub(super) fn cursor_index_for_line_column(
    value: &str,
    line_range: (usize, usize),
    target_column: usize,
) -> usize {
    let (line_start, line_end) = line_range;
    let line_slice = &value[line_start..line_end];
    let line_len = line_slice.chars().count();
    let clamped_column = target_column.min(line_len);
    if clamped_column == line_len {
        return line_end;
    }
    line_start
        + line_slice
            .char_indices()
            .nth(clamped_column)
            .map(|(index, _)| index)
            .unwrap_or(0)
}
