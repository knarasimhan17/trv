use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::diff::{DiffFile, FileChangeKind};

use super::diff_view::HIGHLIGHT_SYMBOL;
use super::{App, DiffListLayout};

pub(super) fn panel_width(total: u16) -> u16 {
    let preferred = (total / 3).clamp(16, 28);
    let reserved_for_diff = total.saturating_sub(40).max(16);
    preferred
        .min(reserved_for_diff)
        .min(total.saturating_sub(12))
}

pub(super) fn contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

pub(super) fn render(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    let title = format!(" files | {} ", app.diff.files.len());
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    let line_width =
        usize::from(inner.width).saturating_sub(UnicodeWidthStr::width(HIGHLIGHT_SYMBOL));
    let items = if app.diff.files.is_empty() {
        vec![ListItem::new("No files.")]
    } else {
        app.diff
            .files
            .iter()
            .map(|file| ListItem::new(file_row(file, line_width)))
            .collect()
    };
    let item_heights: Vec<u16> = items.iter().map(|item| item.height() as u16).collect();
    let list = List::new(items)
        .block(block)
        .highlight_symbol(HIGHLIGHT_SYMBOL)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default().with_offset(app.file_tree_list.offset);
    if !app.diff.files.is_empty() {
        state.select(Some(app.selected_file.min(app.diff.files.len() - 1)));
    }
    frame.render_stateful_widget(list, area, &mut state);
    app.file_tree_area = area;
    app.file_tree_list = DiffListLayout {
        inner,
        offset: state.offset(),
        heights: item_heights,
        line_width,
    };
}

fn file_row(file: &DiffFile, width: usize) -> Line<'static> {
    let kind = kind_letter(file.change_kind);
    let stats = format!("+{} -{}", file.additions, file.deletions);
    let path = file.display_path();
    let kind_style = kind_style(file.change_kind);
    let stats_width = UnicodeWidthStr::width(stats.as_str());
    let prefix = 1usize
        .saturating_add(1)
        .saturating_add(stats_width)
        .saturating_add(1);
    let (show_stats, path_width) = if width > prefix {
        (true, width - prefix)
    } else if width > 2 {
        (false, width - 2)
    } else {
        (false, 0)
    };
    let path = truncate_left(&path, path_width);
    let mut spans = vec![Span::styled(kind.to_string(), kind_style)];
    if show_stats {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(stats, Style::default().fg(Color::Gray)));
    }
    if !path.is_empty() {
        spans.push(Span::raw(" "));
        spans.push(Span::raw(path));
    }
    Line::from(spans)
}

fn kind_letter(kind: FileChangeKind) -> char {
    match kind {
        FileChangeKind::Modified => 'M',
        FileChangeKind::Added => 'A',
        FileChangeKind::Deleted => 'D',
        FileChangeKind::Renamed => 'R',
    }
}

fn kind_style(kind: FileChangeKind) -> Style {
    match kind {
        FileChangeKind::Modified => Style::default().fg(Color::Cyan),
        FileChangeKind::Added => Style::default().fg(Color::Green),
        FileChangeKind::Deleted => Style::default().fg(Color::Red),
        FileChangeKind::Renamed => Style::default().fg(Color::Yellow),
    }
}

fn truncate_left(text: &str, max_width: usize) -> String {
    const ELLIPSIS: &str = "...";

    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_owned();
    }
    if max_width <= ELLIPSIS.len() {
        return ".".repeat(max_width);
    }

    let budget = max_width - ELLIPSIS.len();
    let mut taken = String::new();
    let mut width = 0;
    for character in text.chars().rev() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if width + character_width > budget {
            break;
        }
        taken.push(character);
        width += character_width;
    }
    let suffix: String = taken.chars().rev().collect();
    format!("{ELLIPSIS}{suffix}")
}
