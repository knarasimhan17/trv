use std::fs::OpenOptions;
use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::model::Comment;

pub(crate) fn format_comments(comments: &[Comment]) -> String {
    comments
        .iter()
        .map(|comment| format!("{}: {}", comment.location(), comment.body))
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(crate) fn copy_to_clipboard(text: &str) -> Result<&'static str> {
    let mut failures = Vec::new();
    match write_osc52(text) {
        Ok(()) => return Ok("OSC52"),
        Err(error) => failures.push(format!("OSC52: {error:#}")),
    }

    match write_to_command("wl-copy", &[], text) {
        Ok(()) => return Ok("wl-copy"),
        Err(error) => failures.push(format!("wl-copy: {error:#}")),
    }
    match write_to_command("xclip", &["-selection", "clipboard"], text) {
        Ok(()) => return Ok("xclip"),
        Err(error) => failures.push(format!("xclip: {error:#}")),
    }

    bail!("clipboard export failed ({})", failures.join("; "))
}

fn write_osc52(text: &str) -> Result<()> {
    let mut terminal = OpenOptions::new()
        .write(true)
        .open("/dev/tty")
        .context("controlling terminal is unavailable")?;
    let encoded = STANDARD.encode(text.as_bytes());
    write!(terminal, "\x1b]52;c;{encoded}\x07").context("failed to write OSC52 sequence")?;
    terminal.flush().context("failed to flush OSC52 sequence")
}

fn write_to_command(program: &str, args: &[&str], text: &str) -> Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start {program}"))?;

    let write_result = match child.stdin.take() {
        Some(mut stdin) => stdin
            .write_all(text.as_bytes())
            .with_context(|| format!("failed to write to {program}")),
        None => Err(anyhow::anyhow!(
            "clipboard process stdin must be piped before writing"
        )),
    };
    let output = child
        .wait_with_output()
        .with_context(|| format!("failed to wait for {program}"))?;
    write_result?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!("{program} exited with {}: {}", output.status, stderr.trim())
}

#[cfg(test)]
mod tests {
    use crate::model::{Comment, Side};

    use super::format_comments;

    #[test]
    fn export_separates_comment_blocks_and_preserves_anchors() {
        let comments = vec![
            Comment::open(
                "src/lib.rs".to_owned(),
                12,
                Side::New,
                "Return the original error.".to_owned(),
            ),
            Comment::open(
                "src/main.rs".to_owned(),
                4,
                Side::Old,
                "Keep this validation.".to_owned(),
            ),
        ];

        assert_eq!(
            format_comments(&comments),
            "src/lib.rs:12: Return the original error.\n\nsrc/main.rs:4: Keep this validation.",
            "exports must contain blank-line-separated path and line anchors"
        );
    }

    #[test]
    fn export_formats_range_comments_as_start_end_spans() {
        let comments = vec![Comment::range(
            "src/lib.rs".to_owned(),
            12,
            18,
            Side::New,
            "Extract this block.".to_owned(),
        )];

        assert_eq!(
            format_comments(&comments),
            "src/lib.rs:12-18: Extract this block.",
            "range exports must keep a path:start-end: body shape agents can parse"
        );
    }
}
