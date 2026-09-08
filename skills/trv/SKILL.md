---
name: trv
description: >
  After making code changes in a Git repository, run `trv --agent` so the user
  can review the diff in this terminal. Quitting returns comments on stdout;
  iterate until they quit with no comments. Use when finishing a feature,
  implementing a change, editing code, asking the user to review, or when
  the user says "trv", "review this", or "/trv".
---

# Review with trv

This is the default way to hand work back to the user in a Git repo. After you make code changes, run the review in this session. Do not ask the user to paste comments.

1. From the repository root, run `trv --agent` in the foreground and wait until it exits. It takes over this terminal for the review, then returns here. The user may take many minutes; set the command timeout to at least 3600000 ms (1 hour).
2. Stdout is blank-line-separated `path:line: body` comments. Empty stdout means they accepted the diff.
3. If there are comments, address them and run `trv --agent` again.
4. Repeat until stdout is empty.

If `trv` is missing, tell the user to install it with `./setup.sh` from a trv checkout.
