use crate::diff::{DiffLine, DiffLineKind, LineAnchor, SideBySideRow};
use crate::model::{Comment, Side};

use super::{App, DiffLayout, DiffRow, Mode, ReviewOutcome, View};

#[derive(Clone, Copy)]
struct DiffLocation {
    file: usize,
    line: Option<usize>,
    side: Option<Side>,
}

impl App {
    pub(super) fn move_diff_down(&mut self) {
        if self.selected_diff + 1 < self.diff_rows().len() {
            self.select_diff(self.selected_diff + 1);
        }
    }

    pub(super) fn move_diff_up(&mut self) {
        self.select_diff(self.selected_diff.saturating_sub(1));
    }

    pub(super) fn select_diff(&mut self, index: usize) {
        self.selected_diff = index.min(self.diff_rows().len().saturating_sub(1));
        self.normalize_selected_side();
        self.status = None;
    }

    pub(super) fn select_at_pointer(&mut self, column: u16, row: u16) {
        let Some(index) = super::diff_view::item_index_at(
            self.diff_list.inner,
            &self.diff_list.heights,
            self.diff_list.offset,
            row,
        ) else {
            return;
        };
        self.select_diff(index);
        if self.diff_layout == DiffLayout::SideBySide {
            self.select_side(super::diff_view::side_at_column(
                self.diff_list.inner,
                column,
            ));
        }
        if let Some(comment) = super::diff_view::comment_index_at(self, column, row) {
            self.edit_comment(comment);
        } else if self.selected_line_is_change() {
            self.start_comment();
        }
    }

    pub(super) fn select_side(&mut self, side: Side) {
        if self.diff_layout != DiffLayout::SideBySide {
            return;
        }
        let Some(DiffRow::SideBySide { row, .. }) = self.selected_row() else {
            return;
        };
        let line = match side {
            Side::Old => row.old_line(),
            Side::New => row.new_line(),
        };
        if line.is_some() {
            self.selected_side = side;
            self.status = None;
        }
    }

    pub(super) fn next_file(&mut self) {
        if let Some(index) = self
            .diff_rows()
            .iter()
            .enumerate()
            .find_map(|(index, row)| {
                (index > self.selected_diff && matches!(row, DiffRow::File(_))).then_some(index)
            })
        {
            self.select_diff(index);
        }
    }

    pub(super) fn previous_file(&mut self) {
        if let Some(index) = self
            .diff_rows()
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, row)| {
                (index < self.selected_diff && matches!(row, DiffRow::File(_))).then_some(index)
            })
        {
            self.select_diff(index);
        }
    }

    pub(super) fn start_comment(&mut self) {
        if !self.ensure_writable() {
            return;
        }
        let Some(anchor) = self.selected_anchor().cloned() else {
            self.status = Some("Select a changed or context line to comment.".to_owned());
            return;
        };
        self.mode = Mode::CommentInput {
            anchor,
            body: String::new(),
            existing: None,
        };
        self.status = None;
    }

    pub(super) fn edit_comment(&mut self, index: usize) {
        if !self.ensure_writable() {
            return;
        }
        let Some(comment) = self.comments.get(index) else {
            return;
        };
        self.mode = Mode::CommentInput {
            anchor: crate::diff::LineAnchor {
                path: comment.path.clone(),
                line: comment.line,
                side: comment.side,
            },
            body: comment.body.clone(),
            existing: Some(index),
        };
        self.selected_comment = index;
        self.status = Some("Enter save | Esc cancel | Ctrl-D delete".to_owned());
    }

    pub(super) fn edit_selected_comment(&mut self) {
        if self.comments.is_empty() {
            self.status = Some("No comments to edit.".to_owned());
            return;
        }
        self.edit_comment(self.selected_comment);
    }

    pub(super) fn delete_comment(&mut self, index: usize) {
        if !self.ensure_writable() {
            return;
        }
        if index >= self.comments.len() {
            return;
        }
        self.comments.remove(index);
        if self.selected_comment >= self.comments.len() {
            self.selected_comment = self.comments.len().saturating_sub(1);
        }
        self.status = Some("Comment deleted.".to_owned());
    }

    pub(super) fn delete_open_comment(&mut self) {
        let existing = match &self.mode {
            Mode::CommentInput { existing, .. } => *existing,
            _ => return,
        };
        if let Some(index) = existing {
            self.delete_comment(index);
        } else {
            self.status = Some("Comment canceled.".to_owned());
        }
        self.mode = Mode::Diff;
    }

    pub(super) fn delete_selected_line_comment(&mut self) {
        let Some(anchor) = self.selected_anchor().cloned() else {
            self.status = Some("Select a line with a comment to delete.".to_owned());
            return;
        };
        let Some(index) = self.comments.iter().rposition(|comment| {
            comment.path == anchor.path
                && comment.line == anchor.line
                && comment.side == anchor.side
        }) else {
            self.status = Some("No comment on this line.".to_owned());
            return;
        };
        self.delete_comment(index);
    }

    pub(super) fn toggle_selected_file(&mut self) -> bool {
        let Some(DiffRow::File(file)) = self.selected_row() else {
            return false;
        };
        self.collapsed_files[file] = !self.collapsed_files[file];
        let state = if self.collapsed_files[file] {
            "collapsed"
        } else {
            "expanded"
        };
        self.status = Some(format!("{} {state}.", self.diff.files[file].display_path()));
        true
    }

    pub(super) fn toggle_diff_layout(&mut self) {
        let location = self.selected_location();
        self.diff_layout = match self.diff_layout {
            DiffLayout::Unified => DiffLayout::SideBySide,
            DiffLayout::SideBySide => DiffLayout::Unified,
        };

        if let Some(location) = location {
            if let Some(side) = location.side {
                self.selected_side = side;
            }
            let rows = self.diff_rows();
            self.selected_diff = rows
                .iter()
                .position(|row| self.row_matches_location(*row, location))
                .or_else(|| {
                    rows.iter()
                        .position(|row| *row == DiffRow::File(location.file))
                })
                .unwrap_or(0);
        }
        self.normalize_selected_side();
        self.status = Some(format!("{} view.", self.diff_layout.as_str()));
    }

    pub(super) fn request_quit(&mut self) -> Option<ReviewOutcome> {
        self.stash_live_comments();
        if self.pending_comments() == 0 {
            return Some(ReviewOutcome::Quit);
        }
        let previous = match self.mode {
            Mode::Comments => View::Comments,
            Mode::Diff | Mode::CommentInput { .. } | Mode::QuitConfirm { .. } => View::Diff,
        };
        self.mode = Mode::QuitConfirm { previous };
        None
    }

    pub(super) fn visible_view(&self) -> View {
        match self.mode {
            Mode::Diff | Mode::CommentInput { .. } => View::Diff,
            Mode::Comments => View::Comments,
            Mode::QuitConfirm { previous } => previous,
        }
    }

    fn selected_line_is_change(&self) -> bool {
        let Some(row) = self.selected_row() else {
            return false;
        };
        let kind = match row {
            DiffRow::Line { file, line } => self.diff.files[file].lines[line].kind,
            DiffRow::SideBySide { file, row } => {
                let Some((line, _)) = self.line_on_selected_side(row) else {
                    return false;
                };
                self.diff.files[file].lines[line].kind
            }
            DiffRow::File(_) => return false,
        };
        matches!(kind, DiffLineKind::Addition | DiffLineKind::Deletion)
    }

    pub(super) fn selected_anchor(&self) -> Option<&LineAnchor> {
        match self.selected_row()? {
            DiffRow::File(_) => None,
            DiffRow::Line { file, line } => self.diff.files[file].lines[line].anchor(),
            DiffRow::SideBySide { file, row } => {
                let (line, side) = self.line_on_selected_side(row)?;
                self.diff.files[file].lines[line].anchor_on(side)
            }
        }
    }

    pub(super) fn selected_row(&self) -> Option<DiffRow> {
        self.diff_rows().get(self.selected_diff).copied()
    }

    fn selected_location(&self) -> Option<DiffLocation> {
        match self.selected_row()? {
            DiffRow::File(file) => Some(DiffLocation {
                file,
                line: None,
                side: None,
            }),
            DiffRow::Line { file, line } => {
                let diff_line = &self.diff.files[file].lines[line];
                let side = match (
                    diff_line.anchor_on(Side::Old).is_some(),
                    diff_line.anchor_on(Side::New).is_some(),
                ) {
                    (true, false) => Some(Side::Old),
                    (false, true) => Some(Side::New),
                    (true, true) => Some(self.selected_side),
                    (false, false) => None,
                };
                Some(DiffLocation {
                    file,
                    line: Some(line),
                    side,
                })
            }
            DiffRow::SideBySide { file, row } => {
                let (line, side) = match row {
                    SideBySideRow::Hunk(line) | SideBySideRow::Meta(line) => (Some(line), None),
                    SideBySideRow::Paired { .. }
                    | SideBySideRow::Old(_)
                    | SideBySideRow::New(_) => {
                        let selected = self.line_on_selected_side(row);
                        (
                            selected.map(|(line, _)| line),
                            selected.map(|(_, side)| side),
                        )
                    }
                };
                Some(DiffLocation { file, line, side })
            }
        }
    }

    fn row_matches_location(&self, row: DiffRow, location: DiffLocation) -> bool {
        match row {
            DiffRow::File(file) => file == location.file && location.line.is_none(),
            DiffRow::Line { file, line } => file == location.file && location.line == Some(line),
            DiffRow::SideBySide { file, row } if file == location.file => match row {
                SideBySideRow::Hunk(line) | SideBySideRow::Meta(line) => {
                    location.line == Some(line)
                }
                SideBySideRow::Paired { .. } | SideBySideRow::Old(_) | SideBySideRow::New(_) => {
                    location.line.is_some_and(|line| {
                        row.old_line() == Some(line) || row.new_line() == Some(line)
                    })
                }
            },
            DiffRow::SideBySide { .. } => false,
        }
    }

    fn line_on_selected_side(&self, row: SideBySideRow) -> Option<(usize, Side)> {
        match self.selected_side {
            Side::Old => row
                .old_line()
                .map(|line| (line, Side::Old))
                .or_else(|| row.new_line().map(|line| (line, Side::New))),
            Side::New => row
                .new_line()
                .map(|line| (line, Side::New))
                .or_else(|| row.old_line().map(|line| (line, Side::Old))),
        }
    }

    fn normalize_selected_side(&mut self) {
        let Some(DiffRow::SideBySide { row, .. }) = self.selected_row() else {
            return;
        };
        match self.selected_side {
            Side::Old if row.old_line().is_none() && row.new_line().is_some() => {
                self.selected_side = Side::New;
            }
            Side::New if row.new_line().is_none() && row.old_line().is_some() => {
                self.selected_side = Side::Old;
            }
            Side::Old | Side::New => {}
        }
    }

    pub(super) fn diff_rows(&self) -> Vec<DiffRow> {
        let mut rows = Vec::new();
        for (file_index, file) in self.diff.files.iter().enumerate() {
            rows.push(DiffRow::File(file_index));
            if !self.collapsed_files[file_index] {
                match self.diff_layout {
                    DiffLayout::Unified => {
                        rows.extend((0..file.lines.len()).map(|line| DiffRow::Line {
                            file: file_index,
                            line,
                        }));
                    }
                    DiffLayout::SideBySide => {
                        rows.extend(file.side_by_side_rows().into_iter().map(|row| {
                            DiffRow::SideBySide {
                                file: file_index,
                                row,
                            }
                        }));
                    }
                }
            }
        }
        rows
    }

    pub(super) fn comments_for_line<'a>(
        &'a self,
        line: &'a DiffLine,
    ) -> impl Iterator<Item = &'a Comment> + 'a {
        let old_anchor = line.anchor_on(Side::Old);
        let new_anchor = line.anchor_on(Side::New);
        self.comments.iter().filter(move |comment| {
            [old_anchor, new_anchor]
                .into_iter()
                .flatten()
                .any(|anchor| {
                    comment.path == anchor.path
                        && comment.line == anchor.line
                        && comment.side == anchor.side
                })
        })
    }

    pub(super) fn comment_indices_for_line<'a>(
        &'a self,
        line: &'a DiffLine,
    ) -> impl Iterator<Item = usize> + 'a {
        let old_anchor = line.anchor_on(Side::Old);
        let new_anchor = line.anchor_on(Side::New);
        self.comments
            .iter()
            .enumerate()
            .filter_map(move |(index, comment)| {
                [old_anchor, new_anchor]
                    .into_iter()
                    .flatten()
                    .any(|anchor| {
                        comment.path == anchor.path
                            && comment.line == anchor.line
                            && comment.side == anchor.side
                    })
                    .then_some(index)
            })
    }

    pub(super) fn comments_for_anchor<'a>(
        &'a self,
        anchor: Option<&'a LineAnchor>,
    ) -> impl Iterator<Item = &'a Comment> + 'a {
        self.comments.iter().filter(move |comment| {
            anchor.is_some_and(|anchor| {
                comment.path == anchor.path
                    && comment.line == anchor.line
                    && comment.side == anchor.side
            })
        })
    }

    pub(super) fn comment_indices_for_anchor<'a>(
        &'a self,
        anchor: Option<&'a LineAnchor>,
    ) -> impl Iterator<Item = usize> + 'a {
        self.comments
            .iter()
            .enumerate()
            .filter_map(move |(index, comment)| {
                anchor
                    .is_some_and(|anchor| {
                        comment.path == anchor.path
                            && comment.line == anchor.line
                            && comment.side == anchor.side
                    })
                    .then_some(index)
            })
    }
}
