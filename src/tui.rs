mod bindings;
mod comment_input;
mod diff_view;
mod picker;
mod review;

use std::fs::File;
use std::io::{self, IsTerminal, Write};

use anyhow::{Context, Result};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Constraint, Layout, Position, Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal};
use unicode_width::UnicodeWidthStr;

use crate::diff::{LineAnchor, ParsedDiff, SideBySideRow};
use crate::model::{Comment, Side};
use crate::session::{ReviewSession, ViewKind};

pub(crate) use picker::{CommitPickerOutcome, run as run_picker};

type TrvTerminal = Terminal<SessionBackend>;

enum TerminalWriter {
    Stdout(io::Stdout),
    Tty(File),
}

impl TerminalWriter {
    fn open() -> Result<Self> {
        if io::stdin().is_terminal() && io::stdout().is_terminal() {
            Ok(Self::Stdout(io::stdout()))
        } else {
            Ok(Self::Tty(crate::tty::session_tty()?))
        }
    }

    fn try_clone(&self) -> Result<Self> {
        match self {
            Self::Stdout(_) => Ok(Self::Stdout(io::stdout())),
            Self::Tty(file) => file
                .try_clone()
                .map(Self::Tty)
                .context("failed to clone session terminal"),
        }
    }

    fn size_tty(&self) -> Result<Option<File>> {
        match self {
            Self::Stdout(_) => Ok(None),
            Self::Tty(file) => file
                .try_clone()
                .map(Some)
                .context("failed to clone session terminal"),
        }
    }
}

impl Write for TerminalWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Stdout(stdout) => stdout.write(buf),
            Self::Tty(tty) => tty.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Stdout(stdout) => stdout.flush(),
            Self::Tty(tty) => tty.flush(),
        }
    }
}

pub(crate) enum ReviewOutcome {
    Export(Vec<Comment>),
    Quit,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Diff,
    Comments,
}

enum Mode {
    Diff,
    Comments,
    CommentInput {
        anchor: LineAnchor,
        body: String,
        existing: Option<usize>,
    },
    QuitConfirm {
        previous: View,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiffLayout {
    Unified,
    SideBySide,
}

impl DiffLayout {
    fn as_str(self) -> &'static str {
        match self {
            Self::Unified => "unified",
            Self::SideBySide => "side-by-side",
        }
    }
}

struct App {
    session: ReviewSession,
    viewing: ViewKind,
    read_only: bool,
    rev_picker: Option<usize>,
    diff: ParsedDiff,
    collapsed_files: Vec<bool>,
    comments: Vec<Comment>,
    selected_diff: usize,
    selected_side: Side,
    selected_comment: usize,
    diff_layout: DiffLayout,
    inline_comments: bool,
    mode: Mode,
    status: Option<String>,
    help: bool,
    diff_list: DiffListLayout,
    submit_on_quit: bool,
}

#[derive(Clone, Debug, Default)]
struct DiffListLayout {
    inner: Rect,
    offset: usize,
    heights: Vec<u16>,
    line_width: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiffRow {
    File(usize),
    Line { file: usize, line: usize },
    SideBySide { file: usize, row: SideBySideRow },
}

impl App {
    #[cfg(test)]
    fn new(diff: ParsedDiff) -> Self {
        Self::from_session(ReviewSession::live_only(diff))
    }

    fn from_session(session: ReviewSession) -> Self {
        let viewing = session.initial;
        let mut app = Self {
            session,
            viewing,
            read_only: false,
            rev_picker: None,
            diff: ParsedDiff::parse(""),
            collapsed_files: Vec::new(),
            comments: Vec::new(),
            selected_diff: 0,
            selected_side: Side::New,
            selected_comment: 0,
            diff_layout: DiffLayout::Unified,
            inline_comments: true,
            mode: Mode::Diff,
            status: None,
            help: false,
            diff_list: DiffListLayout::default(),
            submit_on_quit: false,
        };
        app.load_view();
        app
    }

    fn stash_live_comments(&mut self) {
        if !self.read_only
            && let Some(live) = &mut self.session.live
        {
            live.comments = self.comments.clone();
        }
    }

    fn load_view(&mut self) {
        self.stash_live_comments();
        match self.viewing {
            ViewKind::LiveMain => {
                let live = self
                    .session
                    .live
                    .as_ref()
                    .expect("live main view requires a current review");
                self.diff = live.vs_main.clone();
                self.comments = live.comments.clone();
                self.read_only = false;
            }
            ViewKind::LiveSince(rev) => {
                let live = self
                    .session
                    .live
                    .as_ref()
                    .expect("interdiff view requires a current review");
                let diff = live
                    .vs_previous
                    .as_ref()
                    .and_then(|(previous, diff)| (*previous == rev).then_some(diff.clone()))
                    .expect("interdiff view requires the previous revision diff");
                self.diff = diff;
                self.comments = live.comments.clone();
                self.read_only = false;
            }
            ViewKind::Frozen(rev) => {
                let frozen = self
                    .session
                    .frozen
                    .iter()
                    .find(|revision| revision.rev == rev)
                    .expect("frozen view requires a stored revision");
                self.diff = frozen.diff.clone();
                self.comments = frozen.comments.clone();
                self.read_only = true;
            }
        }
        self.collapsed_files = vec![false; self.diff.files.len()];
        self.selected_diff = 0;
        self.selected_side = Side::New;
        self.selected_comment = 0;
        self.diff_list = DiffListLayout::default();
        self.mode = Mode::Diff;
        self.rev_picker = None;
    }

    fn view_label(&self) -> String {
        match self.viewing {
            ViewKind::LiveMain => format!("rev-{} draft", self.session.next_rev()),
            ViewKind::LiveSince(rev) => format!("current vs rev-{rev}"),
            ViewKind::Frozen(rev) => format!("rev-{rev}"),
        }
    }

    fn pending_comments(&self) -> usize {
        self.live_comments().len()
    }

    fn live_comments(&self) -> Vec<Comment> {
        match self.viewing {
            ViewKind::LiveMain | ViewKind::LiveSince(_) => self.comments.clone(),
            ViewKind::Frozen(_) => self
                .session
                .live
                .as_ref()
                .map(|live| live.comments.clone())
                .unwrap_or_default(),
        }
    }

    fn ensure_writable(&mut self) -> bool {
        if self.read_only {
            self.status = Some(format!("{} is read-only.", self.view_label()));
            false
        } else {
            true
        }
    }

    fn revision_choices(&self) -> Vec<(ViewKind, String)> {
        let mut choices = Vec::new();
        if self.session.live.is_some() {
            choices.push((
                ViewKind::LiveMain,
                format!(
                    "current vs mainline (rev-{} draft)",
                    self.session.next_rev()
                ),
            ));
            if let Some((rev, _)) = self
                .session
                .live
                .as_ref()
                .and_then(|live| live.vs_previous.as_ref())
            {
                choices.push((ViewKind::LiveSince(*rev), format!("current vs rev-{rev}")));
            }
        }
        for revision in self.session.frozen.iter().rev() {
            choices.push((
                ViewKind::Frozen(revision.rev),
                format!(
                    "rev-{} ({} comments)",
                    revision.rev,
                    revision.comments.len()
                ),
            ));
        }
        choices
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<ReviewOutcome> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        if self.help {
            if bindings::closes_help(&key) {
                self.help = false;
            }
            return None;
        }
        if self.rev_picker.is_some() {
            self.handle_rev_picker_key(key);
            return None;
        }

        if matches!(self.mode, Mode::Diff) {
            self.handle_diff_key(key)
        } else if matches!(self.mode, Mode::Comments) {
            self.handle_comments_key(key)
        } else if matches!(self.mode, Mode::CommentInput { .. }) {
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                self.request_quit()
            } else if key.code == KeyCode::Char('d')
                && key.modifiers.contains(KeyModifiers::CONTROL)
            {
                self.delete_open_comment();
                None
            } else {
                self.handle_input_key(key)
            }
        } else if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.request_quit()
        } else {
            self.handle_quit_key(key)
        }
    }

    fn open_rev_picker(&mut self) {
        let choices = self.revision_choices();
        if choices.is_empty() {
            self.status = Some("No saved revisions.".to_owned());
            return;
        }
        let selected = choices
            .iter()
            .position(|(kind, _)| *kind == self.viewing)
            .unwrap_or(0);
        self.rev_picker = Some(selected);
        self.status = None;
    }

    fn handle_rev_picker_key(&mut self, key: KeyEvent) {
        let Some(mut selected) = self.rev_picker else {
            return;
        };
        let choices = self.revision_choices();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if selected + 1 < choices.len() {
                    selected += 1;
                }
                self.rev_picker = Some(selected);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                selected = selected.saturating_sub(1);
                self.rev_picker = Some(selected);
            }
            KeyCode::Enter => {
                self.viewing = choices[selected].0;
                self.load_view();
                self.status = Some(format!("Viewing {}.", self.view_label()));
            }
            KeyCode::Char('r') | KeyCode::Esc | KeyCode::Char('q') => {
                self.rev_picker = None;
            }
            _ => {}
        }
    }

    fn export_comments(&mut self) -> Option<ReviewOutcome> {
        if !self.ensure_writable() {
            return None;
        }
        self.stash_live_comments();
        Some(ReviewOutcome::Export(self.comments.clone()))
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.help || self.rev_picker.is_some() || !matches!(self.mode, Mode::Diff) {
            return;
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.select_at_pointer(mouse.column, mouse.row);
            }
            MouseEventKind::ScrollDown => self.move_diff_down(),
            MouseEventKind::ScrollUp => self.move_diff_up(),
            _ => {}
        }
    }

    fn handle_diff_key(&mut self, key: KeyEvent) -> Option<ReviewOutcome> {
        let context = self.review_context();
        let action = bindings::action_for(bindings::review_bindings(context), &key)?;
        match action {
            bindings::ReviewAction::MoveDown => self.move_diff_down(),
            bindings::ReviewAction::MoveUp => self.move_diff_up(),
            bindings::ReviewAction::First => self.select_diff(0),
            bindings::ReviewAction::Last => {
                self.select_diff(self.diff_rows().len().saturating_sub(1));
            }
            bindings::ReviewAction::NextFile => self.next_file(),
            bindings::ReviewAction::PreviousFile => self.previous_file(),
            bindings::ReviewAction::SelectOld => self.select_side(Side::Old),
            bindings::ReviewAction::SelectNew => self.select_side(Side::New),
            bindings::ReviewAction::ToggleFile => {
                self.toggle_selected_file();
            }
            bindings::ReviewAction::ToggleFileOrComments => {
                if !self.toggle_selected_file() {
                    self.mode = Mode::Comments;
                }
            }
            bindings::ReviewAction::AddComment => self.start_comment(),
            bindings::ReviewAction::EditComment => self.edit_selected_comment(),
            bindings::ReviewAction::DeleteComment => self.delete_selected_line_comment(),
            bindings::ReviewAction::OpenRevisions => self.open_rev_picker(),
            bindings::ReviewAction::OpenComments => self.mode = Mode::Comments,
            bindings::ReviewAction::ToggleLayout => self.toggle_diff_layout(),
            bindings::ReviewAction::ToggleInlineComments => {
                self.inline_comments = !self.inline_comments;
                let state = if self.inline_comments {
                    "shown"
                } else {
                    "hidden"
                };
                self.status = Some(format!("Inline comments {state}."));
            }
            bindings::ReviewAction::Export => return self.export_comments(),
            bindings::ReviewAction::Quit => return self.request_quit(),
            bindings::ReviewAction::Help => self.help = true,
            bindings::ReviewAction::ReturnToDiff => {
                unreachable!("diff keymap cannot contain comment-list actions")
            }
        }
        None
    }

    fn handle_comments_key(&mut self, key: KeyEvent) -> Option<ReviewOutcome> {
        let action = bindings::action_for(
            bindings::review_bindings(bindings::ReviewContext::Comments),
            &key,
        )?;
        match action {
            bindings::ReviewAction::MoveDown => {
                if self.selected_comment + 1 < self.comments.len() {
                    self.selected_comment += 1;
                }
            }
            bindings::ReviewAction::MoveUp => {
                self.selected_comment = self.selected_comment.saturating_sub(1);
            }
            bindings::ReviewAction::First => self.selected_comment = 0,
            bindings::ReviewAction::Last => {
                self.selected_comment = self.comments.len().saturating_sub(1);
            }
            bindings::ReviewAction::ReturnToDiff => self.mode = Mode::Diff,
            bindings::ReviewAction::OpenRevisions => self.open_rev_picker(),
            bindings::ReviewAction::EditComment => self.edit_selected_comment(),
            bindings::ReviewAction::DeleteComment => {
                if !self.comments.is_empty() {
                    self.delete_comment(self.selected_comment);
                    if self.comments.is_empty() {
                        self.mode = Mode::Diff;
                    }
                }
            }
            bindings::ReviewAction::Export => return self.export_comments(),
            bindings::ReviewAction::Quit => return self.request_quit(),
            bindings::ReviewAction::Help => self.help = true,
            bindings::ReviewAction::NextFile
            | bindings::ReviewAction::PreviousFile
            | bindings::ReviewAction::SelectOld
            | bindings::ReviewAction::SelectNew
            | bindings::ReviewAction::ToggleFile
            | bindings::ReviewAction::ToggleFileOrComments
            | bindings::ReviewAction::AddComment
            | bindings::ReviewAction::OpenComments
            | bindings::ReviewAction::ToggleInlineComments
            | bindings::ReviewAction::ToggleLayout => {
                unreachable!("comment-list keymap cannot contain diff actions")
            }
        }
        None
    }

    fn review_context(&self) -> bindings::ReviewContext {
        match self.visible_view() {
            View::Comments => bindings::ReviewContext::Comments,
            View::Diff if self.diff_layout == DiffLayout::Unified => {
                bindings::ReviewContext::Unified
            }
            View::Diff => bindings::ReviewContext::SideBySide,
        }
    }

    fn handle_input_key(&mut self, key: KeyEvent) -> Option<ReviewOutcome> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Diff;
                self.status = Some("Comment canceled.".to_owned());
            }
            KeyCode::Backspace => {
                if let Mode::CommentInput { body, .. } = &mut self.mode {
                    body.pop();
                }
            }
            KeyCode::Enter => {
                let mode = std::mem::replace(&mut self.mode, Mode::Diff);
                let Mode::CommentInput {
                    anchor,
                    body,
                    existing,
                } = mode
                else {
                    unreachable!("input handling requires comment-input mode");
                };
                let body = body.trim().to_owned();
                match existing {
                    Some(index) if body.is_empty() => self.delete_comment(index),
                    Some(index) => {
                        if let Some(comment) = self.comments.get_mut(index) {
                            comment.body = body;
                            self.selected_comment = index;
                            self.status = Some("Comment updated.".to_owned());
                        }
                    }
                    None if body.is_empty() => {
                        self.status = Some("Empty comment ignored.".to_owned());
                    }
                    None => {
                        self.comments.push(Comment::open(
                            anchor.path,
                            anchor.line,
                            anchor.side,
                            body,
                        ));
                        self.selected_comment = self.comments.len().saturating_sub(1);
                        self.status = Some("Comment added.".to_owned());
                    }
                }
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Mode::CommentInput { body, .. } = &mut self.mode {
                    body.push(character);
                }
            }
            _ => {}
        }
        None
    }

    fn handle_quit_key(&mut self, key: KeyEvent) -> Option<ReviewOutcome> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Some(ReviewOutcome::Quit),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('q') | KeyCode::Esc => {
                let mode = std::mem::replace(&mut self.mode, Mode::Diff);
                let Mode::QuitConfirm { previous } = mode else {
                    unreachable!("quit handling requires quit-confirm mode");
                };
                self.mode = match previous {
                    View::Diff => Mode::Diff,
                    View::Comments => Mode::Comments,
                };
                None
            }
            _ => None,
        }
    }
}

pub(crate) fn run(session: ReviewSession, submit_on_quit: bool) -> Result<ReviewOutcome> {
    with_terminal(|terminal| run_review(terminal, session, submit_on_quit))
}

fn with_terminal<T>(operation: impl FnOnce(&mut TrvTerminal) -> Result<T>) -> Result<T> {
    let restore = TerminalWriter::open()?;
    let size_tty = restore.size_tty()?;
    let output = restore.try_clone()?;
    let _terminal_mode = TerminalMode::enter(restore)?;
    let backend = SessionBackend {
        inner: CrosstermBackend::new(output),
        size_tty,
    };
    let mut terminal = Terminal::new(backend).context("failed to initialize terminal")?;
    terminal.clear().context("failed to clear terminal")?;
    operation(&mut terminal)
}

struct SessionBackend {
    inner: CrosstermBackend<TerminalWriter>,
    size_tty: Option<File>,
}

impl SessionBackend {
    fn measured_size(&self) -> io::Result<Size> {
        if let Some(tty) = &self.size_tty
            && let Ok((width, height)) = crate::tty::winsize(tty)
        {
            return Ok(Size { width, height });
        }
        self.inner.size()
    }
}

impl Backend for SessionBackend {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.inner.append_lines(n)
    }

    fn size(&self) -> io::Result<Size> {
        self.measured_size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        match self.measured_size() {
            Ok(size) => Ok(WindowSize {
                columns_rows: size,
                pixels: Size {
                    width: 0,
                    height: 0,
                },
            }),
            Err(_) => self.inner.window_size(),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner)
    }
}

fn run_review(
    terminal: &mut TrvTerminal,
    session: ReviewSession,
    submit_on_quit: bool,
) -> Result<ReviewOutcome> {
    let mut app = App::from_session(session);
    app.submit_on_quit = submit_on_quit;

    loop {
        terminal
            .draw(|frame| render(frame, &mut app))
            .context("failed to draw review")?;
        match event::read().context("failed to read terminal input")? {
            Event::Key(key) => {
                if let Some(outcome) = app.handle_key(key) {
                    return Ok(outcome);
                }
            }
            Event::Mouse(mouse) => app.handle_mouse(mouse),
            _ => {}
        }
    }
}

struct TerminalMode {
    restore: TerminalWriter,
    raw: bool,
    alternate: bool,
    mouse: bool,
}

impl TerminalMode {
    fn enter(restore: TerminalWriter) -> Result<Self> {
        let mut mode = Self {
            restore,
            raw: false,
            alternate: false,
            mouse: false,
        };
        enable_raw_mode().context("failed to enable terminal raw mode")?;
        mode.raw = true;

        execute!(&mut mode.restore, EnterAlternateScreen)
            .context("failed to enter alternate screen")?;
        mode.alternate = true;
        execute!(&mut mode.restore, EnableMouseCapture)
            .context("failed to enable mouse capture")?;
        mode.mouse = true;
        execute!(&mut mode.restore, Hide).context("failed to hide terminal cursor")?;
        Ok(mode)
    }
}

impl Drop for TerminalMode {
    fn drop(&mut self) {
        if self.mouse
            && let Err(error) = execute!(&mut self.restore, DisableMouseCapture)
        {
            eprintln!("trv: failed to disable mouse capture: {error}");
        }
        let screen_result = if self.alternate {
            execute!(&mut self.restore, Show, LeaveAlternateScreen)
        } else {
            execute!(&mut self.restore, Show)
        };
        if let Err(error) = screen_result {
            eprintln!("trv: failed to restore terminal screen: {error}");
        }
        if self.raw
            && let Err(error) = disable_raw_mode()
        {
            eprintln!("trv: failed to disable terminal raw mode: {error}");
        }
    }
}

fn render(frame: &mut Frame<'_>, app: &mut App) {
    let [content, footer] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());

    let selected_row = match app.visible_view() {
        View::Diff => diff_view::render(frame, content, app),
        View::Comments => {
            render_comments(frame, content, app);
            None
        }
    };
    render_footer(frame, footer, app);

    match &app.mode {
        Mode::CommentInput { body, existing, .. } => {
            comment_input::render(frame, content, body, selected_row, existing.is_some())
        }
        Mode::QuitConfirm { .. } => render_quit_confirm(frame, app.pending_comments()),
        Mode::Diff | Mode::Comments => {}
    }
    if app.help {
        let context = app.review_context();
        bindings::render_help(
            frame,
            context.help_title(),
            bindings::review_bindings(context),
        );
    }
    if app.rev_picker.is_some() {
        render_rev_picker(frame, app);
    }
}

fn render_comments(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let items = if app.comments.is_empty() {
        vec![ListItem::new("No comments.")]
    } else {
        app.comments
            .iter()
            .map(|comment| {
                ListItem::new(format!(
                    "{}:{} [{}] {}",
                    comment.path,
                    comment.line,
                    comment.side.as_str(),
                    comment.body
                ))
            })
            .collect()
    };
    let list = List::new(items)
        .block(Block::bordered().title(format!(" trv | comments | {} ", app.comments.len())))
        .highlight_symbol("> ")
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default();
    if !app.comments.is_empty() {
        state.select(Some(app.selected_comment));
    }
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    frame.render_widget(
        Paragraph::new(footer_text(app)).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn footer_text(app: &App) -> String {
    let detail = app.status.clone().unwrap_or_else(|| {
        if app.visible_view() == View::Diff {
            app.selected_anchor()
                .map(|anchor| format!("{}:{} [{}]", anchor.path, anchor.line, anchor.side.as_str()))
                .unwrap_or_default()
        } else {
            String::new()
        }
    });
    let controls = match app.visible_view() {
        View::Diff => {
            let inline_state = if app.inline_comments { "on" } else { "off" };
            if app.submit_on_quit {
                format!(
                    "r revs | {} | q send comments | s view: {} | {}",
                    app.view_label(),
                    app.diff_layout.as_str(),
                    bindings::HELP_HINT
                )
            } else {
                format!(
                    "r revs | {} | s view: {} | v inline comments: {inline_state} | {}",
                    app.view_label(),
                    app.diff_layout.as_str(),
                    bindings::HELP_HINT
                )
            }
        }
        View::Comments if app.submit_on_quit => format!(
            "l/Tab/Esc review | q send comments | {}",
            bindings::HELP_HINT
        ),
        View::Comments => format!(
            "l/Tab/Esc review | y export | q quit | {}",
            bindings::HELP_HINT
        ),
    };
    if detail.is_empty() {
        controls
    } else {
        format!("{controls} | {detail}")
    }
}

fn render_rev_picker(frame: &mut Frame<'_>, app: &App) {
    let choices = app.revision_choices();
    let selected = app.rev_picker.unwrap_or(0);
    let width = choices
        .iter()
        .map(|(_, label)| UnicodeWidthStr::width(label.as_str()))
        .max()
        .unwrap_or(0)
        .saturating_add(6)
        .max(24)
        .min(usize::from(frame.area().width)) as u16;
    let height = (choices.len() + 2).min(usize::from(frame.area().height)) as u16;
    let area = centered(frame.area(), width, height);
    let items = choices
        .into_iter()
        .map(|(_, label)| ListItem::new(format!(" {label}")))
        .collect::<Vec<_>>();
    let mut state = ListState::default();
    if !items.is_empty() {
        state.select(Some(selected));
    }
    frame.render_widget(Clear, area);
    frame.render_stateful_widget(
        List::new(items)
            .block(Block::bordered().title(" Revisions | Enter select | r close "))
            .highlight_symbol("> ")
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
        area,
        &mut state,
    );
}

fn render_quit_confirm(frame: &mut Frame<'_>, comment_count: usize) {
    let suffix = if comment_count == 1 { "" } else { "s" };
    let prompt = format!(" Discard {comment_count} unexported comment{suffix}? (y/N)");
    let screen = frame.area();
    let area = dock_bottom(screen, screen.width, 3);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(prompt).block(Block::bordered().title(" Quit ")),
        area,
    );
}

pub(super) fn centered(outer: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(outer.width);
    let height = height.min(outer.height);
    Rect::new(
        outer.x + outer.width.saturating_sub(width) / 2,
        outer.y + outer.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn dock_bottom(outer: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(outer.width);
    let height = height.min(outer.height);
    Rect::new(
        outer.x,
        outer.bottom().saturating_sub(height),
        width,
        height,
    )
}

#[cfg(test)]
mod tests;
