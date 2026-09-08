---
name: trv
description: >
  After making code changes in a Git repository, run `trv --agent` so the user
  can review the diff. It opens in this window (a split pane when the agent has
  no tty). Quitting returns comments on stdout; iterate until they quit with no
  comments. Use when finishing a feature, implementing a change, editing code,
  asking the user to review, or when the user says "trv", "review this", or
  "/trv".
---

# Review with trv

This is the default way to hand work back to the user in a Git repo. After you make code changes, run the review. Do not ask the user to paste comments.

1. From the repository root, run `trv --agent` in the foreground and wait until it exits. If you have no tty, trv opens a review tab in this window without stealing focus; the user will switch to it when ready. Do not background the command. The user may take many minutes; set the command timeout to at least 3600000 ms (1 hour).
2. Stdout is blank-line-separated comments: `path:line: body` for a single line, or `path:start-end: body` for a range. Empty stdout means they accepted the diff. Address every line in a range.
3. If there are comments, address them and run `trv --agent` again.
4. Repeat until stdout is empty.

If `trv` is missing, tell the user to install it with `./setup.sh` from a trv checkout.
