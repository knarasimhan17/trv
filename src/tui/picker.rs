use std::collections::HashSet;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::git::{BranchStack, CommitLogEntry, PreparedReview};
use crate::session::ReviewSession;

use super::{ReviewOutcome, TrvTerminal, bindings, run_review, with_terminal};

#[derive(Debug)]
pub(crate) struct ReviewTarget {
    pub(crate) base_sha: String,
    pub(crate) source_sha: String,
}

pub(crate) enum CommitPickerOutcome {
    Reviewed {
        prepared: PreparedReview,
        outcome: ReviewOutcome,
    },
    Quit,
}

#[derive(Debug)]
enum PickerChoice {
    Review(ReviewTarget),
    Quit,
}

#[derive(Clone, Copy)]
enum PickerStep {
    Source { selected: usize },
    Base { source: usize, selected: usize },
}

struct CommitPicker {
    branch: Option<BranchStack>,
    commits: Vec<CommitLogEntry>,
    graph_prefixes: Vec<String>,
    step: PickerStep,
    now: DateTime<Utc>,
    help: bool,
}

enum SourceRow {
    Branch,
    Commit(usize),
}

impl CommitPicker {
    fn new(commits: Vec<CommitLogEntry>, branch: Option<BranchStack>) -> Self {
        let graph_prefixes = commit_graph_prefixes(&commits);
        Self {
            branch,
            commits,
            graph_prefixes,
            step: PickerStep::Source { selected: 0 },
            now: Utc::now(),
            help: false,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<PickerChoice> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        if self.help {
            if bindings::closes_help(&key) {
                self.help = false;
            }
            return None;
        }

        let action = bindings::action_for(bindings::picker_bindings(self.context()).iter(), &key)?;
        match action {
            bindings::PickerAction::MoveDown => self.move_down(),
            bindings::PickerAction::MoveUp => self.move_up(),
            bindings::PickerAction::Select => return self.select(),
            bindings::PickerAction::Back => return self.back(),
            bindings::PickerAction::Quit => return Some(PickerChoice::Quit),
            bindings::PickerAction::Help => self.help = true,
        }
        None
    }

    fn context(&self) -> bindings::PickerContext {
        match self.step {
            PickerStep::Source { .. } => bindings::PickerContext::Source,
            PickerStep::Base { .. } => bindings::PickerContext::Base,
        }
    }

    fn move_down(&mut self) {
        let item_count = self.item_count();
        let selected = self.selected_mut();
        if *selected + 1 < item_count {
            *selected += 1;
        }
    }

    fn move_up(&mut self) {
        let selected = self.selected_mut();
        *selected = selected.saturating_sub(1);
    }

    fn select(&mut self) -> Option<PickerChoice> {
        match self.step {
            PickerStep::Source { selected } => match self.source_row(selected) {
                SourceRow::Branch => {
                    let branch = self
                        .branch
                        .as_ref()
                        .expect("the branch row requires a detected stack");
                    Some(PickerChoice::Review(ReviewTarget {
                        base_sha: branch.merge_base_sha.clone(),
                        source_sha: branch.head_sha.clone(),
                    }))
                }
                SourceRow::Commit(index) => {
                    let commit = self
                        .commits
                        .get(index)
                        .expect("source picker index must be bounded by the commit list");
                    let base = commit
                        .first_parent_sha()
                        .and_then(|parent| {
                            self.commits.iter().position(|commit| commit.sha == parent)
                        })
                        .unwrap_or(index);
                    self.step = PickerStep::Base {
                        source: index,
                        selected: base,
                    };
                    None
                }
            },
            PickerStep::Base { source, selected } => {
                let source = self
                    .commits
                    .get(source)
                    .expect("base picker source index must be bounded by the commit list");
                let base = self
                    .commits
                    .get(selected)
                    .expect("base picker selection must be bounded by the commit list");
                Some(PickerChoice::Review(ReviewTarget {
                    base_sha: base.sha.clone(),
                    source_sha: source.sha.clone(),
                }))
            }
        }
    }

    fn back(&mut self) -> Option<PickerChoice> {
        match self.step {
            PickerStep::Source { .. } => Some(PickerChoice::Quit),
            PickerStep::Base { source, .. } => {
                self.step = PickerStep::Source { selected: source };
                None
            }
        }
    }

    fn item_count(&self) -> usize {
        match self.step {
            PickerStep::Source { .. } => self.commits.len() + usize::from(self.branch.is_some()),
            PickerStep::Base { .. } => self.commits.len(),
        }
    }

    fn source_row(&self, selected: usize) -> SourceRow {
        match &self.branch {
            Some(_) if selected == 0 => SourceRow::Branch,
            Some(_) => SourceRow::Commit(selected - 1),
            None => SourceRow::Commit(selected),
        }
    }

    fn selected(&self) -> usize {
        match self.step {
            PickerStep::Source { selected } | PickerStep::Base { selected, .. } => selected,
        }
    }

    fn selected_mut(&mut self) -> &mut usize {
        match &mut self.step {
            PickerStep::Source { selected } | PickerStep::Base { selected, .. } => selected,
        }
    }

    fn commit_items(&self, area_width: u16) -> Vec<ListItem<'static>> {
        self.commits
            .iter()
            .zip(self.graph_prefixes.iter())
            .map(|(commit, graph)| commit_item(commit, graph, &self.now, area_width))
            .collect()
    }
}

pub(crate) fn run(
    commits: Vec<CommitLogEntry>,
    branch: Option<BranchStack>,
    prepare: impl FnOnce(ReviewTarget) -> Result<PreparedReview>,
    into_session: impl FnOnce(&PreparedReview) -> Result<ReviewSession>,
    submit_on_quit: bool,
) -> Result<CommitPickerOutcome> {
    with_terminal(move |terminal| {
        let mut picker = CommitPicker::new(commits, branch);
        match choose_review_target(terminal, &mut picker)? {
            PickerChoice::Review(target) => {
                let prepared = prepare(target)?;
                let session = into_session(&prepared)?;
                let outcome = run_review(terminal, session, submit_on_quit)?;
                Ok(CommitPickerOutcome::Reviewed { prepared, outcome })
            }
            PickerChoice::Quit => Ok(CommitPickerOutcome::Quit),
        }
    })
}

fn choose_review_target(
    terminal: &mut TrvTerminal,
    picker: &mut CommitPicker,
) -> Result<PickerChoice> {
    loop {
        terminal
            .draw(|frame| render(frame, picker))
            .context("failed to draw commit picker")?;
        if let Event::Key(key) = event::read().context("failed to read terminal input")?
            && let Some(choice) = picker.handle_key(key)
        {
            return Ok(choice);
        }
    }
}

fn render(frame: &mut Frame<'_>, picker: &CommitPicker) {
    let screen = frame.area();
    let [area, footer] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(screen);
    let (title, items) = match picker.step {
        PickerStep::Source { .. } => {
            let mut items = Vec::new();
            if let Some(branch) = &picker.branch {
                items.push(branch_item(branch));
            }
            items.extend(picker.commit_items(area.width));
            (" trv | select review ".to_owned(), items)
        }
        PickerStep::Base { source, .. } => {
            let source = picker
                .commits
                .get(source)
                .expect("base picker source index must be bounded by the commit list");
            let items = picker.commit_items(area.width);
            (
                format!(" trv | select base for {} ", source.short_sha),
                items,
            )
        }
    };
    let list = List::new(items)
        .block(Block::bordered().title(title))
        .highlight_symbol("> ")
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default();
    state.select(Some(picker.selected()));
    frame.render_stateful_widget(list, area, &mut state);
    frame.render_widget(
        Paragraph::new(bindings::HELP_HINT).style(Style::default().fg(Color::DarkGray)),
        footer,
    );

    if picker.help {
        let context = picker.context();
        bindings::render_help(
            frame,
            context.help_title(),
            bindings::picker_bindings(context).iter(),
        );
    }
}

fn branch_item(branch: &BranchStack) -> ListItem<'static> {
    let commits = if branch.commit_count == 1 {
        "1 commit".to_owned()
    } else {
        format!("{} commits", branch.commit_count)
    };
    ListItem::new(Line::from(vec![
        Span::styled(
            "this branch  ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("vs {}  {commits}", branch.mainline)),
    ]))
}

fn commit_item(
    commit: &CommitLogEntry,
    graph: &str,
    now: &DateTime<Utc>,
    area_width: u16,
) -> ListItem<'static> {
    // Two border columns and the two-column selection marker are unavailable to row text.
    const LIST_CHROME_WIDTH: u16 = 4;

    let prefix = format!("{graph}{}  ", commit.short_sha);
    let age = relative_age(&commit.committed_at, now);
    let indicator = if commit.unpushed { "  [unpushed]" } else { "" };
    let suffix = format!("  {age}{indicator}");
    let row_width = usize::from(area_width.saturating_sub(LIST_CHROME_WIDTH));
    let subject_width = row_width
        .saturating_sub(UnicodeWidthStr::width(prefix.as_str()))
        .saturating_sub(UnicodeWidthStr::width(suffix.as_str()));
    let subject = truncate_to_width(&commit.subject, subject_width);

    let mut spans = vec![
        Span::styled(graph.to_owned(), Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{}  ", commit.short_sha),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(subject),
        Span::styled(format!("  {age}"), Style::default().fg(Color::DarkGray)),
    ];
    if commit.unpushed {
        spans.push(Span::styled(
            indicator,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }
    ListItem::new(Line::from(spans))
}

fn relative_age(committed_at: &DateTime<Utc>, now: &DateTime<Utc>) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    // Compact picker labels use fixed durations rather than calendar boundaries.
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 365 * DAY;

    let seconds = now
        .timestamp()
        .saturating_sub(committed_at.timestamp())
        .max(0);
    let (value, unit) = if seconds < MINUTE {
        (seconds, "s")
    } else if seconds < HOUR {
        (seconds / MINUTE, "m")
    } else if seconds < DAY {
        (seconds / HOUR, "h")
    } else if seconds < MONTH {
        (seconds / DAY, "d")
    } else if seconds < YEAR {
        (seconds / MONTH, "mo")
    } else {
        (seconds / YEAR, "y")
    };
    format!("{value}{unit} ago")
}

fn truncate_to_width(text: &str, max_width: usize) -> String {
    const ELLIPSIS: &str = "...";

    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_owned();
    }
    if max_width <= ELLIPSIS.len() {
        return ".".repeat(max_width);
    }

    let target_width = max_width - ELLIPSIS.len();
    let mut truncated = String::new();
    let mut width = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if width + character_width > target_width {
            break;
        }
        truncated.push(character);
        width += character_width;
    }
    truncated.push_str(ELLIPSIS);
    truncated
}

const GRAPH_NORTH: u8 = 1 << 0;
const GRAPH_SOUTH: u8 = 1 << 1;
const GRAPH_EAST: u8 = 1 << 2;
const GRAPH_WEST: u8 = 1 << 3;
const GRAPH_NODE: u8 = 1 << 4;

fn commit_graph_prefixes(commits: &[CommitLogEntry]) -> Vec<String> {
    let in_list = commits
        .iter()
        .map(|commit| commit.sha.as_str())
        .collect::<HashSet<_>>();
    let mut lanes: Vec<Option<&str>> = Vec::new();
    let mut prefixes = Vec::with_capacity(commits.len());

    for commit in commits {
        let reserved = lanes
            .iter()
            .enumerate()
            .filter_map(|(index, lane)| (*lane == Some(commit.sha.as_str())).then_some(index))
            .collect::<Vec<_>>();
        let commit_lane = reserved.first().copied().unwrap_or_else(|| {
            lanes
                .iter()
                .position(Option::is_none)
                .unwrap_or(lanes.len())
        });

        let mut next = lanes.clone();
        if commit_lane >= next.len() {
            next.resize(commit_lane + 1, None);
        }
        for &index in &reserved {
            next[index] = None;
        }
        next[commit_lane] = None;

        let mut parent_lanes = Vec::new();
        let mut used_commit_lane = false;
        for parent in commit.parent_shas.iter().map(String::as_str) {
            if !in_list.contains(parent) {
                continue;
            }
            if let Some(existing) = next.iter().position(|lane| *lane == Some(parent)) {
                parent_lanes.push(existing);
                continue;
            }
            let slot = if !used_commit_lane {
                used_commit_lane = true;
                commit_lane
            } else {
                next.iter().position(Option::is_none).unwrap_or(next.len())
            };
            if slot == next.len() {
                next.push(None);
            }
            next[slot] = Some(parent);
            parent_lanes.push(slot);
        }

        let incoming = reserved
            .iter()
            .copied()
            .filter(|index| *index != commit_lane)
            .collect::<Vec<_>>();
        prefixes.push(graph_row_prefix(
            &lanes,
            &next,
            commit_lane,
            &parent_lanes,
            &incoming,
        ));

        lanes = next;
        while matches!(lanes.last(), Some(None)) {
            lanes.pop();
        }
    }

    let width = prefixes
        .iter()
        .map(|prefix| UnicodeWidthStr::width(prefix.as_str()))
        .max()
        .unwrap_or(0);
    for prefix in &mut prefixes {
        let padding = width.saturating_sub(UnicodeWidthStr::width(prefix.as_str()));
        prefix.push_str(&" ".repeat(padding));
    }
    prefixes
}

fn graph_row_prefix(
    lanes: &[Option<&str>],
    next: &[Option<&str>],
    commit_lane: usize,
    parent_lanes: &[usize],
    incoming: &[usize],
) -> String {
    let columns = lanes
        .len()
        .max(next.len())
        .max(commit_lane.saturating_add(1));
    let mut flags = vec![0_u8; columns];
    for index in 0..columns {
        if lanes.get(index).copied().flatten().is_some() {
            flags[index] |= GRAPH_NORTH;
        }
        if next.get(index).copied().flatten().is_some() {
            flags[index] |= GRAPH_SOUTH;
        }
    }
    flags[commit_lane] |= GRAPH_NODE;

    let mut connected = incoming.to_vec();
    connected.extend(
        parent_lanes
            .iter()
            .copied()
            .filter(|index| *index != commit_lane),
    );
    if let Some(min) = connected.iter().copied().chain([commit_lane]).min()
        && let Some(max) = connected.iter().copied().chain([commit_lane]).max()
        && min != max
    {
        for index in min..=max {
            if index != min {
                flags[index] |= GRAPH_WEST;
            }
            if index != max {
                flags[index] |= GRAPH_EAST;
            }
        }
    }

    let mut prefix = String::with_capacity(columns.saturating_mul(2));
    for column_flags in flags {
        let (center, east) = graph_cell(column_flags);
        prefix.push(center);
        prefix.push(east);
    }
    prefix
}

fn graph_cell(flags: u8) -> (char, char) {
    let east = if flags & GRAPH_EAST != 0 { '─' } else { ' ' };
    if flags & GRAPH_NODE != 0 {
        return ('*', east);
    }

    let north = flags & GRAPH_NORTH != 0;
    let south = flags & GRAPH_SOUTH != 0;
    let west = flags & GRAPH_WEST != 0;
    let east_edge = flags & GRAPH_EAST != 0;
    let center = match (north, south, east_edge, west) {
        (true, true, false, false) => '│',
        (true, true, true, false) => '├',
        (true, true, false, true) => '┤',
        (true, true, true, true) => '┼',
        (false, false, true, true) => '─',
        (true, false, true, false) => '└',
        (true, false, false, true) => '┘',
        (false, true, true, false) => '┌',
        (false, true, false, true) => '┐',
        (true, false, true, true) => '┴',
        (false, true, true, true) => '┬',
        (true, false, false, false) | (false, true, false, false) => '│',
        (false, false, true, false) | (false, false, false, true) => '─',
        _ => ' ',
    };
    (center, east)
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{
        BranchStack, CommitLogEntry, CommitPicker, PickerChoice, PickerStep, ReviewTarget, Utc,
        commit_graph_prefixes,
    };

    #[test]
    fn source_picker_lists_only_commits() {
        let picker = CommitPicker::new(
            vec![commit("source", Some("parent")), commit("parent", None)],
            None,
        );

        assert_eq!(
            picker.item_count(),
            2,
            "the source picker must not insert a working-tree row"
        );
        assert!(
            matches!(picker.step, PickerStep::Source { selected: 0 }),
            "the first commit must be selected by default"
        );
    }

    #[test]
    fn selecting_a_commit_reviews_it_against_its_parent() {
        let mut picker = CommitPicker::new(
            vec![commit("source", Some("parent")), commit("parent", None)],
            None,
        );

        assert!(
            picker.handle_key(key(KeyCode::Enter)).is_none(),
            "choosing a source commit must open the base picker"
        );
        assert!(
            matches!(
                picker.step,
                PickerStep::Base {
                    source: 0,
                    selected: 1
                }
            ),
            "the base picker must preselect the source commit's parent"
        );

        match picker.handle_key(key(KeyCode::Enter)) {
            Some(PickerChoice::Review(ReviewTarget {
                base_sha,
                source_sha,
            })) => {
                assert_eq!(source_sha, "source");
                assert_eq!(base_sha, "parent");
            }
            other => panic!("expected a commit range review, got {other:?}"),
        }
    }

    #[test]
    fn help_closes_without_changing_either_picker_step() {
        let commits = vec![commit("source", Some("parent")), commit("parent", None)];
        let mut picker = CommitPicker::new(commits, None);

        for close in [KeyCode::Char('?'), KeyCode::Esc, KeyCode::Char('q')] {
            picker.handle_key(key(KeyCode::Char('?')));
            assert!(picker.help, "? must open help in the source picker");
            picker.handle_key(key(KeyCode::Char('j')));
            picker.handle_key(key(close));
            assert!(!picker.help, "a help close key must dismiss the overlay");
            assert!(
                matches!(picker.step, PickerStep::Source { selected: 0 }),
                "help must preserve the source picker selection"
            );
        }

        picker.handle_key(key(KeyCode::Enter));
        assert!(
            matches!(
                picker.step,
                PickerStep::Base {
                    source: 0,
                    selected: 1
                }
            ),
            "selecting a commit must open its parent in the base picker"
        );

        picker.handle_key(key(KeyCode::Char('?')));
        picker.handle_key(key(KeyCode::Up));
        picker.handle_key(key(KeyCode::Char('q')));
        assert!(
            matches!(
                picker.step,
                PickerStep::Base {
                    source: 0,
                    selected: 1
                }
            ),
            "closing help must preserve the base picker selection"
        );
    }

    #[test]
    fn source_picker_puts_the_branch_stack_first() {
        let mut picker = CommitPicker::new(
            vec![commit("source", Some("parent")), commit("parent", None)],
            Some(stack("origin/main", "parent", "source", 2)),
        );

        assert_eq!(
            picker.item_count(),
            3,
            "the source picker must list this branch above individual commits"
        );
        match picker.handle_key(key(KeyCode::Enter)) {
            Some(PickerChoice::Review(ReviewTarget {
                base_sha,
                source_sha,
            })) => {
                assert_eq!(base_sha, "parent");
                assert_eq!(source_sha, "source");
            }
            other => panic!("expected an immediate branch review, got {other:?}"),
        }
    }

    #[test]
    fn selecting_a_commit_below_the_branch_row_still_opens_the_base_picker() {
        let mut picker = CommitPicker::new(
            vec![commit("source", Some("parent")), commit("parent", None)],
            Some(stack("origin/main", "parent", "source", 2)),
        );
        picker.handle_key(key(KeyCode::Char('j')));
        assert!(
            picker.handle_key(key(KeyCode::Enter)).is_none(),
            "choosing a commit under the branch row must still open the base picker"
        );
        assert!(
            matches!(
                picker.step,
                PickerStep::Base {
                    source: 0,
                    selected: 1
                }
            ),
            "the base picker must use the selected commit, not the branch row index"
        );
    }

    #[test]
    fn graph_prefix_links_a_child_to_its_parent() {
        let commits = vec![commit("child", Some("parent")), commit("parent", None)];
        let prefixes = trimmed_graph(&commits);

        assert_eq!(
            prefixes,
            vec!["*", "*"],
            "linear history must keep one star column so the child sits on its parent"
        );
        assert_eq!(
            commit_graph_prefixes(&commits).len(),
            commits.len(),
            "every commit must keep exactly one graph row"
        );
    }

    #[test]
    fn merge_history_renders_distinct_tree_lines() {
        let commits = vec![
            commit_with_parents("merge", &["main", "side"]),
            commit("side", Some("main")),
            commit("main", None),
        ];
        let prefixes = trimmed_graph(&commits);

        assert_eq!(
            prefixes,
            vec!["*─┐", "├─*", "*"],
            "a merge must open a second lane for the side parent, then rejoin at main"
        );
        assert_ne!(
            prefixes[0], prefixes[1],
            "the merge commit and its side parent must not share a flat list row"
        );
        assert!(
            prefixes.iter().all(|prefix| prefix.contains('*')),
            "each selectable row must include the commit node, not a decoration-only connector"
        );

        let with_first_parent = vec![
            commit_with_parents("merge", &["mainline", "side"]),
            commit("mainline", Some("root")),
            commit("side", Some("root")),
            commit("root", None),
        ];
        assert_eq!(
            trimmed_graph(&with_first_parent),
            vec!["*─┐", "* │", "├─*", "*"],
            "the first-parent chain must keep the left lane while the side commit hangs off it"
        );
    }

    #[test]
    fn selecting_a_tree_row_maps_to_the_commit_sha() {
        let mut picker = CommitPicker::new(
            vec![
                commit_with_parents("merge", &["main", "side"]),
                commit("side", Some("main")),
                commit("main", None),
            ],
            None,
        );

        assert_eq!(
            picker.item_count(),
            3,
            "the tree must not insert decoration-only rows between commits"
        );
        assert_eq!(
            picker.graph_prefixes.len(),
            picker.commits.len(),
            "graph prefixes must stay aligned with commit indices"
        );

        picker.handle_key(key(KeyCode::Char('j')));
        assert!(
            picker.handle_key(key(KeyCode::Enter)).is_none(),
            "choosing the side-branch row must open the base picker"
        );
        let PickerStep::Base { source, selected } = picker.step else {
            panic!("choosing a tree row must open the base picker");
        };
        assert_eq!(
            picker.commits[source].sha, "side",
            "j/k must select the side commit, not a graph decoration"
        );
        assert_eq!(
            picker.commits[selected].sha, "main",
            "the base picker must still preselect the chosen commit's first parent"
        );

        match picker.handle_key(key(KeyCode::Enter)) {
            Some(PickerChoice::Review(ReviewTarget {
                base_sha,
                source_sha,
            })) => {
                assert_eq!(source_sha, "side");
                assert_eq!(base_sha, "main");
            }
            other => panic!("expected a commit range review, got {other:?}"),
        }
    }

    #[test]
    fn branch_shortcut_stays_above_the_commit_tree() {
        let picker = CommitPicker::new(
            vec![
                commit_with_parents("merge", &["main", "side"]),
                commit("side", Some("main")),
                commit("main", None),
            ],
            Some(stack("origin/main", "main", "merge", 3)),
        );

        assert_eq!(
            picker.item_count(),
            4,
            "the branch shortcut must remain a header row above the tree"
        );
        assert!(
            matches!(picker.source_row(0), super::SourceRow::Branch),
            "row 0 must stay the branch-stack shortcut, not a fake commit node"
        );
        assert!(
            matches!(picker.source_row(2), super::SourceRow::Commit(1)),
            "tree rows under the shortcut must still map to commit indices"
        );
        assert_eq!(
            trimmed_graph(&picker.commits),
            vec!["*─┐", "├─*", "*"],
            "the commit tree must keep merge geometry under the shortcut row"
        );
    }

    fn stack(mainline: &str, merge_base: &str, head: &str, commit_count: usize) -> BranchStack {
        BranchStack {
            mainline: mainline.to_owned(),
            merge_base_sha: merge_base.to_owned(),
            head_sha: head.to_owned(),
            commit_count,
        }
    }

    fn commit(sha: &str, first_parent_sha: Option<&str>) -> CommitLogEntry {
        commit_with_parents(sha, first_parent_sha.as_slice())
    }

    fn commit_with_parents(sha: &str, parents: &[&str]) -> CommitLogEntry {
        CommitLogEntry {
            sha: sha.to_owned(),
            short_sha: sha.to_owned(),
            parent_shas: parents.iter().map(|parent| (*parent).to_owned()).collect(),
            subject: sha.to_owned(),
            committed_at: Utc::now(),
            unpushed: false,
        }
    }

    fn trimmed_graph(commits: &[CommitLogEntry]) -> Vec<String> {
        commit_graph_prefixes(commits)
            .into_iter()
            .map(|prefix| prefix.trim_end().to_owned())
            .collect()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
}
