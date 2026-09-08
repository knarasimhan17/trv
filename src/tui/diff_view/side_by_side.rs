use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::diff::{DiffFile, DiffLineKind, SideBySideRow};
use crate::model::Side;

use super::super::App;
use super::{display_text, line_style, wrap_comment_body};

use super::COMMENT_PREFIX;

const DIVIDER: &str = "│";

pub(super) fn item_lines(
    app: &App,
    file: &DiffFile,
    row: SideBySideRow,
    width: usize,
    selected: bool,
) -> Vec<Line<'static>> {
    match row {
        SideBySideRow::Hunk(line) | SideBySideRow::Meta(line) => {
            let line = &file.lines[line];
            vec![Line::styled(
                fit_width(&display_text(line), width),
                line_style(line.kind),
            )]
        }
        SideBySideRow::Paired { .. } | SideBySideRow::Old(_) | SideBySideRow::New(_) => {
            paired_lines(app, file, row.old_line(), row.new_line(), width, selected)
        }
    }
}

fn paired_lines(
    app: &App,
    file: &DiffFile,
    old: Option<usize>,
    new: Option<usize>,
    width: usize,
    selected: bool,
) -> Vec<Line<'static>> {
    let (left_width, right_width, divider) = column_widths(width);
    let mut rows = vec![split_line(
        main_cell(
            app,
            file,
            old,
            Side::Old,
            left_width,
            selected && app.selected_side == Side::Old,
        ),
        main_cell(
            app,
            file,
            new,
            Side::New,
            right_width,
            selected && app.selected_side == Side::New,
        ),
        divider,
    )];

    if app.inline_comments {
        let left_comments = comment_rows(app, file, old, Side::Old, left_width);
        let right_comments = comment_rows(app, file, new, Side::New, right_width);
        let comment_rows = left_comments.len().max(right_comments.len());
        rows.extend((0..comment_rows).map(|index| {
            split_line(
                comment_cell(left_comments.get(index), left_width),
                comment_cell(right_comments.get(index), right_width),
                divider,
            )
        }));
    }

    rows
}

fn main_cell(
    app: &App,
    file: &DiffFile,
    line: Option<usize>,
    side: Side,
    width: usize,
    active: bool,
) -> Span<'static> {
    let Some(line) = line else {
        return Span::raw(" ".repeat(width));
    };
    let line = &file.lines[line];
    let marker = if app
        .comments_for_anchor(line.anchor_on(side))
        .next()
        .is_some()
    {
        "●"
    } else {
        " "
    };
    let line_number = match side {
        Side::Old => line.old_line,
        Side::New => line.new_line,
    }
    .map(|line| line.to_string())
    .unwrap_or_default();
    let prefix = match (side, line.kind) {
        (Side::Old, DiffLineKind::Deletion) => "-",
        (Side::New, DiffLineKind::Addition) => "+",
        (
            Side::Old | Side::New,
            DiffLineKind::Hunk | DiffLineKind::Context | DiffLineKind::Meta,
        ) => " ",
        (Side::Old, DiffLineKind::Addition) | (Side::New, DiffLineKind::Deletion) => " ",
    };
    let cursor = if active { "▌" } else { " " };
    let text = format!("{cursor}{marker} {line_number:>5} {prefix}{}", line.text);
    let mut style = line_style(line.kind);
    if active || app.visual_covers_anchor(line.anchor_on(side)) {
        style = style.bg(Color::DarkGray);
    }
    if active {
        style = style.add_modifier(Modifier::BOLD);
    }
    Span::styled(fit_width(&text, width), style)
}

fn comment_rows(
    app: &App,
    file: &DiffFile,
    line: Option<usize>,
    side: Side,
    width: usize,
) -> Vec<String> {
    let Some(line) = line else {
        return Vec::new();
    };
    let anchor = file.lines[line].anchor_on(side);
    let body_width = width
        .saturating_sub(UnicodeWidthStr::width(COMMENT_PREFIX))
        .max(1);

    app.comments_ending_on_anchor(anchor)
        .flat_map(|comment| wrap_comment_body(&comment.body, body_width))
        .map(|body| format!("{COMMENT_PREFIX}{body}"))
        .collect()
}

fn comment_cell(comment: Option<&String>, width: usize) -> Span<'static> {
    match comment {
        Some(comment) => Span::styled(
            fit_width(comment, width),
            Style::default().add_modifier(Modifier::DIM | Modifier::REVERSED),
        ),
        None => Span::raw(" ".repeat(width)),
    }
}

fn split_line(left: Span<'static>, right: Span<'static>, divider: bool) -> Line<'static> {
    let mut spans = vec![left];
    if divider {
        spans.push(Span::styled(DIVIDER, Style::default().fg(Color::DarkGray)));
    }
    spans.push(right);
    Line::from(spans)
}

pub(super) fn column_widths(width: usize) -> (usize, usize, bool) {
    let divider = width > 0;
    let content_width = width.saturating_sub(usize::from(divider));
    let left = content_width / 2;
    (left, content_width - left, divider)
}

pub(super) fn side_at_column(list_area: Rect, column: u16) -> Side {
    let highlight = UnicodeWidthStr::width(super::HIGHLIGHT_SYMBOL) as u16;
    let content_x = list_area.x.saturating_add(highlight);
    let content_width = list_area.width.saturating_sub(highlight);
    let (left, _, _) = column_widths(usize::from(content_width));
    if column < content_x.saturating_add(left as u16) {
        Side::Old
    } else {
        Side::New
    }
}

fn fit_width(text: &str, width: usize) -> String {
    let mut end = 0;
    let mut used = 0;
    for (index, character) in text.char_indices() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > width {
            break;
        }
        used += character_width;
        end = index + character.len_utf8();
    }

    let mut fitted = text[..end].to_owned();
    fitted.push_str(&" ".repeat(width.saturating_sub(used)));
    fitted
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    use crate::diff::ParsedDiff;
    use crate::model::{Comment, Side};

    use super::super::super::App;
    use super::{item_lines, side_at_column};
    use ratatui::layout::Rect;

    const DIFF: &str = "\
diff --git file.rs file.rs
--- file.rs
+++ file.rs
@@ -1 +1 @@
-old
+new
";

    #[test]
    fn side_at_column_splits_on_the_old_and_new_panes() {
        let area = Rect::new(0, 0, 42, 10);
        assert_eq!(
            side_at_column(area, 4),
            Side::Old,
            "clicks in the left pane must select the old side"
        );
        assert_eq!(
            side_at_column(area, 30),
            Side::New,
            "clicks in the right pane must select the new side"
        );
    }

    #[test]
    fn inline_comments_render_on_both_sides_and_leave_gutter_markers() {
        // One divider plus two 20-column panes exercises exact split sizing.
        const WIDTH: usize = 41;

        let mut app = App::new(ParsedDiff::parse(DIFF));
        app.comments.push(Comment::open(
            "file.rs".to_owned(),
            1,
            Side::Old,
            "left note".to_owned(),
        ));
        app.comments.push(Comment::open(
            "file.rs".to_owned(),
            1,
            Side::New,
            "right note".to_owned(),
        ));
        let row = app.diff.files[0].side_by_side_rows()[1];

        let visible = item_lines(&app, &app.diff.files[0], row, WIDTH, true)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            visible.len(),
            2,
            "both side comments must share one aligned inline row"
        );
        assert_eq!(
            UnicodeWidthStr::width(visible[0].as_str()),
            WIDTH,
            "paired code rows must retain the requested terminal width"
        );
        assert!(
            visible[0].contains("-old")
                && visible[0].contains("+new")
                && visible[0].matches('●').count() == 2,
            "each side must render its code and comment gutter marker"
        );
        assert!(
            visible[1].contains("┃ left note") && visible[1].contains("┃ right note"),
            "inline comment bodies must render beneath their anchored side"
        );

        app.inline_comments = false;
        let hidden = item_lines(&app, &app.diff.files[0], row, WIDTH, true)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        assert!(
            hidden.len() == 1 && hidden[0].matches('●').count() == 2,
            "hiding inline bodies must retain both side gutter markers"
        );
    }
}
