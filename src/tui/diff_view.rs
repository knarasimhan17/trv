mod side_by_side;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::diff::{DiffFile, DiffLine, DiffLineKind};
use crate::model::Side;

use super::{App, DiffLayout, DiffRow};

pub(super) const HIGHLIGHT_SYMBOL: &str = "> ";
pub(super) const COMMENT_PREFIX: &str = "┃ ";

pub(super) fn render(frame: &mut Frame<'_>, area: Rect, app: &mut App) -> Option<Rect> {
    let block = Block::bordered().title(format!(
        " trv | files changed | {} | {} comments ",
        app.diff_layout.as_str(),
        app.comments.len()
    ));
    let inner = block.inner(area);
    let line_width =
        usize::from(inner.width).saturating_sub(UnicodeWidthStr::width(HIGHLIGHT_SYMBOL));
    let rows = app.diff_rows();
    let items = if rows.is_empty() {
        vec![ListItem::new("No changes to review.")]
    } else {
        rows.iter()
            .enumerate()
            .map(|(index, row)| match *row {
                DiffRow::File(file) => ListItem::new(file_header(
                    &app.diff.files[file],
                    line_width,
                    app.collapsed_files[file],
                )),
                DiffRow::Line { file, line } => ListItem::new(item_lines(
                    app,
                    &app.diff.files[file].lines[line],
                    line_width,
                    app.visual_covers_index(index),
                )),
                DiffRow::SideBySide { file, row } => ListItem::new(side_by_side::item_lines(
                    app,
                    &app.diff.files[file],
                    row,
                    line_width,
                    index == app.selected_diff,
                )),
            })
            .collect::<Vec<_>>()
    };
    let item_heights: Vec<u16> = items.iter().map(|item| item.height() as u16).collect();
    let list = List::new(items)
        .block(block)
        .highlight_symbol(HIGHLIGHT_SYMBOL);
    let list = if app.diff_layout == DiffLayout::Unified {
        list.highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        list
    };
    let mut state = ListState::default().with_offset(app.diff_list.offset);
    if !rows.is_empty() {
        state.select(Some(app.selected_diff));
    }
    frame.render_stateful_widget(list, area, &mut state);
    app.diff_list.inner = inner;
    app.diff_list.offset = state.offset();
    app.diff_list.heights = item_heights;
    app.diff_list.line_width = line_width;
    selected_line_rect(
        inner,
        &app.diff_list.heights,
        app.selected_diff,
        app.diff_list.offset,
    )
}

pub(super) fn item_index_at(
    list_area: Rect,
    heights: &[u16],
    offset: usize,
    row: u16,
) -> Option<usize> {
    if list_area.height == 0 || row < list_area.y || row >= list_area.bottom() {
        return None;
    }

    let mut y = list_area.y;
    for (index, height) in heights.iter().copied().enumerate().skip(offset) {
        if y >= list_area.bottom() {
            return None;
        }
        let next = y.saturating_add(height).min(list_area.bottom());
        if row >= y && row < next {
            return Some(index);
        }
        y = next;
    }
    None
}

pub(super) fn side_at_column(list_area: Rect, column: u16) -> Side {
    side_by_side::side_at_column(list_area, column)
}

pub(super) fn item_top(
    list_area: Rect,
    heights: &[u16],
    offset: usize,
    index: usize,
) -> Option<u16> {
    if list_area.height == 0 || index < offset || index >= heights.len() {
        return None;
    }
    let mut y = list_area.y;
    for (item, height) in heights.iter().copied().enumerate().skip(offset) {
        if y >= list_area.bottom() {
            return None;
        }
        if item == index {
            return Some(y);
        }
        y = y.saturating_add(height);
    }
    None
}

pub(super) fn comment_index_at(app: &App, column: u16, row: u16) -> Option<usize> {
    if !app.inline_comments {
        return None;
    }
    let index = item_index_at(
        app.diff_list.inner,
        &app.diff_list.heights,
        app.diff_list.offset,
        row,
    )?;
    let top = item_top(
        app.diff_list.inner,
        &app.diff_list.heights,
        app.diff_list.offset,
        index,
    )?;
    let local = usize::from(row.saturating_sub(top));
    if local == 0 {
        return None;
    }
    let diff_row = *app.diff_rows().get(index)?;
    match diff_row {
        DiffRow::File(_) => None,
        DiffRow::Line { file, line } => comment_at_unified_offset(app, file, line, local),
        DiffRow::SideBySide { file, row } => comment_at_side_offset(
            app,
            file,
            row,
            side_at_column(app.diff_list.inner, column),
            local,
        ),
    }
}

fn comment_at_unified_offset(app: &App, file: usize, line: usize, local: usize) -> Option<usize> {
    let line = app.diff.files.get(file)?.lines.get(line)?;
    let body_width = app
        .diff_list
        .line_width
        .saturating_sub(UnicodeWidthStr::width(COMMENT_PREFIX))
        .max(1);
    comment_at_wrapped_offset(
        app.comment_indices_for_line(line),
        app,
        body_width,
        local - 1,
    )
}

fn comment_at_side_offset(
    app: &App,
    file: usize,
    row: crate::diff::SideBySideRow,
    side: Side,
    local: usize,
) -> Option<usize> {
    let line = match side {
        Side::Old => row.old_line(),
        Side::New => row.new_line(),
    }?;
    let file = app.diff.files.get(file)?;
    let (left, right, _) = side_by_side::column_widths(app.diff_list.line_width);
    let width = match side {
        Side::Old => left,
        Side::New => right,
    };
    let body_width = width
        .saturating_sub(UnicodeWidthStr::width(COMMENT_PREFIX))
        .max(1);
    comment_at_wrapped_offset(
        app.comment_indices_for_anchor(file.lines.get(line)?.anchor_on(side)),
        app,
        body_width,
        local - 1,
    )
}

fn comment_at_wrapped_offset(
    indices: impl Iterator<Item = usize>,
    app: &App,
    body_width: usize,
    mut remaining: usize,
) -> Option<usize> {
    for index in indices {
        let wraps = wrap_comment_body(&app.comments.get(index)?.body, body_width)
            .len()
            .max(1);
        if remaining < wraps {
            return Some(index);
        }
        remaining -= wraps;
    }
    None
}

pub(super) fn selected_line_rect(
    list_area: Rect,
    heights: &[u16],
    selected: usize,
    offset: usize,
) -> Option<Rect> {
    if list_area.height == 0 || selected < offset || selected >= heights.len() {
        return None;
    }

    let mut y = list_area.y;
    for (index, height) in heights.iter().copied().enumerate().skip(offset) {
        if y >= list_area.bottom() {
            return None;
        }
        if index == selected {
            let visible = 1u16.min(height).min(list_area.bottom().saturating_sub(y));
            if visible == 0 {
                return None;
            }
            return Some(Rect {
                x: list_area.x,
                y,
                width: list_area.width,
                height: visible,
            });
        }
        y = y.saturating_add(height);
    }
    None
}

fn file_header(file: &DiffFile, width: usize, collapsed: bool) -> Line<'static> {
    let marker = if collapsed { "▸" } else { "▾" };
    let path = format!(" {marker} {} ", file.display_path());
    let kind = format!(" {} ", file.change_kind.as_str());
    let additions = format!(" +{}", file.additions);
    let deletions = format!(" -{} ", file.deletions);
    let used_width = [&path, &kind, &additions, &deletions]
        .into_iter()
        .map(|text| UnicodeWidthStr::width(text.as_str()))
        .sum::<usize>();
    let padding = " ".repeat(width.saturating_sub(used_width));
    let background = Style::default().bg(Color::DarkGray);

    Line::from(vec![
        Span::styled(
            path,
            background.fg(Color::White).add_modifier(Modifier::BOLD),
        ),
        Span::styled(kind, background.fg(Color::Cyan)),
        Span::styled(additions, background.fg(Color::Green)),
        Span::styled(deletions, background.fg(Color::Red)),
        Span::styled(padding, background),
    ])
}

fn item_lines(app: &App, line: &DiffLine, width: usize, in_visual: bool) -> Vec<Line<'static>> {
    let covering = app.comments_for_line(line).collect::<Vec<_>>();
    let comments = app.comments_ending_on_line(line).collect::<Vec<_>>();
    let marker = if covering.is_empty() { " " } else { "●" };
    let old_line = line
        .old_line
        .map(|line| line.to_string())
        .unwrap_or_default();
    let new_line = line
        .new_line
        .map(|line| line.to_string())
        .unwrap_or_default();
    let gutter = format!("{marker} {old_line:>5} {new_line:>5} ");
    let mut style = line_style(line.kind);
    if in_visual {
        style = style.bg(Color::DarkGray);
    }
    let mut rows = vec![Line::from(vec![
        Span::styled(gutter, Style::default().fg(Color::DarkGray)),
        Span::styled(display_text(line), style),
    ])];

    if app.inline_comments {
        let body_width = width
            .saturating_sub(UnicodeWidthStr::width(COMMENT_PREFIX))
            .max(1);
        let comment_style = Style::default().add_modifier(Modifier::DIM | Modifier::REVERSED);
        for comment in comments {
            rows.extend(
                wrap_comment_body(&comment.body, body_width)
                    .into_iter()
                    .map(|body| Line::styled(format!("{COMMENT_PREFIX}{body}"), comment_style)),
            );
        }
    }

    rows
}

fn display_text(line: &DiffLine) -> String {
    match line.kind {
        DiffLineKind::Hunk if line.text.is_empty() => "──".to_owned(),
        DiffLineKind::Hunk => format!("── {}", line.text),
        DiffLineKind::Addition => format!("+{}", line.text),
        DiffLineKind::Deletion => format!("-{}", line.text),
        DiffLineKind::Context => format!(" {}", line.text),
        DiffLineKind::Meta => line.text.clone(),
    }
}

pub(super) fn wrap_comment_body(body: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();

    for source_line in body.split('\n') {
        let mut remaining = source_line;
        if remaining.is_empty() {
            rows.push(String::new());
            continue;
        }

        while UnicodeWidthStr::width(remaining) > width {
            let hard_break = byte_index_at_width(remaining, width);
            let soft_break = remaining[..hard_break]
                .rfind(|character: char| character.is_whitespace())
                .filter(|index| *index > 0);
            let split = soft_break.unwrap_or(hard_break);
            rows.push(remaining[..split].trim_end().to_owned());
            remaining = remaining[split..].trim_start();
        }

        if !remaining.is_empty() {
            rows.push(remaining.to_owned());
        }
    }

    rows
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

fn line_style(kind: DiffLineKind) -> Style {
    match kind {
        DiffLineKind::Hunk => Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
        DiffLineKind::Addition => Style::default().fg(Color::Green),
        DiffLineKind::Deletion => Style::default().fg(Color::Red),
        DiffLineKind::Context => Style::default(),
        DiffLineKind::Meta => Style::default().fg(Color::DarkGray),
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::diff::{DiffLineKind, ParsedDiff};

    use super::super::Mode;
    use super::{App, item_index_at, item_lines, selected_line_rect, wrap_comment_body};
    use ratatui::layout::Rect;

    // Ten body columns force both word-boundary and hard-word wrapping.
    const NARROW_WIDTH: usize = 12;
    const DIFF: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1 @@
-old
+new
";

    #[test]
    fn inline_comments_follow_their_diff_line_and_wrap() {
        let mut app = App::new(ParsedDiff::parse(DIFF));
        let selected = app.diff.files[0]
            .lines
            .iter()
            .position(|line| line.kind == DiffLineKind::Addition)
            .expect("the fixture must contain an added line");
        app.selected_diff = selected + 1;
        app.start_comment();
        let Mode::CommentInput { body, .. } = &mut app.mode else {
            panic!("commenting on an added line must open the input");
        };
        body.push_str("first exceptionallylong\n\nnext");
        app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let visible = item_lines(
            &app,
            &app.diff.files[0].lines[selected],
            NARROW_WIDTH,
            false,
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
        assert_eq!(
            visible,
            [
                "●           1 +new",
                "┃ first",
                "┃ exceptiona",
                "┃ llylong",
                "┃ ",
                "┃ next",
            ],
            "inline rows must follow the anchored diff line and fit the available width"
        );
        assert_eq!(
            wrap_comment_body("界", 1),
            ["界"],
            "wrapping a wide leading character must still make progress"
        );

        app.inline_comments = false;
        let hidden = item_lines(
            &app,
            &app.diff.files[0].lines[selected],
            NARROW_WIDTH,
            false,
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
        assert_eq!(
            hidden,
            ["●           1 +new"],
            "hiding inline bodies must retain the gutter marker"
        );
    }

    #[test]
    fn range_comments_mark_every_covered_line_and_show_the_body_on_the_last() {
        let mut app = App::new(ParsedDiff::parse(
            "\
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,2 @@
 first
-old
+second
",
        ));

        let first = app.diff.files[0]
            .lines
            .iter()
            .position(|line| line.text == "first")
            .expect("fixture must contain the first context line");
        let second = app.diff.files[0]
            .lines
            .iter()
            .position(|line| line.text == "second")
            .expect("fixture must contain the added line");
        let path = app.diff.files[0].lines[first]
            .anchor_on(crate::model::Side::New)
            .expect("the context line must have a new-side anchor")
            .path
            .clone();
        app.comments.push(crate::model::Comment::range(
            path,
            1,
            2,
            crate::model::Side::New,
            "extract both lines".to_owned(),
        ));

        let start_rows = item_lines(&app, &app.diff.files[0].lines[first], NARROW_WIDTH, false)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        let end_rows = item_lines(&app, &app.diff.files[0].lines[second], NARROW_WIDTH, false)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert_eq!(
            start_rows,
            ["●     1     1  first"],
            "covered lines before the end of a range must keep a marker without repeating the body"
        );
        assert_eq!(
            end_rows,
            ["●           2 +second", "┃ extract", "┃ both lines"],
            "the last line of a range must show the wrapped comment body"
        );
    }

    #[test]
    fn selected_line_rect_uses_the_first_visible_row_of_the_item() {
        let area = Rect::new(1, 2, 40, 10);
        let selected = selected_line_rect(area, &[1, 3, 1], 1, 0)
            .expect("the selected item must be on screen");

        assert_eq!(
            selected,
            Rect::new(1, 3, 40, 1),
            "the popup anchor must be the code row, not the full multi-line item"
        );
        assert_eq!(
            selected_line_rect(area, &[1, 3, 1], 1, 2),
            None,
            "a selected item scrolled above the viewport must not produce an anchor"
        );
    }

    #[test]
    fn item_index_at_maps_a_screen_row_onto_the_visible_item() {
        let area = Rect::new(1, 2, 40, 8);
        let heights = [1, 3, 1, 1];

        assert_eq!(item_index_at(area, &heights, 0, 2), Some(0));
        assert_eq!(
            item_index_at(area, &heights, 0, 4),
            Some(1),
            "clicks on wrapped comment rows must still select that diff item"
        );
        assert_eq!(item_index_at(area, &heights, 0, 5), Some(1));
        assert_eq!(item_index_at(area, &heights, 0, 6), Some(2));
        assert_eq!(item_index_at(area, &heights, 1, 2), Some(1));
        assert_eq!(
            item_index_at(area, &heights, 0, 1),
            None,
            "clicks on the list border must not select a diff row"
        );
    }
}
