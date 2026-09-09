use std::fs::OpenOptions;
use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::model::Comment;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommentExportMeta {
    pub(crate) repository: String,
    pub(crate) branch: String,
    pub(crate) view: String,
    pub(crate) base_commit: String,
    pub(crate) reviewed: String,
    pub(crate) base_label: Option<String>,
    pub(crate) reviewed_label: Option<String>,
}

pub(crate) fn format_comments(comments: &[Comment], meta: &CommentExportMeta) -> String {
    if comments.is_empty() {
        return String::new();
    }

    format!(
        "{}\n\n{}",
        format_header(meta),
        format_comment_blocks(comments)
    )
}

fn format_header(meta: &CommentExportMeta) -> String {
    [
        format!("repository: {}", meta.repository),
        format!("branch: {}", meta.branch),
        format!("view: {}", meta.view),
        format_sha_field("base", &meta.base_commit, meta.base_label.as_deref()),
        format_sha_field("reviewed", &meta.reviewed, meta.reviewed_label.as_deref()),
    ]
    .join("\n")
}

fn format_sha_field(key: &str, sha: &str, label: Option<&str>) -> String {
    match label {
        Some(label) if !label.is_empty() => format!("{key}: {sha} ({label})"),
        _ => format!("{key}: {sha}"),
    }
}

fn format_comment_blocks(comments: &[Comment]) -> String {
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

    use super::{CommentExportMeta, format_comments};

    fn sample_meta() -> CommentExportMeta {
        CommentExportMeta {
            repository: "trv".to_owned(),
            branch: "feat/review".to_owned(),
            view: "current vs mainline".to_owned(),
            base_commit: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            reviewed: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
            base_label: Some("origin/main".to_owned()),
            reviewed_label: Some("HEAD".to_owned()),
        }
    }

    fn comment_blocks(export: &str) -> &str {
        export
            .split_once("\n\n")
            .map(|(_, blocks)| blocks)
            .unwrap_or(export)
    }

    #[test]
    fn empty_comments_export_as_an_empty_string() {
        assert_eq!(
            format_comments(&[], &sample_meta()),
            "",
            "accepting a diff must not emit a metadata-only payload"
        );
    }

    #[test]
    fn export_prefixes_comments_with_the_reviewed_diff_header() {
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

        let export = format_comments(&comments, &sample_meta());
        assert_eq!(
            export,
            "\
repository: trv
branch: feat/review
view: current vs mainline
base: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa (origin/main)
reviewed: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb (HEAD)

src/lib.rs:12: Return the original error.

src/main.rs:4: Keep this validation.",
            "agents need the reviewed diff identity before the comment blocks"
        );
        assert_eq!(
            comment_blocks(&export),
            "src/lib.rs:12: Return the original error.\n\nsrc/main.rs:4: Keep this validation.",
            "comment blocks after the header must stay blank-line-separated path:line: body"
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
        let mut meta = sample_meta();
        meta.view = "interdiff since rev-2".to_owned();
        meta.base_label = Some("rev-2".to_owned());
        meta.reviewed_label = Some("current".to_owned());

        let export = format_comments(&comments, &meta);
        assert!(
            export.starts_with(
                "\
repository: trv
branch: feat/review
view: interdiff since rev-2
base: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa (rev-2)
reviewed: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb (current)\n\n"
            ),
            "interdiff exports must name the view and rev-N base: {export}"
        );
        assert_eq!(
            comment_blocks(&export),
            "src/lib.rs:12-18: Extract this block.",
            "range exports must keep a path:start-end: body shape agents can parse"
        );
    }

    #[test]
    fn export_omits_parentheses_when_a_human_label_is_missing() {
        let comments = vec![Comment::open(
            "src/lib.rs".to_owned(),
            12,
            Side::New,
            "Return the original error.".to_owned(),
        )];
        let mut meta = sample_meta();
        meta.view = "frozen rev-1".to_owned();
        meta.base_label = None;
        meta.reviewed_label = Some("rev-1".to_owned());

        let export = format_comments(&comments, &meta);
        assert!(
            export.contains(
                "view: frozen rev-1\nbase: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nreviewed: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb (rev-1)\n"
            ),
            "missing labels must not invent placeholders: {export}"
        );
        assert_eq!(
            comment_blocks(&export),
            "src/lib.rs:12: Return the original error."
        );
    }
}
