use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::centered;

pub(super) const HELP_HINT: &str = "? help";

#[derive(Clone, Copy)]
pub(super) enum PickerContext {
    Source,
    Base,
}

impl PickerContext {
    pub(super) fn help_title(self) -> &'static str {
        match self {
            Self::Source => "commit picker",
            Self::Base => "base picker",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum PickerAction {
    MoveDown,
    MoveUp,
    Select,
    Back,
    Quit,
    Help,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReviewContext {
    Unified,
    SideBySide,
    Comments,
    FileTree,
    CommentsPanel,
}

impl ReviewContext {
    pub(super) fn help_title(self) -> &'static str {
        match self {
            Self::Unified => "unified review",
            Self::SideBySide => "side-by-side review",
            Self::Comments => "comment list",
            Self::FileTree => "file list",
            Self::CommentsPanel => "comments panel",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum ReviewAction {
    MoveDown,
    MoveUp,
    First,
    Last,
    NextFile,
    PreviousFile,
    SelectOld,
    SelectNew,
    ToggleFile,
    ToggleFileOrComments,
    ToggleFileTree,
    ActivateFile,
    AddComment,
    EditComment,
    DeleteComment,
    OpenComments,
    FocusCommentsPanel,
    JumpToComment,
    OpenRevisions,
    ReturnToDiff,
    StartVisual,
    ToggleInlineComments,
    ToggleLayout,
    Export,
    Cancel,
    Quit,
    Help,
}

#[derive(Clone, Copy)]
enum KeyPattern {
    Code(KeyCode),
    Modified(KeyCode, KeyModifiers),
}

impl KeyPattern {
    fn matches(self, key: &KeyEvent) -> bool {
        match self {
            Self::Code(code) => key.code == code,
            Self::Modified(code, modifiers) => {
                key.code == code && key.modifiers.contains(modifiers)
            }
        }
    }

    fn is_modified(self) -> bool {
        matches!(self, Self::Modified(..))
    }
}

pub(super) struct Binding<Action> {
    keys: &'static [KeyPattern],
    label: &'static str,
    description: &'static str,
    action: Action,
}

#[derive(Clone, Copy)]
enum ReviewScope {
    All,
    Diff,
    SideBySide,
    Comments,
    FileTree,
    DiffOrFileTree,
    CommentsPanel,
}

impl ReviewScope {
    fn includes(self, context: ReviewContext) -> bool {
        match self {
            Self::All => true,
            Self::Diff => matches!(context, ReviewContext::Unified | ReviewContext::SideBySide),
            Self::SideBySide => context == ReviewContext::SideBySide,
            Self::Comments => context == ReviewContext::Comments,
            Self::FileTree => context == ReviewContext::FileTree,
            Self::DiffOrFileTree => matches!(
                context,
                ReviewContext::Unified | ReviewContext::SideBySide | ReviewContext::FileTree
            ),
            Self::CommentsPanel => context == ReviewContext::CommentsPanel,
        }
    }
}

struct ReviewBinding {
    scope: ReviewScope,
    binding: Binding<ReviewAction>,
}

const fn plain(code: KeyCode) -> KeyPattern {
    KeyPattern::Code(code)
}

const fn modified(code: KeyCode, modifiers: KeyModifiers) -> KeyPattern {
    KeyPattern::Modified(code, modifiers)
}

const fn binding<Action>(
    keys: &'static [KeyPattern],
    label: &'static str,
    description: &'static str,
    action: Action,
) -> Binding<Action> {
    Binding {
        keys,
        label,
        description,
        action,
    }
}

const fn review_binding(
    scope: ReviewScope,
    keys: &'static [KeyPattern],
    label: &'static str,
    description: &'static str,
    action: ReviewAction,
) -> ReviewBinding {
    ReviewBinding {
        scope,
        binding: binding(keys, label, description, action),
    }
}

const SOURCE_PICKER_BINDINGS: &[Binding<PickerAction>] = &[
    binding(
        &[plain(KeyCode::Char('j')), plain(KeyCode::Down)],
        "j / Down",
        "Move to the next review source",
        PickerAction::MoveDown,
    ),
    binding(
        &[plain(KeyCode::Char('k')), plain(KeyCode::Up)],
        "k / Up",
        "Move to the previous review source",
        PickerAction::MoveUp,
    ),
    binding(
        &[plain(KeyCode::Enter)],
        "Enter",
        "Choose this source",
        PickerAction::Select,
    ),
    binding(
        &[plain(KeyCode::Char('q')), plain(KeyCode::Esc)],
        "q / Esc",
        "Quit the picker",
        PickerAction::Back,
    ),
    binding(
        &[modified(KeyCode::Char('c'), KeyModifiers::CONTROL)],
        "Ctrl-C",
        "Quit the picker",
        PickerAction::Quit,
    ),
    binding(
        &[plain(KeyCode::Char('?'))],
        "?",
        "Open or close help",
        PickerAction::Help,
    ),
];

const BASE_PICKER_BINDINGS: &[Binding<PickerAction>] = &[
    binding(
        &[plain(KeyCode::Char('j')), plain(KeyCode::Down)],
        "j / Down",
        "Move to the next base",
        PickerAction::MoveDown,
    ),
    binding(
        &[plain(KeyCode::Char('k')), plain(KeyCode::Up)],
        "k / Up",
        "Move to the previous base",
        PickerAction::MoveUp,
    ),
    binding(
        &[plain(KeyCode::Enter)],
        "Enter",
        "Review against this base",
        PickerAction::Select,
    ),
    binding(
        &[plain(KeyCode::Char('q')), plain(KeyCode::Esc)],
        "q / Esc",
        "Return to the source picker",
        PickerAction::Back,
    ),
    binding(
        &[modified(KeyCode::Char('c'), KeyModifiers::CONTROL)],
        "Ctrl-C",
        "Quit the picker",
        PickerAction::Quit,
    ),
    binding(
        &[plain(KeyCode::Char('?'))],
        "?",
        "Open or close help",
        PickerAction::Help,
    ),
];

const REVIEW_BINDINGS: &[ReviewBinding] = &[
    review_binding(
        ReviewScope::All,
        &[plain(KeyCode::Char('j')), plain(KeyCode::Down)],
        "j / Down",
        "Move down",
        ReviewAction::MoveDown,
    ),
    review_binding(
        ReviewScope::All,
        &[plain(KeyCode::Char('k')), plain(KeyCode::Up)],
        "k / Up",
        "Move up",
        ReviewAction::MoveUp,
    ),
    review_binding(
        ReviewScope::All,
        &[plain(KeyCode::Char('g')), plain(KeyCode::Home)],
        "g / Home",
        "Jump to the first item",
        ReviewAction::First,
    ),
    review_binding(
        ReviewScope::All,
        &[plain(KeyCode::Char('G')), plain(KeyCode::End)],
        "G / End",
        "Jump to the last item",
        ReviewAction::Last,
    ),
    review_binding(
        ReviewScope::DiffOrFileTree,
        &[plain(KeyCode::Char(']'))],
        "]",
        "Jump to the next file",
        ReviewAction::NextFile,
    ),
    review_binding(
        ReviewScope::DiffOrFileTree,
        &[plain(KeyCode::Char('['))],
        "[",
        "Jump to the previous file",
        ReviewAction::PreviousFile,
    ),
    review_binding(
        ReviewScope::SideBySide,
        &[plain(KeyCode::Left)],
        "Left",
        "Select the old side",
        ReviewAction::SelectOld,
    ),
    review_binding(
        ReviewScope::SideBySide,
        &[plain(KeyCode::Right)],
        "Right",
        "Select the new side",
        ReviewAction::SelectNew,
    ),
    review_binding(
        ReviewScope::Diff,
        &[plain(KeyCode::Enter)],
        "Enter",
        "Collapse or expand the selected file",
        ReviewAction::ToggleFile,
    ),
    review_binding(
        ReviewScope::FileTree,
        &[plain(KeyCode::Enter)],
        "Enter",
        "Jump to the selected file",
        ReviewAction::ActivateFile,
    ),
    review_binding(
        ReviewScope::DiffOrFileTree,
        &[plain(KeyCode::Char('f'))],
        "f",
        "Show or hide the file list",
        ReviewAction::ToggleFileTree,
    ),
    review_binding(
        ReviewScope::FileTree,
        &[plain(KeyCode::Esc)],
        "Esc",
        "Return to the review",
        ReviewAction::ReturnToDiff,
    ),
    review_binding(
        ReviewScope::Diff,
        &[plain(KeyCode::Tab)],
        "Tab",
        "Toggle a file, or open the comment list",
        ReviewAction::ToggleFileOrComments,
    ),
    review_binding(
        ReviewScope::Diff,
        &[plain(KeyCode::Char('c'))],
        "c",
        "Add a comment to the selected line or range",
        ReviewAction::AddComment,
    ),
    review_binding(
        ReviewScope::Diff,
        &[plain(KeyCode::Char('d'))],
        "d",
        "Delete the comment on the selected line",
        ReviewAction::DeleteComment,
    ),
    // Mouse-only: empty keys so click-to-edit shows in help without a key match.
    review_binding(
        ReviewScope::Diff,
        &[],
        "click comment",
        "Edit an existing inline comment",
        ReviewAction::EditComment,
    ),
    review_binding(
        ReviewScope::Comments,
        &[plain(KeyCode::Char('c')), plain(KeyCode::Enter)],
        "c / Enter",
        "Edit the selected comment",
        ReviewAction::EditComment,
    ),
    review_binding(
        ReviewScope::Comments,
        &[plain(KeyCode::Char('d'))],
        "d",
        "Delete the selected comment",
        ReviewAction::DeleteComment,
    ),
    review_binding(
        ReviewScope::Diff,
        &[plain(KeyCode::Char('l'))],
        "l",
        "Open the comment list",
        ReviewAction::OpenComments,
    ),
    review_binding(
        ReviewScope::Diff,
        &[plain(KeyCode::Char('C'))],
        "C",
        "Focus the comments panel",
        ReviewAction::FocusCommentsPanel,
    ),
    review_binding(
        ReviewScope::CommentsPanel,
        &[plain(KeyCode::Enter)],
        "Enter",
        "Jump to the selected comment in the diff",
        ReviewAction::JumpToComment,
    ),
    review_binding(
        ReviewScope::CommentsPanel,
        &[plain(KeyCode::Char('C')), plain(KeyCode::Esc)],
        "C / Esc",
        "Return to the diff",
        ReviewAction::ReturnToDiff,
    ),
    review_binding(
        ReviewScope::CommentsPanel,
        &[plain(KeyCode::Char('l')), plain(KeyCode::Tab)],
        "l / Tab",
        "Open the comment list",
        ReviewAction::OpenComments,
    ),
    review_binding(
        ReviewScope::Comments,
        &[
            plain(KeyCode::Char('l')),
            plain(KeyCode::Tab),
            plain(KeyCode::Esc),
        ],
        "l / Tab / Esc",
        "Return to the review",
        ReviewAction::ReturnToDiff,
    ),
    review_binding(
        ReviewScope::Diff,
        &[plain(KeyCode::Char('v')), plain(KeyCode::Char('V'))],
        "v / V",
        "Start or cancel a line range",
        ReviewAction::StartVisual,
    ),
    review_binding(
        ReviewScope::Diff,
        &[plain(KeyCode::Char('i'))],
        "i",
        "Show or hide inline comments",
        ReviewAction::ToggleInlineComments,
    ),
    review_binding(
        ReviewScope::Diff,
        &[plain(KeyCode::Char('s'))],
        "s",
        "Switch unified and side-by-side views",
        ReviewAction::ToggleLayout,
    ),
    review_binding(
        ReviewScope::All,
        &[plain(KeyCode::Char('r'))],
        "r",
        "Switch between the current review and saved revisions",
        ReviewAction::OpenRevisions,
    ),
    review_binding(
        ReviewScope::All,
        &[plain(KeyCode::Char('y'))],
        "y",
        "Export comments",
        ReviewAction::Export,
    ),
    review_binding(
        ReviewScope::Diff,
        &[plain(KeyCode::Esc)],
        "Esc",
        "Cancel a range, or quit",
        ReviewAction::Cancel,
    ),
    review_binding(
        ReviewScope::DiffOrFileTree,
        &[plain(KeyCode::Char('q'))],
        "q",
        "Quit the review",
        ReviewAction::Quit,
    ),
    review_binding(
        ReviewScope::Comments,
        &[plain(KeyCode::Char('q'))],
        "q",
        "Quit the review",
        ReviewAction::Quit,
    ),
    review_binding(
        ReviewScope::CommentsPanel,
        &[plain(KeyCode::Char('q'))],
        "q",
        "Quit the review",
        ReviewAction::Quit,
    ),
    review_binding(
        ReviewScope::All,
        &[modified(KeyCode::Char('c'), KeyModifiers::CONTROL)],
        "Ctrl-C",
        "Quit the review",
        ReviewAction::Quit,
    ),
    review_binding(
        ReviewScope::All,
        &[plain(KeyCode::Char('?'))],
        "?",
        "Open or close help",
        ReviewAction::Help,
    ),
];

pub(super) fn picker_bindings(context: PickerContext) -> &'static [Binding<PickerAction>] {
    match context {
        PickerContext::Source => SOURCE_PICKER_BINDINGS,
        PickerContext::Base => BASE_PICKER_BINDINGS,
    }
}

pub(super) fn review_bindings(
    context: ReviewContext,
) -> impl Iterator<Item = &'static Binding<ReviewAction>> {
    REVIEW_BINDINGS
        .iter()
        .filter(move |binding| binding.scope.includes(context))
        .map(|binding| &binding.binding)
}

pub(super) fn action_for<'a, Action: Copy + 'a>(
    bindings: impl IntoIterator<Item = &'a Binding<Action>>,
    key: &KeyEvent,
) -> Option<Action> {
    let mut plain_action = None;
    for binding in bindings {
        for pattern in binding.keys {
            if !pattern.matches(key) {
                continue;
            }
            if pattern.is_modified() {
                return Some(binding.action);
            }
            plain_action.get_or_insert(binding.action);
        }
    }
    plain_action
}

pub(super) fn closes_help(key: &KeyEvent) -> bool {
    matches!(
        key.code,
        KeyCode::Char('?') | KeyCode::Char('q') | KeyCode::Esc
    )
}

pub(super) fn render_help<'a, Action: 'a>(
    frame: &mut Frame<'_>,
    context: &str,
    bindings: impl IntoIterator<Item = &'a Binding<Action>>,
) {
    let bindings = bindings.into_iter().collect::<Vec<_>>();
    let key_width = bindings
        .iter()
        .map(|binding| UnicodeWidthStr::width(binding.label))
        .max()
        .unwrap_or(0);
    let description_width = bindings
        .iter()
        .map(|binding| UnicodeWidthStr::width(binding.description))
        .max()
        .unwrap_or(0);
    let title = format!(" Help | {context} | ? Esc q close ");
    let desired_width = key_width
        .saturating_add(description_width)
        .saturating_add(6)
        .max(UnicodeWidthStr::width(title.as_str()).saturating_add(2));
    let screen = frame.area();
    let width = desired_width.min(usize::from(screen.width)) as u16;
    let height = bindings
        .len()
        .saturating_add(2)
        .min(usize::from(screen.height)) as u16;
    let area = centered(screen, width, height);
    let lines = bindings
        .into_iter()
        .map(|binding| {
            let padding =
                " ".repeat(key_width.saturating_sub(UnicodeWidthStr::width(binding.label)));
            Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    binding.label,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(padding),
                Span::raw("  "),
                Span::raw(binding.description),
            ])
        })
        .collect::<Vec<_>>();

    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(title)),
        area,
    );
}
