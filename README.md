# trv

`trv` is a terminal code-review tool for Git changes. It is designed around
vim-style navigation, inline comments, and immutable local review revisions. It
closes the loop with coding agents: `trv --agent` takes over the current
terminal for the review, then returns comments on stdout so the agent can
iterate in the same session.

## Revisions

Each comment export creates a new immutable revision for the current repository
and branch. `trv` stores the revision metadata and an exact Git snapshot of the
reviewed code inside the repository's `.git` directory. The review data does not
modify tracked worktree files and is not included in ordinary branch pushes.

Revisions are numbered in export order. A revision is created only when you
export a review you actually opened. Unreviewed agent edits do not mint a new
rev; the next time you review, that latest tree becomes the next round.

Creating rev-2 never changes rev-1. The default view is the current tree with
a clean comment set. Press `r` to open a saved revision (with its comments) or
to compare the current tree against the last review. Comments on the current
vs mainline view and on the interdiff both belong to the current round. Frozen
revisions stay read-only.

## Install

Run the setup script from the repository checkout:

```sh
./setup.sh
```

The script installs `trv`, bootstraps Rust when needed, and copies an [Agent
Skill](https://agentskills.io) into the usual global skill directories (Grok,
Claude Code, Codex, Cursor, Gemini, Copilot, and `~/.agents/skills`). After
that, coding agents already know to run `trv --agent` when they finish a
change. To install only the binary with an existing Rust toolchain, run:

```sh
cargo install --path .
```

## Usage

```sh
trv
trv -w
trv -r <revset>
trv revs
trv --agent
```

If the working tree has uncommitted changes, `trv` reviews them against `HEAD`
directly. If the working tree is clean, it opens a picker of the latest 200
commits on the current branch. Commits not found on any remote-tracking ref
are marked as unpushed. Selecting a commit opens a second picker for its base,
preselected to the commit's first parent.

`trv -w` (or `trv --working-tree`) reviews the current working tree against
`HEAD` directly, even when it is clean. `trv -r <revset>` reviews a commit or
revision range directly. Both flags skip the picker. Exporting comments creates
the next immutable revision for the current repository and branch.

`trv revs` lists the stored revisions.

`trv --agent` (or `trv --stdout`) is the agent loop. It takes over this
terminal for the review (the same session the agent is using), then returns
control when you quit. Add comments as usual, then press `q`. Quitting submits
whatever comments you left on stdout and unblocks the agent. Empty stdout means
you accepted the diff; that does not create a revision. Non-empty comments
persist the next immutable revision, same as a manual `y` export.

```sh
REVIEW=$(trv --agent)
# REVIEW is `path:line: body` blocks, or empty if you quit with no comments
```

The footer shows `q send comments` so it is obvious that quit returns the
review to the agent. If stdout is piped, the TUI draws on the controlling
terminal so the agent can still capture comments.

In the picker, use `j`/`k` to move, `Enter` to select, and `q` or `Esc` to go
back. Press `?` for the current picker's keybindings.

The review groups changes into file sections. Each file header shows its change
kind and added/deleted line counts; raw Git patch metadata is omitted. Files
start expanded. With a file header selected, use `Enter` or `Tab` to collapse or
expand it.

Inside a review, use `j`/`k` or the up/down arrows to move between lines, or
click a line with the mouse. The scroll wheel also moves the selection. Use
`]`/`[` to move between files, `g`/`G` to jump to the first or last line, `c`
to add a comment, or click an added or deleted line to comment on that side.
Click an existing comment to edit it, `d` to delete the
comment on the selected line, `l` to view comments, `r` to switch between the
current review, the interdiff since the last revision (also commentable), and
frozen revisions,
`s` to toggle unified or side-by-side layout, `v` to show or hide inline
comment rows, `y` to export, `?` to show context-aware help, and `q` to quit
(`q` sends comments in `--agent` mode). While editing, `Enter` saves,
`Esc` cancels, and `Ctrl-D` or an empty `Enter` deletes the comment. In the
comment list, `c`/`Enter` edits and `d` deletes. Press `?`, `Esc`, or `q` to close help and
return to the same screen. Unified layout is the default. In side-by-side
layout, use the left/right arrow keys or click a column to choose the old or
new side before adding a comment. Clicking a red (deleted) line comments on
the old side; clicking a green (added) line comments on the new side. Inline comments work
in both layouts and are shown by default; commented lines keep a `●` gutter
marker when inline rows are hidden.
