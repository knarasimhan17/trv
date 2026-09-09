use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, List, ListItem, ListState};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::model::Comment;

use super::App;
use super::diff_view::{HIGHLIGHT_SYMBOL, item_index_at};

pub(super) const MAX_INNER_ROWS: u16 = 4;
pub(super) const MIN_DIFF_ROWS: u16 = 8;

pub(super) fn height(comment_count: usize, available: u16) -> u16 {
    if comment_count == 0 || available < 3 {
        return 0;
    }
    let inner = (comment_count as u16).clamp(1, MAX_INNER_ROWS);
    let desired = inner.saturating_add(2);
    let max_height = available
        .saturating_sub(MIN_DIFF_ROWS)
        .max(3)
        .min(available);
    desired.min(max_height)
}

pub(super) fn render(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    let title = if app.panel_focused {
        format!(" comments | {} | focused ", app.comments.len())
    } else {
        format!(" comments | {} ", app.comments.len())
    };
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    let line_width =
        usize::from(inner.width).saturating_sub(UnicodeWidthStr::width(HIGHLIGHT_SYMBOL));
    let items = app
        .comments
        .iter()
        .map(|comment| ListItem::new(panel_row(comment, line_width)))
        .collect::<Vec<_>>();
    let item_heights: Vec<u16> = items.iter().map(|item| item.height() as u16).collect();
    let mut list = List::new(items)
        .block(block)
        .highlight_symbol(HIGHLIGHT_SYMBOL);
    list = if app.panel_focused {
        list.highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        list.highlight_style(Style::default().bg(Color::DarkGray))
    };
    let mut state = ListState::default().with_offset(app.comments_panel.offset);
    if !app.comments.is_empty() {
        state.select(Some(app.selected_comment.min(app.comments.len() - 1)));
    }
    frame.render_stateful_widget(list, area, &mut state);
    app.comments_panel.area = area;
    app.comments_panel.inner = inner;
    app.comments_panel.offset = state.offset();
    app.comments_panel.heights = item_heights;
}

pub(super) fn contains_pointer(app: &App, column: u16, row: u16) -> bool {
    !app.comments.is_empty()
        && app
            .comments_panel
            .area
            .contains(Position { x: column, y: row })
}

pub(super) fn select_at_pointer(app: &mut App, column: u16, row: u16) -> bool {
    if !contains_pointer(app, column, row) {
        return false;
    }
    if let Some(index) = item_index_at(
        app.comments_panel.inner,
        &app.comments_panel.heights,
        app.comments_panel.offset,
        row,
    ) && index < app.comments.len()
    {
        app.panel_focused = true;
        app.jump_to_comment(index);
    } else {
        app.panel_focused = true;
    }
    true
}

fn panel_row(comment: &Comment, width: usize) -> String {
    let location = format!("{} [{}]", comment.location(), comment.side.as_str());
    if width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(location.as_str()) >= width {
        return truncate(&location, width);
    }
    let snippet = snippet(&comment.body);
    if snippet.is_empty() {
        return location;
    }
    let rest = width
        .saturating_sub(UnicodeWidthStr::width(location.as_str()))
        .saturating_sub(1);
    if rest == 0 {
        location
    } else {
        format!("{location} {}", truncate(&snippet, rest))
    }
}

fn snippet(body: &str) -> String {
    body.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_owned();
    }
    let target = width.saturating_sub(1);
    let mut used = 0;
    let mut end = 0;
    for (index, character) in text.char_indices() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > target {
            break;
        }
        used += character_width;
        end = index + character.len_utf8();
    }
    let mut truncated = text[..end].to_owned();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::{height, panel_row, snippet, truncate};
    use crate::model::{Comment, Side};

    #[test]
    fn height_hides_the_panel_when_there_are_no_comments() {
        assert_eq!(height(0, 23), 0);
        assert_eq!(height(0, 3), 0);
    }

    #[test]
    fn height_stays_small_on_an_80x24_class_terminal() {
        assert_eq!(height(1, 23), 3);
        assert_eq!(height(2, 23), 4);
        assert_eq!(height(4, 23), 6);
        assert_eq!(height(20, 23), 6, "the panel must cap inner rows");
        assert!(
            height(20, 23) + 8 <= 23,
            "the diff must keep a readable number of rows"
        );
    }

    #[test]
    fn height_still_fits_a_tiny_terminal() {
        assert_eq!(height(3, 10), 3);
        assert_eq!(height(1, 2), 0);
    }

    #[test]
    fn panel_rows_show_location_and_a_flattened_body_snippet() {
        let comment = Comment::range(
            "src/lib.rs".to_owned(),
            12,
            18,
            Side::New,
            "handle the\nerror  here".to_owned(),
        );
        assert_eq!(snippet(&comment.body), "handle the error here");
        assert_eq!(
            panel_row(&comment, 80),
            "src/lib.rs:12-18 [new] handle the error here"
        );
        let truncated = panel_row(&comment, 28);
        assert!(
            truncated.starts_with("src/lib.rs:12-18 [new]"),
            "{truncated}"
        );
        assert!(truncated.ends_with('…'), "{truncated}");
        assert!(truncated.len() <= 28 + "…".len(), "{truncated}");
    }

    #[test]
    fn truncate_keeps_short_text_and_ellipsizes_long_text() {
        assert_eq!(truncate("abc", 8), "abc");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("a", 1), "a");
        assert_eq!(truncate("ab", 1), "…");
    }
}
