use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::{Block, Clear, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::centered;

// 72 columns keeps a long review comment visible without hiding the surrounding diff.
const MAX_WIDTH: u16 = 72;
const MAX_INNER_HEIGHT: u16 = 8;

pub(super) const DEFAULT_WRAP_WIDTH: usize = MAX_WIDTH as usize - 2;

struct CommentInputLayout {
    area: Rect,
    text: String,
    scroll: u16,
    cursor: Position,
}

struct VisualRow {
    text: String,
    start: usize,
    end: usize,
}

pub(super) fn wrap_width(bounds: Rect) -> usize {
    let width = bounds.width.saturating_sub(4).min(MAX_WIDTH);
    let width = width.max(3.min(bounds.width));
    usize::from(width.saturating_sub(2)).max(1)
}

pub(super) fn render(
    frame: &mut Frame<'_>,
    bounds: Rect,
    body: &str,
    cursor: usize,
    selected_row: Option<Rect>,
    editing: bool,
) {
    let layout = layout_comment_input(bounds, body, cursor, selected_row);
    let title = if editing {
        " Edit comment "
    } else {
        " Comment "
    };
    frame.render_widget(Clear, layout.area);
    frame.render_widget(
        Paragraph::new(layout.text)
            .block(Block::bordered().title(title))
            .scroll((layout.scroll, 0)),
        layout.area,
    );
    if layout.area.width > 2 && layout.area.height > 2 {
        frame.set_cursor_position(layout.cursor);
    }
}

fn layout_comment_input(
    screen: Rect,
    body: &str,
    cursor: usize,
    selected_row: Option<Rect>,
) -> CommentInputLayout {
    let inner_width = wrap_width(screen);
    let rows = visual_rows(body, inner_width);
    let cursor = cursor.min(body.len());
    let cursor_row = row_index(&rows, cursor);

    let max_inner = screen.height.saturating_sub(2).min(MAX_INNER_HEIGHT).max(1);
    let visible_inner = (rows.len() as u16).clamp(1, max_inner);
    let height = visible_inner.saturating_add(2).min(screen.height);
    let area = place_comment_popup(screen, selected_row, popup_width(screen), height);
    let inner_height = area.height.saturating_sub(2).max(1);
    let cursor_row = cursor_row as u16;
    let scroll = cursor_row
        .saturating_add(1)
        .saturating_sub(inner_height)
        .min((rows.len() as u16).saturating_sub(inner_height));
    let cursor_col = visual_col(&rows[cursor_row as usize], body, cursor);
    let cursor = Position::new(
        area.x.saturating_add(1).saturating_add(cursor_col),
        area.y
            .saturating_add(1)
            .saturating_add(cursor_row.saturating_sub(scroll)),
    );

    CommentInputLayout {
        area,
        text: rows
            .into_iter()
            .map(|row| row.text)
            .collect::<Vec<_>>()
            .join("\n"),
        scroll,
        cursor,
    }
}

fn popup_width(screen: Rect) -> u16 {
    let width = screen.width.saturating_sub(4).min(MAX_WIDTH);
    width.max(3.min(screen.width))
}

pub(super) fn move_left(body: &str, cursor: usize) -> usize {
    prev_boundary(body, cursor.min(body.len()))
}

pub(super) fn move_right(body: &str, cursor: usize) -> usize {
    next_boundary(body, cursor.min(body.len()))
}

pub(super) fn move_up(body: &str, cursor: usize, width: usize) -> usize {
    let rows = visual_rows(body, width);
    let cursor = cursor.min(body.len());
    let row = row_index(&rows, cursor);
    if row == 0 {
        return cursor;
    }
    let col = visual_col(&rows[row], body, cursor);
    index_at_col(body, &rows[row - 1], col)
}

pub(super) fn move_down(body: &str, cursor: usize, width: usize) -> usize {
    let rows = visual_rows(body, width);
    let cursor = cursor.min(body.len());
    let row = row_index(&rows, cursor);
    if row + 1 >= rows.len() {
        return cursor;
    }
    let col = visual_col(&rows[row], body, cursor);
    index_at_col(body, &rows[row + 1], col)
}

pub(super) fn move_line_start(body: &str, cursor: usize, width: usize) -> usize {
    let rows = visual_rows(body, width);
    let cursor = cursor.min(body.len());
    rows[row_index(&rows, cursor)].start
}

pub(super) fn move_line_end(body: &str, cursor: usize, width: usize) -> usize {
    let rows = visual_rows(body, width);
    let cursor = cursor.min(body.len());
    rows[row_index(&rows, cursor)].end
}

pub(super) fn insert_char(body: &mut String, cursor: &mut usize, character: char) {
    *cursor = (*cursor).min(body.len());
    body.insert(*cursor, character);
    *cursor += character.len_utf8();
}

pub(super) fn backspace(body: &mut String, cursor: &mut usize) {
    *cursor = (*cursor).min(body.len());
    if *cursor == 0 {
        return;
    }
    let prev = prev_boundary(body, *cursor);
    body.replace_range(prev..*cursor, "");
    *cursor = prev;
}

pub(super) fn delete_forward(body: &mut String, cursor: &mut usize) {
    *cursor = (*cursor).min(body.len());
    if *cursor == body.len() {
        return;
    }
    let next = next_boundary(body, *cursor);
    body.replace_range(*cursor..next, "");
}

fn visual_rows(body: &str, width: usize) -> Vec<VisualRow> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut offset = 0;
    for (index, source_line) in body.split('\n').enumerate() {
        if index > 0 {
            offset += 1;
        }
        wrap_source_line(source_line, offset, width, &mut rows);
        offset += source_line.len();
    }
    if rows.is_empty() {
        rows.push(VisualRow {
            text: String::new(),
            start: 0,
            end: 0,
        });
    }
    if UnicodeWidthStr::width(rows.last().map(|row| row.text.as_str()).unwrap_or("")) >= width {
        rows.push(VisualRow {
            text: String::new(),
            start: body.len(),
            end: body.len(),
        });
    }
    rows
}

fn wrap_source_line(source_line: &str, line_start: usize, width: usize, rows: &mut Vec<VisualRow>) {
    if source_line.is_empty() {
        rows.push(VisualRow {
            text: String::new(),
            start: line_start,
            end: line_start,
        });
        return;
    }

    let mut remaining = source_line;
    let mut remaining_start = line_start;
    while UnicodeWidthStr::width(remaining) > width {
        let hard_break = byte_index_at_width(remaining, width);
        let soft_break = remaining[..hard_break]
            .rfind(|character: char| character.is_whitespace())
            .filter(|index| *index > 0);
        let split = soft_break.unwrap_or(hard_break);
        let displayed = remaining[..split].trim_end();
        rows.push(VisualRow {
            text: displayed.to_owned(),
            start: remaining_start,
            end: remaining_start + displayed.len(),
        });
        let next = remaining[split..].trim_start();
        remaining_start += remaining.len() - next.len();
        remaining = next;
    }
    if !remaining.is_empty() {
        rows.push(VisualRow {
            text: remaining.to_owned(),
            start: remaining_start,
            end: remaining_start + remaining.len(),
        });
    }
}

fn byte_index_at_width(text: &str, width: usize) -> usize {
    let mut used = 0;
    for (index, character) in text.char_indices() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > width {
            return if index == 0 {
                character.len_utf8()
            } else {
                index
            };
        }
        used += character_width;
    }
    text.len()
}

fn row_index(rows: &[VisualRow], cursor: usize) -> usize {
    rows.iter()
        .rposition(|row| row.start <= cursor)
        .unwrap_or(0)
}

fn visual_col(row: &VisualRow, body: &str, cursor: usize) -> u16 {
    let clamped = cursor.clamp(row.start, row.end);
    UnicodeWidthStr::width(&body[row.start..clamped]) as u16
}

fn index_at_col(body: &str, row: &VisualRow, col: u16) -> usize {
    let mut used = 0u16;
    for (offset, character) in body[row.start..row.end].char_indices() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0) as u16;
        if used + character_width > col {
            return row.start + offset;
        }
        used += character_width;
    }
    row.end
}

fn prev_boundary(text: &str, index: usize) -> usize {
    if index == 0 {
        return 0;
    }
    let mut prev = index - 1;
    while !text.is_char_boundary(prev) {
        prev -= 1;
    }
    prev
}

fn next_boundary(text: &str, index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    let mut next = index + 1;
    while next < text.len() && !text.is_char_boundary(next) {
        next += 1;
    }
    next
}

fn place_comment_popup(screen: Rect, selected_row: Option<Rect>, width: u16, height: u16) -> Rect {
    let width = width.min(screen.width);
    let height = height.min(screen.height);
    let Some(row) = selected_row.filter(|row| row.y < screen.bottom() && row.bottom() > screen.y)
    else {
        return centered(screen, width, height);
    };

    let max_x = screen.x.saturating_add(screen.width.saturating_sub(width));
    let x = row.x.saturating_add(2).clamp(screen.x, max_x);
    let below_y = row.y.saturating_add(1);
    let y = if below_y.saturating_add(height) <= screen.bottom() {
        below_y
    } else if row.y.saturating_sub(screen.y) >= height {
        row.y.saturating_sub(height)
    } else {
        screen.bottom().saturating_sub(height).max(screen.y)
    };

    Rect::new(x, y, width, height)
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use unicode_width::UnicodeWidthStr;

    use super::super::diff_view::wrap_comment_body;
    use super::{
        DEFAULT_WRAP_WIDTH, MAX_INNER_HEIGHT, MAX_WIDTH, backspace, delete_forward, insert_char,
        layout_comment_input, move_down, move_left, move_line_end, move_line_start, move_right,
        move_up, place_comment_popup, render, visual_rows, wrap_width,
    };

    fn inner_width(area: Rect) -> usize {
        usize::from(area.width.saturating_sub(2)).max(1)
    }

    fn layout_at_end(
        screen: Rect,
        body: &str,
        selected_row: Option<Rect>,
    ) -> super::CommentInputLayout {
        layout_comment_input(screen, body, body.len(), selected_row)
    }

    #[test]
    fn visual_rows_match_comment_body_wrapping() {
        let long = "word ".repeat(40);
        let bodies = [
            "",
            "looks good",
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda",
            long.trim_end(),
            "hello\n\nworld",
            "exceptionallylongwordwithoutspaces",
        ];
        for body in bodies {
            for width in [1, 8, 12, 34, DEFAULT_WRAP_WIDTH] {
                let wrapped = wrap_comment_body(body, width);
                let rows: Vec<String> = visual_rows(body, width)
                    .into_iter()
                    .map(|row| row.text)
                    .take(wrapped.len().max(1))
                    .collect();
                let expected = if wrapped.is_empty() {
                    vec![String::new()]
                } else {
                    wrapped
                };
                assert_eq!(
                    rows, expected,
                    "popup wrapping must match inline comment wrapping for {body:?} width {width}"
                );
            }
        }
    }

    #[test]
    fn short_comment_stays_one_line_tall() {
        let layout = layout_at_end(Rect::new(0, 0, 80, 24), "looks good", None);
        assert_eq!(
            layout.area.height, 3,
            "a short comment must keep the original single-line popup height"
        );
        assert_eq!(layout.text, "looks good");
        assert_eq!(layout.scroll, 0);
    }

    #[test]
    fn long_comment_wraps_and_grows_instead_of_scrolling_sideways() {
        let body = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda";
        let layout = layout_at_end(Rect::new(0, 0, 40, 24), body, None);
        let wrapped: Vec<&str> = layout.text.lines().collect();

        assert!(
            layout.area.height > 3,
            "wrapping a long comment must grow the popup: {:?}",
            layout.area
        );
        assert!(
            wrapped.len() > 1,
            "a long comment must occupy more than one visual row: {wrapped:?}"
        );
        assert!(
            wrapped
                .iter()
                .all(|line| UnicodeWidthStr::width(*line) <= inner_width(layout.area)),
            "no wrapped row may exceed the popup inner width: {wrapped:?}"
        );
        assert!(
            wrapped.iter().any(|line| line.contains("alpha"))
                && wrapped.iter().any(|line| line.contains("lambda")),
            "wrapping must keep the start and end of the comment visible across rows: {wrapped:?}"
        );
        assert_eq!(layout.scroll, 0);
        assert!(
            layout.area.width <= MAX_WIDTH,
            "the popup must still cap its width"
        );
    }

    #[test]
    fn very_long_comment_caps_height_and_keeps_the_cursor_on_the_last_row() {
        let body = "word ".repeat(400);
        let body = body.trim_end();
        let layout = layout_at_end(Rect::new(0, 0, 80, 24), body, None);

        assert!(
            layout.area.height <= MAX_INNER_HEIGHT + 2,
            "an oversized comment must stop growing at the inner-height cap: {:?}",
            layout.area
        );
        assert!(
            layout.scroll > 0,
            "overflowing wrapped rows must scroll vertically to the end of the comment"
        );
        assert_eq!(
            layout.cursor.y,
            layout.area.y + layout.area.height - 2,
            "the cursor must stay on the last visible input row"
        );
    }

    #[test]
    fn cursor_at_the_start_scrolls_a_long_comment_to_the_top() {
        let body = "word ".repeat(400);
        let body = body.trim_end();
        let layout = layout_comment_input(Rect::new(0, 0, 80, 24), body, 0, None);

        assert_eq!(
            layout.scroll, 0,
            "the cursor at the start must not stay scrolled to the end"
        );
        assert_eq!(
            layout.cursor.y,
            layout.area.y + 1,
            "the cursor must sit on the first visible input row"
        );
        assert_eq!(
            layout.cursor.x,
            layout.area.x + 1,
            "the cursor must sit at the start of the first row"
        );
    }

    #[test]
    fn left_from_the_end_moves_the_cursor_back_one_column() {
        let body = "looks good";
        let at_end = layout_at_end(Rect::new(0, 0, 80, 24), body, None);
        let cursor = move_left(body, body.len());
        let moved = layout_comment_input(Rect::new(0, 0, 80, 24), body, cursor, None);

        assert_eq!(cursor, body.len() - 1);
        assert_eq!(moved.cursor.y, at_end.cursor.y);
        assert_eq!(moved.cursor.x, at_end.cursor.x - 1);
    }

    #[test]
    fn insert_in_the_middle_does_not_append() {
        let mut body = String::from("hello");
        let mut cursor = body.len();
        cursor = move_left(&body, cursor);
        cursor = move_left(&body, cursor);
        insert_char(&mut body, &mut cursor, 'X');

        assert_eq!(body, "helXlo");
        assert_eq!(cursor, 4);
    }

    #[test]
    fn backspace_and_delete_operate_at_the_cursor() {
        let mut body = String::from("hello");
        let mut cursor = 2;
        backspace(&mut body, &mut cursor);
        assert_eq!(body, "hllo");
        assert_eq!(cursor, 1);

        delete_forward(&mut body, &mut cursor);
        assert_eq!(body, "hlo");
        assert_eq!(cursor, 1);
    }

    #[test]
    fn wrap_row_up_and_down_move_between_visual_rows() {
        let body = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda";
        let screen = Rect::new(0, 0, 40, 24);
        let width = wrap_width(screen);
        let at_end = layout_at_end(screen, body, None);
        assert!(
            at_end.text.lines().count() > 1,
            "the fixture must wrap: {}",
            at_end.text
        );

        let up = move_up(body, body.len(), width);
        assert!(up < body.len(), "up from the end must leave the last row");
        let up_layout = layout_comment_input(screen, body, up, None);
        assert!(
            up_layout.cursor.y < at_end.cursor.y,
            "up must move the rendered cursor to the previous wrapped row: end={:?} up={:?}",
            at_end.cursor,
            up_layout.cursor
        );

        let down = move_down(body, up, width);
        let down_layout = layout_comment_input(screen, body, down, None);
        assert_eq!(
            down_layout.cursor.y, at_end.cursor.y,
            "down must return to the last wrapped row"
        );

        let home = move_line_start(body, up, width);
        let home_layout = layout_comment_input(screen, body, home, None);
        assert_eq!(home_layout.cursor.x, up_layout.area.x + 1);
        assert_eq!(home_layout.cursor.y, up_layout.cursor.y);

        let end = move_line_end(body, home, width);
        let end_layout = layout_comment_input(screen, body, end, None);
        assert!(end_layout.cursor.x >= home_layout.cursor.x);
        assert_eq!(end_layout.cursor.y, home_layout.cursor.y);
    }

    #[test]
    fn left_and_right_cross_wrapped_rows() {
        let body = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda";
        let screen = Rect::new(0, 0, 40, 24);
        let width = wrap_width(screen);
        let second_row = move_down(body, 0, width);
        assert!(
            second_row > 0,
            "down from the start must reach the next wrap"
        );

        let back = move_left(body, second_row);
        assert!(back < second_row);
        let start_layout = layout_comment_input(screen, body, 0, None);
        let second_layout = layout_comment_input(screen, body, second_row, None);
        let back_layout = layout_comment_input(screen, body, back, None);
        assert_eq!(second_layout.cursor.y, start_layout.cursor.y + 1);
        assert_eq!(
            back_layout.cursor.y, start_layout.cursor.y,
            "left from the start of a wrapped row must land on the previous row"
        );
        assert_eq!(move_right(body, back), second_row);
    }

    #[test]
    fn render_puts_wrapped_words_on_separate_rows() {
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).expect("test terminal");
        let body = "alpha beta gamma delta epsilon zeta eta theta iota kappa";
        terminal
            .draw(|frame| render(frame, frame.area(), body, body.len(), None, false))
            .expect("comment input must render");

        let buffer = terminal.backend().buffer();
        let rows: Vec<String> = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect();
        let comment_rows: Vec<&str> = rows
            .iter()
            .map(String::as_str)
            .filter(|row| {
                row.contains("alpha")
                    || row.contains("kappa")
                    || row.contains("epsilon")
                    || row.contains("theta")
            })
            .collect();

        assert!(
            comment_rows.len() >= 2,
            "the rendered popup must wrap the long comment onto multiple rows, got {rows:?}"
        );
        assert!(
            rows.iter().any(|row| row.contains("Comment")),
            "the rendered popup must keep its title: {rows:?}"
        );
    }

    #[test]
    fn popup_sits_below_the_selected_line_when_there_is_room() {
        let screen = Rect::new(0, 0, 80, 24);
        let selected = Rect::new(1, 4, 78, 1);
        let area = place_comment_popup(screen, Some(selected), 36, 3);
        let centered = place_comment_popup(screen, None, 36, 3);

        assert_eq!(area.y, 5, "the popup must open on the row under the line");
        assert_eq!(area.x, 3, "the popup must indent from the selected line");
        assert_ne!(
            area.y, centered.y,
            "anchoring must not fall back to the screen center when the line has room below it"
        );
    }

    #[test]
    fn popup_sits_above_the_selected_line_near_the_bottom() {
        let screen = Rect::new(0, 0, 80, 24);
        let selected = Rect::new(1, 22, 78, 1);
        let area = place_comment_popup(screen, Some(selected), 36, 3);

        assert_eq!(
            area.y, 19,
            "a line near the bottom must open the popup above it: {area:?}"
        );
        assert_eq!(area.y + area.height, selected.y);
    }

    #[test]
    fn layout_follows_the_selected_line_instead_of_the_screen_center() {
        let screen = Rect::new(0, 0, 80, 24);
        let selected = Rect::new(1, 3, 78, 1);
        let anchored = layout_at_end(screen, "note", Some(selected));
        let centered = layout_at_end(screen, "note", None);

        assert_eq!(anchored.area.y, 4);
        assert_eq!(centered.area.y, 10);
        assert_ne!(anchored.area.y, centered.area.y);
    }
}
