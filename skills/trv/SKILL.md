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

1. From the repository root, run `trv --agent` in the foreground and wait until it exits. Do not run bare `trv`; that opens an interactive commit picker. If the current branch is ahead of mainline, `--agent` skips the picker and reviews the whole stack (every commit plus uncommitted work) against that mainline, like a pull request. If you have no tty, trv still shows the review in this session without stealing focus (the SSH tty, or a tab in Warp/tmux/iTerm/Kitty/WezTerm); the user will switch to it when ready. Do not background the command. The user may take many minutes; set the command timeout to at least 3600000 ms (1 hour).
2. Empty stdout means they accepted the diff. Do not treat a header with no comments as a review; that payload is never emitted. Otherwise stdout is:

   ```
   repository: <repo>
   branch: <branch>
   view: current vs mainline | interdiff since rev-N | frozen rev-N
   base: <commit-sha> (<origin/main|HEAD|rev-N>)
   reviewed: <tree-or-commit-sha> (<HEAD|current|rev-N>)

   path:line: body

   path:start-end: body
   ```

   Parenthetical labels are omitted when unknown. Address every line in a range. Use the header to act on the diff the user reviewed, not a different revision.

3. If there are comments, address them and run `trv --agent` again.
4. Repeat until stdout is empty.

If `trv` is missing, tell the user to install it with `./setup.sh` from a trv checkout.
