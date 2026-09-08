use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

use crate::diff::{ParsedDiff, SideBySideRow};
use crate::model::{Comment, Side};
use crate::session::{FrozenReview, LiveReview, ReviewSession, ViewKind};

use super::{App, DiffLayout, DiffRow, Mode, ReviewOutcome, dock_bottom, footer_text, render};

const LIVE_DIFF: &str = "\
diff --git file.rs file.rs
--- file.rs
+++ file.rs
@@ -1 +1 @@
-old
+live
";
const FROZEN_DIFF: &str = "\
diff --git file.rs file.rs
--- file.rs
+++ file.rs
@@ -1 +1 @@
-old
+frozen
";
const SINCE_DIFF: &str = "\
diff --git file.rs file.rs
--- file.rs
+++ file.rs
@@ -1 +1 @@
-frozen
+live
";

fn two_round_session() -> ReviewSession {
    ReviewSession {
        live: Some(LiveReview {
            vs_main: ParsedDiff::parse(LIVE_DIFF),
            vs_previous: Some((1, ParsedDiff::parse(SINCE_DIFF))),
            comments: Vec::new(),
        }),
        frozen: vec![FrozenReview {
            rev: 1,
            diff: ParsedDiff::parse(FROZEN_DIFF),
            comments: vec![Comment::open(
                "file.rs".to_owned(),
                1,
                Side::New,
                "from rev-1".to_owned(),
            )],
        }],
        initial: ViewKind::LiveMain,
    }
}

fn click(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn buffer_rows(buffer: &Buffer) -> Vec<String> {
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect()
        })
        .collect()
}

const TWO_FILE_DIFF: &str = "\
diff --git first.rs first.rs
--- first.rs
+++ first.rs
@@ -1 +1 @@
-old
+new
diff --git second.rs second.rs
--- second.rs
+++ second.rs
@@ -1 +1 @@
-before
+after
";
const SIDE_COMMENT_DIFF: &str = "\
diff --git file.rs file.rs
--- file.rs
+++ file.rs
@@ -1,2 +1,2 @@
 shared
-old
+new
";

#[test]
fn inline_comment_toggle_preserves_the_selected_diff_line() {
    let mut app = App::new(ParsedDiff::parse(
        "\
diff --git file.rs file.rs
--- file.rs
+++ file.rs
@@ -1 +1 @@
-first
+second
",
    ));
    app.selected_diff = 1;
    let selected = app.selected_diff;

    assert!(app.inline_comments, "inline comments must start visible");
    assert!(
        footer_text(&app).contains("i inline comments: on"),
        "the footer must document the toggle and its state"
    );
    assert!(
        footer_text(&app).contains("? help"),
        "the review footer must make help discoverable"
    );

    app.handle_diff_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));

    assert!(!app.inline_comments, "i must hide inline comment bodies");
    assert_eq!(
        app.selected_diff, selected,
        "toggling inline comments must preserve the selected diff line"
    );
}

#[test]
fn help_closes_without_changing_review_state() {
    let mut app = App::new(ParsedDiff::parse(TWO_FILE_DIFF));
    app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    app.select_diff(2);
    let selected_diff = app.selected_diff;
    let collapsed_files = app.collapsed_files.clone();

    for close in [KeyCode::Char('?'), KeyCode::Esc, KeyCode::Char('q')] {
        app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(app.help, "? must open help in the review");
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        let outcome = app.handle_key(KeyEvent::new(close, KeyModifiers::NONE));

        assert!(outcome.is_none(), "closing help must not exit the review");
        assert!(!app.help, "a help close key must dismiss the overlay");
        assert_eq!(
            app.selected_diff, selected_diff,
            "help must preserve the selected diff row"
        );
        assert_eq!(
            app.collapsed_files, collapsed_files,
            "help must preserve file collapse state"
        );
        assert_eq!(
            app.diff_layout,
            DiffLayout::SideBySide,
            "help must preserve the active review layout"
        );
    }

    app.mode = Mode::Comments;
    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    let outcome = app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));

    assert!(
        outcome.is_none(),
        "q must close help instead of quitting from the comment list"
    );
    assert!(
        matches!(app.mode, Mode::Comments),
        "closing help must return to the comment list"
    );
}

#[test]
fn file_headers_toggle_bodies_and_remain_navigation_targets() {
    let mut app = App::new(ParsedDiff::parse(TWO_FILE_DIFF));
    let expanded_rows = app.diff_rows();

    app.handle_diff_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(
        app.diff_rows(),
        [
            DiffRow::File(0),
            DiffRow::File(1),
            DiffRow::Line { file: 1, line: 0 },
            DiffRow::Line { file: 1, line: 1 },
            DiffRow::Line { file: 1, line: 2 },
        ],
        "collapsing a header must hide only that file's body"
    );

    app.handle_diff_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
    assert_eq!(
        app.selected_row(),
        Some(DiffRow::File(1)),
        "next-file navigation must still target collapsed section headers"
    );
    app.handle_diff_key(KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE));
    app.handle_diff_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert_eq!(
        app.diff_rows(),
        expanded_rows,
        "Tab on a header must expand the selected file without changing views"
    );

    app.select_diff(1);
    app.handle_diff_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert!(
        matches!(app.mode, Mode::Comments),
        "Tab on a code row must retain the existing comments-view shortcut"
    );
}

#[test]
fn side_by_side_comments_follow_the_active_column() {
    let mut app = App::new(ParsedDiff::parse(SIDE_COMMENT_DIFF));
    app.handle_diff_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    app.select_diff(2);

    app.handle_diff_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    app.start_comment();
    let Mode::CommentInput { body, .. } = &mut app.mode else {
        panic!("the old side of a context row must accept comments");
    };
    body.push_str("old side");
    app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    app.handle_diff_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    app.start_comment();
    let Mode::CommentInput { body, .. } = &mut app.mode else {
        panic!("the new side of a context row must accept comments");
    };
    body.push_str("new side");
    app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(
        app.comments
            .iter()
            .map(|comment| (comment.line, comment.side, comment.body.as_str()))
            .collect::<Vec<_>>(),
        [(1, Side::Old, "old side"), (1, Side::New, "new side")],
        "side-by-side comments must retain the active column in their anchors"
    );

    app.handle_diff_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    assert_eq!(
        app.diff_layout,
        DiffLayout::Unified,
        "the s key must return to the session's unified view"
    );
    assert_eq!(
        app.comments_for_line(&app.diff.files[0].lines[1]).count(),
        2,
        "unified view must show comments anchored to either side of a context line"
    );

    app.selected_side = Side::Old;
    app.select_diff(4);
    app.handle_diff_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    assert!(
        matches!(app.selected_row(), Some(DiffRow::SideBySide { .. })),
        "switching layouts on a paired addition must retain its review row"
    );
    assert_eq!(
        app.selected_anchor().map(|anchor| anchor.side),
        Some(Side::New),
        "an addition must select the new column even after the old column was preferred"
    );
}

#[test]
fn comment_popup_opens_beside_the_selected_diff_line() {
    let mut app = App::new(ParsedDiff::parse(
        "\
diff --git file.rs file.rs
--- file.rs
+++ file.rs
@@ -1,6 +1,6 @@
 one
 two
 three
-old
+new
 four
 five
",
    ));
    let addition = app
        .diff_rows()
        .iter()
        .position(|row| {
            matches!(
                row,
                DiffRow::Line { file: 0, line } if app.diff.files[0].lines[*line].text == "new"
            )
        })
        .expect("the fixture must contain the added line");
    app.select_diff(addition);
    app.start_comment();
    let Mode::CommentInput { body, .. } = &mut app.mode else {
        panic!("commenting on an added line must open the input");
    };
    body.push_str("note");

    let mut terminal = Terminal::new(TestBackend::new(72, 16)).expect("test terminal");
    terminal
        .draw(|frame| render(frame, &mut app))
        .expect("review must render");
    let buffer = terminal.backend().buffer();
    let rows: Vec<String> = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
        })
        .collect();
    let line_y = rows
        .iter()
        .position(|row| row.contains("+new"))
        .expect("the selected addition must be visible");
    let popup_y = rows
        .iter()
        .position(|row| row.contains("Comment"))
        .expect("the comment popup must be visible");
    let centered_y = (buffer.area.height.saturating_sub(3)) / 2;

    assert_eq!(
        popup_y,
        line_y + 1,
        "the comment popup must sit on the row under the selected line, got {rows:?}"
    );
    assert_ne!(
        popup_y as u16, centered_y,
        "the comment popup must not open in the middle of the screen"
    );
}

#[test]
fn quit_confirm_docks_to_the_bottom_instead_of_the_screen_center() {
    let screen = ratatui::layout::Rect::new(0, 0, 80, 24);
    let area = dock_bottom(screen, screen.width, 3);
    assert_eq!(
        area.y, 21,
        "the quit prompt must sit on the last three rows"
    );
    assert_eq!(area.height, 3);
    assert_eq!(area.width, 80);
    assert_ne!(
        area.y,
        screen.height.saturating_sub(3) / 2,
        "the quit prompt must not float in the middle of the review"
    );

    let mut app = App::new(ParsedDiff::parse(TWO_FILE_DIFF));
    app.select_diff(3);
    app.start_comment();
    let Mode::CommentInput { body, .. } = &mut app.mode else {
        panic!("the fixture must open comment input");
    };
    body.push_str("keep this");
    app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(
        matches!(app.mode, Mode::QuitConfirm { .. }),
        "q must still confirm before discarding comments"
    );

    let mut terminal = Terminal::new(TestBackend::new(80, 16)).expect("test terminal");
    terminal
        .draw(|frame| render(frame, &mut app))
        .expect("quit confirm must render");
    let rows = buffer_rows(terminal.backend().buffer());
    let prompt_y = rows
        .iter()
        .position(|row| row.contains("Discard 1 unexported comment?"))
        .expect("the confirm prompt must be visible");
    assert!(
        prompt_y >= 13,
        "the confirm prompt must render at the bottom, got {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("Quit")),
        "the prompt must stay labeled as a quit confirmation"
    );
}

#[test]
fn arrow_keys_move_the_diff_selection() {
    let mut app = App::new(ParsedDiff::parse(TWO_FILE_DIFF));
    assert_eq!(app.selected_diff, 0);

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(
        app.selected_diff, 1,
        "Down must select the next diff row without requiring j"
    );
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(
        app.selected_diff, 0,
        "Up must select the previous diff row without requiring k"
    );
}

#[test]
fn mouse_click_selects_the_diff_line_under_the_cursor() {
    let mut app = App::new(ParsedDiff::parse(
        "\
diff --git file.rs file.rs
--- file.rs
+++ file.rs
@@ -1,3 +1,3 @@
 one
-old
+new
",
    ));
    let mut terminal = Terminal::new(TestBackend::new(72, 16)).expect("test terminal");
    terminal
        .draw(|frame| render(frame, &mut app))
        .expect("review must render");
    let rows = buffer_rows(terminal.backend().buffer());
    let line_y = rows
        .iter()
        .position(|row| row.contains("+new"))
        .expect("the added line must be visible") as u16;

    assert_eq!(app.selected_diff, 0, "the file header starts selected");
    app.handle_mouse(click(8, line_y));

    assert_eq!(
        app.selected_anchor()
            .map(|anchor| (anchor.line, anchor.side)),
        Some((2, Side::New)),
        "clicking a diff line must select it for commenting, got {:?} from {rows:?}",
        app.selected_row()
    );
    assert!(
        matches!(app.mode, Mode::CommentInput { .. }),
        "clicking an added or deleted line must open the comment box"
    );
}

#[test]
fn side_by_side_deletion_rows_accept_comments() {
    let mut app = App::new(ParsedDiff::parse(
        "\
diff --git file.rs file.rs
--- file.rs
+++ file.rs
@@ -1,4 +1,3 @@
 keep
-removed_fn
-also_gone
 still
",
    ));
    app.handle_diff_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    let removed = app
        .diff_rows()
        .iter()
        .position(|row| {
            matches!(
                row,
                DiffRow::SideBySide { file: 0, row: SideBySideRow::Old(line) }
                    if app.diff.files[0].lines[*line].text == "removed_fn"
            )
        })
        .expect("the deleted line must have its own old-side row");
    app.select_diff(removed);
    assert_eq!(
        app.selected_anchor()
            .map(|anchor| (anchor.line, anchor.side)),
        Some((2, Side::Old)),
        "landing on a deleted line must select the old column"
    );
    app.start_comment();
    let Mode::CommentInput {
        body, existing: _, ..
    } = &mut app.mode
    else {
        panic!(
            "commenting on a deleted side-by-side line must open the input, selected {:?}",
            app.selected_anchor()
        );
    };
    body.push_str("why remove this?");
    app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.comments
            .iter()
            .map(|comment| (comment.line, comment.side, comment.body.as_str()))
            .collect::<Vec<_>>(),
        [(2, Side::Old, "why remove this?")],
        "deleted lines must keep old-side comment anchors"
    );
}

#[test]
fn side_by_side_click_comments_on_a_deleted_line() {
    let mut app = App::new(ParsedDiff::parse(
        "\
diff --git file.rs file.rs
--- file.rs
+++ file.rs
@@ -1,3 +1,3 @@
 keep
-removed_fn
+added_fn
 still
",
    ));
    app.handle_diff_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    let mut terminal = Terminal::new(TestBackend::new(72, 16)).expect("test terminal");
    terminal
        .draw(|frame| render(frame, &mut app))
        .expect("review must render");
    let rows = buffer_rows(terminal.backend().buffer());
    let line_y = rows
        .iter()
        .position(|row| row.contains("removed_fn"))
        .expect("the deleted line must be visible") as u16;
    let old_x = rows[line_y as usize]
        .find("removed_fn")
        .expect("the deleted text must be on screen") as u16;

    app.handle_mouse(click(old_x, line_y));
    let Mode::CommentInput { body, .. } = &mut app.mode else {
        panic!(
            "clicking a deleted line must open a comment on the old side, selected {:?} from {rows:?}",
            app.selected_anchor()
        );
    };
    body.push_str("keep this method");
    app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.comments
            .iter()
            .map(|comment| (comment.side, comment.body.as_str()))
            .collect::<Vec<_>>(),
        [(Side::Old, "keep this method")],
        "a click on the red column must comment on the deleted line, not the replacement"
    );
}

#[test]
fn side_by_side_left_key_comments_on_a_paired_deletion() {
    let mut app = App::new(ParsedDiff::parse(
        "\
diff --git file.rs file.rs
--- file.rs
+++ file.rs
@@ -1,3 +1,3 @@
 keep
-removed_fn
+added_fn
 still
",
    ));
    app.handle_diff_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    let paired = app
        .diff_rows()
        .iter()
        .position(|row| {
            matches!(
                row,
                DiffRow::SideBySide {
                    file: 0,
                    row: SideBySideRow::Paired { old, new }
                } if app.diff.files[0].lines[*old].text == "removed_fn"
                    && app.diff.files[0].lines[*new].text == "added_fn"
            )
        })
        .expect("the replacement must share one side-by-side row");
    app.select_diff(paired);
    app.handle_diff_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    app.start_comment();
    let Mode::CommentInput { body, .. } = &mut app.mode else {
        panic!(
            "Left then c must comment on the deleted side of a replacement, selected {:?}",
            app.selected_anchor()
        );
    };
    body.push_str("why delete this?");
    app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.comments[0].side,
        Side::Old,
        "the left column of a replacement row is the deleted line"
    );
}

#[test]
fn mouse_click_selects_the_side_by_side_column_under_the_cursor() {
    let mut app = App::new(ParsedDiff::parse(SIDE_COMMENT_DIFF));
    app.handle_diff_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    let mut terminal = Terminal::new(TestBackend::new(72, 16)).expect("test terminal");
    terminal
        .draw(|frame| render(frame, &mut app))
        .expect("review must render");
    let rows = buffer_rows(terminal.backend().buffer());
    let line_y = rows
        .iter()
        .position(|row| row.contains("shared"))
        .expect("the context row must be visible") as u16;
    let old_x = rows[line_y as usize]
        .find("shared")
        .expect("the old-side copy must be visible") as u16;
    let new_x = rows[line_y as usize]
        .rfind("shared")
        .expect("the new-side copy must be visible") as u16;

    app.handle_mouse(click(old_x, line_y));
    assert_eq!(
        app.selected_anchor().map(|anchor| anchor.side),
        Some(Side::Old),
        "clicking the left column must select the old side, got {:?} from {rows:?}",
        app.selected_anchor()
    );

    app.handle_mouse(click(new_x, line_y));
    assert_eq!(
        app.selected_anchor().map(|anchor| anchor.side),
        Some(Side::New),
        "clicking the right column must select the new side"
    );
}

#[test]
fn mouse_clicks_are_ignored_while_entering_a_comment() {
    let mut app = App::new(ParsedDiff::parse(TWO_FILE_DIFF));
    let addition = app
        .diff_rows()
        .iter()
        .position(|row| {
            matches!(
                row,
                DiffRow::Line { file: 0, line } if app.diff.files[0].lines[*line].text == "new"
            )
        })
        .expect("the fixture must contain an added line");
    app.select_diff(addition);
    app.start_comment();
    assert!(
        matches!(app.mode, Mode::CommentInput { .. }),
        "the test must start from an open comment box"
    );
    let mut terminal = Terminal::new(TestBackend::new(72, 12)).expect("test terminal");
    terminal
        .draw(|frame| render(frame, &mut app))
        .expect("review must render");
    let selected = app.selected_diff;

    app.handle_mouse(click(8, 6));

    assert_eq!(
        app.selected_diff, selected,
        "clicking must not retarget the line while the comment box is open"
    );
    assert!(
        matches!(app.mode, Mode::CommentInput { .. }),
        "a click must not dismiss comment input"
    );
}

#[test]
fn clicking_a_visible_line_does_not_scroll_it_to_the_bottom() {
    let mut body = String::from(
        "\
diff --git file.rs file.rs
--- file.rs
+++ file.rs
@@ -1,20 +1,20 @@
",
    );
    for index in 1..=20 {
        body.push_str(&format!(" line-{index:02}\n"));
    }
    let mut app = App::new(ParsedDiff::parse(&body));
    app.select_diff(app.diff_rows().len().saturating_sub(1));

    let mut terminal = Terminal::new(TestBackend::new(72, 10)).expect("test terminal");
    terminal
        .draw(|frame| render(frame, &mut app))
        .expect("review must render");
    let offset = app.diff_list.offset;
    assert!(
        offset > 0,
        "the fixture must start scrolled past the first page"
    );

    let rows = buffer_rows(terminal.backend().buffer());
    let target_y = rows
        .iter()
        .position(|row| row.contains("line-16"))
        .expect("a mid-viewport line must stay on screen after scrolling to the end")
        as u16;
    let bottom_y = rows
        .iter()
        .rposition(|row| !row.trim().is_empty())
        .expect("the list must have a bottom row") as u16;
    assert!(
        target_y < bottom_y,
        "the clicked line must start above the bottom of the viewport, got {rows:?}"
    );

    app.handle_mouse(click(8, target_y));
    terminal
        .draw(|frame| render(frame, &mut app))
        .expect("review must render after the click");
    let rows = buffer_rows(terminal.backend().buffer());
    let new_y = rows
        .iter()
        .position(|row| row.contains("line-16"))
        .expect("the clicked line must remain visible") as u16;

    assert_eq!(
        app.diff_list.offset, offset,
        "clicking a visible line must keep the current viewport"
    );
    assert_eq!(
        new_y, target_y,
        "the clicked line must stay where it was instead of jumping to the bottom, got {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("line-20")),
        "later lines that were on screen must stay on screen after the click, got {rows:?}"
    );
}

#[test]
fn clicking_an_inline_comment_opens_it_for_editing() {
    let mut app = App::new(ParsedDiff::parse(
        "\
diff --git file.rs file.rs
--- file.rs
+++ file.rs
@@ -1 +1 @@
-old
+new
",
    ));
    let addition = app
        .diff_rows()
        .iter()
        .position(|row| {
            matches!(
                row,
                DiffRow::Line { file: 0, line } if app.diff.files[0].lines[*line].text == "new"
            )
        })
        .expect("the fixture must contain an added line");
    app.select_diff(addition);
    app.start_comment();
    let Mode::CommentInput { body, .. } = &mut app.mode else {
        panic!("commenting on an added line must open the input");
    };
    body.push_str("first note");
    app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let mut terminal = Terminal::new(TestBackend::new(72, 16)).expect("test terminal");
    terminal
        .draw(|frame| render(frame, &mut app))
        .expect("review must render");
    let rows = buffer_rows(terminal.backend().buffer());
    let comment_y = rows
        .iter()
        .position(|row| row.contains("first note"))
        .expect("the inline comment must be visible") as u16;

    app.handle_mouse(click(12, comment_y));
    let Mode::CommentInput { body, existing, .. } = &app.mode else {
        panic!(
            "clicking the inline comment must open the editor, got {:?}",
            rows
        );
    };
    assert_eq!(body, "first note");
    assert_eq!(*existing, Some(0));
}

#[test]
fn editing_a_comment_replaces_it_instead_of_adding_another() {
    let mut app = App::new(ParsedDiff::parse(
        "\
diff --git file.rs file.rs
--- file.rs
+++ file.rs
@@ -1 +1 @@
-old
+new
",
    ));
    app.select_diff(2);
    app.start_comment();
    let Mode::CommentInput { body, .. } = &mut app.mode else {
        panic!("must open comment input");
    };
    body.push_str("old note");
    app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.edit_comment(0);
    let Mode::CommentInput { body, .. } = &mut app.mode else {
        panic!("edit must reopen the comment");
    };
    body.clear();
    body.push_str("new note");
    app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(
        app.comments
            .iter()
            .map(|comment| comment.body.as_str())
            .collect::<Vec<_>>(),
        ["new note"],
        "saving an edit must replace the existing comment"
    );
}

#[test]
fn ctrl_d_and_empty_enter_delete_the_open_comment() {
    let mut app = App::new(ParsedDiff::parse(
        "\
diff --git file.rs file.rs
--- file.rs
+++ file.rs
@@ -1 +1 @@
-old
+new
",
    ));
    app.select_diff(2);
    app.start_comment();
    let Mode::CommentInput { body, .. } = &mut app.mode else {
        panic!("must open comment input");
    };
    body.push_str("remove me");
    app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.edit_comment(0);
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert!(
        app.comments.is_empty(),
        "Ctrl-D must delete the comment being edited"
    );
    assert!(matches!(app.mode, Mode::Diff));

    app.start_comment();
    let Mode::CommentInput { body, .. } = &mut app.mode else {
        panic!("must open comment input");
    };
    body.push_str("remove later");
    app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.edit_comment(0);
    let Mode::CommentInput { body, .. } = &mut app.mode else {
        panic!("edit must reopen the comment");
    };
    body.clear();
    app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.comments.is_empty(),
        "saving an empty edit must delete the comment"
    );
}

#[test]
fn clicking_the_code_line_does_not_edit_its_comment() {
    let mut app = App::new(ParsedDiff::parse(
        "\
diff --git file.rs file.rs
--- file.rs
+++ file.rs
@@ -1 +1 @@
-old
+new
",
    ));
    app.select_diff(3);
    app.start_comment();
    let Mode::CommentInput { body, .. } = &mut app.mode else {
        panic!("must open comment input");
    };
    body.push_str("keep");
    app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let mut terminal = Terminal::new(TestBackend::new(72, 16)).expect("test terminal");
    terminal
        .draw(|frame| render(frame, &mut app))
        .expect("review must render");
    let rows = buffer_rows(terminal.backend().buffer());
    let line_y = rows
        .iter()
        .position(|row| row.contains("+new"))
        .expect("the added line must be visible") as u16;

    app.handle_mouse(click(8, line_y));
    let Mode::CommentInput { existing, .. } = &app.mode else {
        panic!("clicking an added or deleted code row must open a new comment");
    };
    assert_eq!(
        *existing, None,
        "clicking the code row must not edit the existing inline comment"
    );
}

#[test]
fn comment_list_can_edit_and_delete_the_selected_comment() {
    let mut app = App::new(ParsedDiff::parse(
        "\
diff --git file.rs file.rs
--- file.rs
+++ file.rs
@@ -1 +1 @@
-old
+new
",
    ));
    app.select_diff(2);
    app.start_comment();
    let Mode::CommentInput { body, .. } = &mut app.mode else {
        panic!("must open comment input");
    };
    body.push_str("listed");
    app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.mode = Mode::Comments;

    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
    let Mode::CommentInput { body, existing, .. } = &app.mode else {
        panic!("c in the comment list must edit the selected comment");
    };
    assert_eq!(body, "listed");
    assert_eq!(*existing, Some(0));

    app.mode = Mode::Comments;
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert!(
        app.comments.is_empty(),
        "d in the comment list must delete the selected comment"
    );
}

#[test]
fn d_deletes_the_comment_on_the_selected_diff_line() {
    let mut app = App::new(ParsedDiff::parse(
        "\
diff --git file.rs file.rs
--- file.rs
+++ file.rs
@@ -1 +1 @@
-old
+new
",
    ));
    app.select_diff(3);
    app.start_comment();
    let Mode::CommentInput { body, .. } = &mut app.mode else {
        panic!("must open comment input");
    };
    body.push_str("gone");
    app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert!(
        app.comments.is_empty(),
        "d on a commented diff line must delete that comment"
    );
}

#[test]
fn mouse_scroll_moves_the_diff_selection() {
    let mut app = App::new(ParsedDiff::parse(TWO_FILE_DIFF));
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.selected_diff, 1);
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.selected_diff, 0);
}

#[test]
fn current_round_starts_clean_and_frozen_rev_keeps_its_comments() {
    let mut app = App::from_session(two_round_session());
    assert_eq!(app.viewing, ViewKind::LiveMain);
    assert!(
        app.comments.is_empty(),
        "the current round must not show rev-1 comments"
    );
    assert!(!app.read_only);

    app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.viewing, ViewKind::Frozen(1));
    assert_eq!(
        app.comments
            .iter()
            .map(|comment| comment.body.as_str())
            .collect::<Vec<_>>(),
        ["from rev-1"]
    );
    assert!(app.read_only);
    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
    assert!(
        matches!(app.mode, Mode::Diff),
        "frozen revisions must not accept new comments"
    );
}

#[test]
fn switching_back_to_current_does_not_keep_frozen_comments() {
    let mut app = App::from_session(two_round_session());
    app.select_diff(3);
    app.start_comment();
    let Mode::CommentInput { body, .. } = &mut app.mode else {
        panic!("the current round must accept comments");
    };
    body.push_str("rev-2 note");
    app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    app.viewing = ViewKind::Frozen(1);
    app.load_view();
    assert_eq!(app.comments[0].body, "from rev-1");

    app.viewing = ViewKind::LiveMain;
    app.load_view();
    assert_eq!(
        app.comments
            .iter()
            .map(|comment| comment.body.as_str())
            .collect::<Vec<_>>(),
        ["rev-2 note"],
        "live comments must come back when leaving the rev-1 viewer"
    );
}

#[test]
fn interdiff_view_accepts_comments_on_the_current_round() {
    let mut app = App::from_session(two_round_session());
    app.viewing = ViewKind::LiveSince(1);
    app.load_view();
    assert!(
        !app.read_only,
        "the since-last-rev view must accept comments for the current round"
    );
    assert!(
        app.diff.files[0]
            .lines
            .iter()
            .any(|line| line.text == "live"),
        "the interdiff must show the current tree against rev-1"
    );

    let addition = app
        .diff_rows()
        .iter()
        .position(|row| {
            matches!(
                row,
                DiffRow::Line { file: 0, line } if app.diff.files[0].lines[*line].text == "live"
            )
        })
        .expect("the interdiff must contain the current-tree line");
    app.select_diff(addition);
    app.start_comment();
    let Mode::CommentInput { body, .. } = &mut app.mode else {
        panic!("commenting on the interdiff must open the input");
    };
    body.push_str("since last rev");
    app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    app.viewing = ViewKind::LiveMain;
    app.load_view();
    assert_eq!(
        app.comments
            .iter()
            .map(|comment| comment.body.as_str())
            .collect::<Vec<_>>(),
        ["since last rev"],
        "interdiff comments must stay on the current round"
    );
    assert!(!app.read_only);
}

#[test]
fn same_tree_reopen_starts_on_the_frozen_revision() {
    let app = App::from_session(ReviewSession {
        live: None,
        frozen: vec![FrozenReview {
            rev: 1,
            diff: ParsedDiff::parse(FROZEN_DIFF),
            comments: vec![Comment::open(
                "file.rs".to_owned(),
                1,
                Side::New,
                "saved".to_owned(),
            )],
        }],
        initial: ViewKind::Frozen(1),
    });
    assert_eq!(app.viewing, ViewKind::Frozen(1));
    assert_eq!(app.comments[0].body, "saved");
    assert!(app.read_only);
}

fn add_line_comment(app: &mut App, body: &str) {
    let index = app
        .diff_rows()
        .iter()
        .position(|row| match row {
            DiffRow::Line { file, line } => app.diff.files[*file].lines[*line].anchor().is_some(),
            _ => false,
        })
        .expect("fixture must have a commentable line");
    app.select_diff(index);
    app.start_comment();
    let Mode::CommentInput { body: input, .. } = &mut app.mode else {
        panic!("the fixture must open comment input");
    };
    input.push_str(body);
    app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

#[test]
fn agent_quit_submits_comments_without_a_confirm_prompt() {
    let mut app = App::new(ParsedDiff::parse(TWO_FILE_DIFF));
    app.submit_on_quit = true;
    add_line_comment(&mut app, "please rename this");

    assert!(
        footer_text(&app).contains("q send comments"),
        "agent mode must tell the user that quit sends comments"
    );
    assert!(
        !footer_text(&app).contains("i inline comments"),
        "agent footer should prioritize sending comments over extra toggles"
    );

    let outcome = app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    let Some(ReviewOutcome::Export(comments)) = outcome else {
        panic!("q must submit comments to the agent instead of confirming quit");
    };
    assert_eq!(
        comments
            .iter()
            .map(|comment| comment.body.as_str())
            .collect::<Vec<_>>(),
        ["please rename this"],
        "quitting agent mode must return the live comments"
    );
    assert!(
        !matches!(app.mode, Mode::QuitConfirm { .. }),
        "agent mode must not ask to discard comments that are being sent"
    );
}

#[test]
fn agent_quit_with_no_comments_still_unblocks_the_agent() {
    let mut app = App::new(ParsedDiff::parse(TWO_FILE_DIFF));
    app.submit_on_quit = true;

    let outcome = app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    let Some(ReviewOutcome::Export(comments)) = outcome else {
        panic!("q with no comments must still return an empty export");
    };
    assert!(
        comments.is_empty(),
        "accepting the diff must send an empty comment list, not a discard"
    );
}

const RANGE_DIFF: &str = "\
diff --git file.rs file.rs
--- file.rs
+++ file.rs
@@ -1,4 +1,4 @@
 alpha
 beta
 gamma
-old
+delta
";

fn row_with_text(app: &App, text: &str) -> usize {
    app.diff_rows()
        .iter()
        .position(|row| match row {
            DiffRow::Line { file, line } => app.diff.files[*file].lines[*line].text == text,
            _ => false,
        })
        .unwrap_or_else(|| panic!("fixture must contain {text:?}"))
}

fn save_open_comment(app: &mut App, body: &str) {
    let Mode::CommentInput { body: input, .. } = &mut app.mode else {
        panic!("comment input must be open");
    };
    input.push_str(body);
    app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

#[test]
fn visual_range_comments_cover_every_selected_line() {
    let mut app = App::new(ParsedDiff::parse(RANGE_DIFF));
    app.select_diff(row_with_text(&app, "alpha"));
    app.handle_diff_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert!(
        app.visual.is_some(),
        "v must start a visual range on a commentable line"
    );
    assert!(
        footer_text(&app).contains("visual file.rs:1 [new]"),
        "the footer must show the in-progress range: {}",
        footer_text(&app)
    );

    app.handle_diff_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    app.handle_diff_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    app.handle_diff_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(
        app.selected_anchor().map(|anchor| anchor.line),
        Some(4),
        "j in visual mode must extend onto later lines of the same side"
    );

    app.handle_diff_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
    save_open_comment(&mut app, "extract this block");

    assert!(
        app.visual.is_none(),
        "saving a range comment must leave visual mode"
    );
    assert_eq!(app.comments.len(), 1);
    assert_eq!(app.comments[0].start_line(), 1);
    assert_eq!(app.comments[0].end_line(), 4);
    assert_eq!(app.comments[0].side, Side::New);
    assert_eq!(app.comments[0].location(), "file.rs:1-4");
    assert!(
        app.comments[0].covers("file.rs", 2, Side::New)
            && app.comments[0].covers("file.rs", 4, Side::New),
        "the saved comment must cover every line in the visual span"
    );
}

#[test]
fn capital_v_also_starts_visual_mode() {
    let mut app = App::new(ParsedDiff::parse(RANGE_DIFF));
    app.select_diff(row_with_text(&app, "beta"));
    app.handle_diff_key(KeyEvent::new(KeyCode::Char('V'), KeyModifiers::NONE));
    assert!(app.visual.is_some(), "V must start a visual range");
}

#[test]
fn esc_cancels_a_visual_range_instead_of_quitting() {
    let mut app = App::new(ParsedDiff::parse(RANGE_DIFF));
    app.select_diff(row_with_text(&app, "alpha"));
    app.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    let outcome = app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert!(
        outcome.is_none(),
        "Esc during a range must not quit the review"
    );
    assert!(
        app.visual.is_none(),
        "Esc must cancel the in-progress range"
    );
    assert!(
        app.status.as_deref() == Some("Range canceled."),
        "canceling a range must tell the user: {:?}",
        app.status
    );
}

#[test]
fn visual_range_cannot_leave_the_current_file() {
    let mut app = App::new(ParsedDiff::parse(
        "\
diff --git first.rs first.rs
--- first.rs
+++ first.rs
@@ -1,2 +1,2 @@
 one
-old
+new
diff --git second.rs second.rs
--- second.rs
+++ second.rs
@@ -1 +1 @@
-before
+after
",
    ));
    app.select_diff(row_with_text(&app, "one"));
    app.handle_diff_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    for _ in 0..8 {
        app.handle_diff_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    }

    assert_eq!(
        app.selected_anchor()
            .map(|anchor| (anchor.path.as_str(), anchor.line)),
        Some(("first.rs", 2)),
        "visual movement must stop at the last commentable line of the current file"
    );
    app.handle_diff_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
    assert_eq!(
        app.selected_anchor().map(|anchor| anchor.path.as_str()),
        Some("first.rs"),
        "next-file must not break an in-progress range"
    );
}

#[test]
fn deleting_from_any_line_in_a_range_removes_the_comment() {
    let mut app = App::new(ParsedDiff::parse(RANGE_DIFF));
    app.select_diff(row_with_text(&app, "alpha"));
    app.handle_diff_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    app.handle_diff_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    app.handle_diff_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    app.handle_diff_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
    save_open_comment(&mut app, "too broad");

    app.select_diff(row_with_text(&app, "beta"));
    app.handle_diff_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert!(
        app.comments.is_empty(),
        "d on a line covered by a range comment must delete that comment"
    );
}

#[test]
fn enter_in_visual_mode_opens_a_range_comment() {
    let mut app = App::new(ParsedDiff::parse(RANGE_DIFF));
    app.select_diff(row_with_text(&app, "alpha"));
    app.handle_diff_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    app.handle_diff_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    app.handle_diff_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let Mode::CommentInput {
        anchor, end_line, ..
    } = &app.mode
    else {
        panic!("Enter in visual mode must open a comment for the selected range");
    };
    assert_eq!(anchor.line, 1);
    assert_eq!(*end_line, 2);
}

#[test]
fn frozen_revisions_reject_visual_ranges() {
    let mut app = App::from_session(two_round_session());
    app.viewing = ViewKind::Frozen(1);
    app.load_view();
    let addition = app
        .diff_rows()
        .iter()
        .position(|row| {
            matches!(
                row,
                DiffRow::Line { file: 0, line } if app.diff.files[0].lines[*line].anchor().is_some()
            )
        })
        .expect("frozen fixture must have a commentable line");
    app.select_diff(addition);
    app.handle_diff_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert!(
        app.visual.is_none(),
        "frozen revisions must not start a commentable range"
    );
}

#[test]
fn agent_quit_from_a_frozen_view_sends_the_live_draft_comments() {
    let mut app = App::from_session(two_round_session());
    app.submit_on_quit = true;
    add_line_comment(&mut app, "on the draft");

    app.viewing = ViewKind::Frozen(1);
    app.load_view();
    assert!(app.read_only, "frozen revisions stay read-only");

    let outcome = app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    let Some(ReviewOutcome::Export(comments)) = outcome else {
        panic!("q from a frozen view must still submit the live draft");
    };
    assert_eq!(
        comments
            .iter()
            .map(|comment| comment.body.as_str())
            .collect::<Vec<_>>(),
        ["on the draft"],
        "comments belong to the current round even if the user browsed a frozen rev"
    );
}
