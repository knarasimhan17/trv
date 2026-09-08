use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, ErrorKind, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::git::{PreparedReview, Repository};
use crate::model::Comment;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Revision {
    pub(crate) rev: u64,
    pub(crate) timestamp: String,
    pub(crate) base_commit_sha: String,
    pub(crate) snapshot_commit_sha: String,
    pub(crate) comments: Vec<Comment>,
}

pub(crate) fn persist_revision(
    repository: &Repository,
    thread: &str,
    prepared: &PreparedReview,
    comments: Vec<Comment>,
) -> Result<Revision> {
    let sanitized_thread = sanitize_thread_name(thread);
    let thread_dir = repository.git_dir().join("trv").join(&sanitized_thread);
    fs::create_dir_all(&thread_dir)
        .with_context(|| format!("failed to create {}", thread_dir.display()))?;

    let rev = next_revision_number(&thread_dir)?;
    let base_commit_sha = list_revisions(repository.git_dir(), thread)?
        .first()
        .map(|revision| revision.base_commit_sha.clone())
        .unwrap_or_else(|| prepared.base_commit_sha.clone());
    let snapshot_commit_sha =
        repository.create_snapshot_commit(&prepared.tree_sha, &base_commit_sha, rev)?;
    let revision = Revision {
        rev,
        timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        base_commit_sha,
        snapshot_commit_sha: snapshot_commit_sha.clone(),
        comments,
    };
    validate_revision(&revision)?;

    let path = revision_path(&thread_dir, rev);
    write_revision(&path, &revision)?;
    if let Err(error) = repository.create_revision_ref(&sanitized_thread, rev, &snapshot_commit_sha)
    {
        return match fs::remove_file(&path) {
            Ok(()) => Err(error).context("failed to create immutable revision ref"),
            Err(cleanup_error) => Err(error).context(format!(
                "failed to create immutable revision ref; also failed to remove {}: {cleanup_error}",
                path.display()
            )),
        };
    }

    Ok(revision)
}

pub(crate) fn list_revisions(git_dir: &Path, thread: &str) -> Result<Vec<Revision>> {
    let thread_dir = git_dir.join("trv").join(sanitize_thread_name(thread));
    let entries = match fs::read_dir(&thread_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", thread_dir.display()));
        }
    };

    let mut revisions = Vec::new();
    for entry in entries {
        let entry = entry.context("failed to read revision directory entry")?;
        let Some(number) = revision_number_from_path(&entry.path()) else {
            continue;
        };
        let file = File::open(entry.path())
            .with_context(|| format!("failed to open {}", entry.path().display()))?;
        let revision: Revision = serde_json::from_reader(BufReader::new(file))
            .with_context(|| format!("failed to parse {}", entry.path().display()))?;
        validate_revision(&revision)
            .with_context(|| format!("invalid revision in {}", entry.path().display()))?;
        if revision.rev != number {
            bail!(
                "{} stores rev {} but its filename is rev-{number}.json",
                entry.path().display(),
                revision.rev
            );
        }
        revisions.push(revision);
    }
    revisions.sort_unstable_by_key(|revision| revision.rev);
    Ok(revisions)
}

pub(crate) fn next_revision_number(thread_dir: &Path) -> Result<u64> {
    let entries = match fs::read_dir(thread_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(1),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", thread_dir.display()));
        }
    };

    let mut highest = 0;
    for entry in entries {
        let entry = entry.context("failed to read revision directory entry")?;
        if let Some(number) = revision_number_from_path(&entry.path()) {
            highest = highest.max(number);
        }
    }
    highest
        .checked_add(1)
        .context("revision number space is exhausted")
}

pub(crate) fn sanitize_thread_name(name: &str) -> String {
    if name.is_empty() {
        return "%".to_owned();
    }

    let mut sanitized = String::with_capacity(name.len());
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            sanitized.push(char::from(byte));
        } else {
            sanitized.push('%');
            sanitized.push(hex_digit(byte >> 4));
            sanitized.push(hex_digit(byte & 0x0f));
        }
    }
    sanitized
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => char::from(b'0' + value),
        10..=15 => char::from(b'A' + value - 10),
        _ => unreachable!("a four-bit value must be in 0..=15, got {value}"),
    }
}

fn write_revision(path: &Path, revision: &Revision) -> Result<()> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("failed to create immutable {}", path.display()))?;
    let result = write_revision_contents(file, revision);
    if let Err(error) = result {
        return match fs::remove_file(path) {
            Ok(()) => Err(error).with_context(|| format!("failed to write {}", path.display())),
            Err(cleanup_error) if cleanup_error.kind() == ErrorKind::NotFound => {
                Err(error).with_context(|| format!("failed to write {}", path.display()))
            }
            Err(cleanup_error) => Err(error).context(format!(
                "failed to write {}; also failed to remove the partial file: {cleanup_error}",
                path.display()
            )),
        };
    }
    Ok(())
}

fn write_revision_contents(file: File, revision: &Revision) -> Result<()> {
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, revision).context("failed to serialize revision")?;
    writer
        .write_all(b"\n")
        .context("failed to terminate revision JSON")?;
    writer.flush().context("failed to flush revision JSON")?;
    writer
        .get_ref()
        .sync_all()
        .context("failed to sync revision JSON")
}

fn validate_revision(revision: &Revision) -> Result<()> {
    if revision.rev == 0 {
        bail!("revision number must be greater than zero");
    }
    DateTime::parse_from_rfc3339(&revision.timestamp)
        .context("revision timestamp must be RFC 3339")?;
    validate_sha("base commit", &revision.base_commit_sha)?;
    validate_sha("snapshot commit", &revision.snapshot_commit_sha)?;
    for comment in &revision.comments {
        if comment.path.is_empty() {
            bail!("comment path must not be empty");
        }
        if comment.line == 0 {
            bail!("comment line must be greater than zero");
        }
        if comment.body.trim().is_empty() {
            bail!("comment body must not be empty");
        }
    }
    Ok(())
}

fn validate_sha(label: &str, sha: &str) -> Result<()> {
    if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("{label} SHA is not a full Git object ID: {sha}");
    }
    Ok(())
}

fn revision_path(thread_dir: &Path, rev: u64) -> PathBuf {
    thread_dir.join(format!("rev-{rev}.json"))
}

fn revision_number_from_path(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    let number = name.strip_prefix("rev-")?.strip_suffix(".json")?;
    number.parse().ok()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{next_revision_number, sanitize_thread_name};

    #[test]
    fn revision_numbers_start_at_one_and_follow_the_highest_revision() {
        let directory =
            tempfile::tempdir().expect("temporary directory creation must succeed for this test");
        assert_eq!(
            next_revision_number(directory.path())
                .expect("an empty revision directory must have a next number"),
            1,
            "the first revision number must be one"
        );

        // The gap proves numbering follows the durable maximum rather than the file count.
        fs::write(directory.path().join("rev-1.json"), "{}")
            .expect("the first synthetic revision must be writable");
        fs::write(directory.path().join("rev-7.json"), "{}")
            .expect("the gapped synthetic revision must be writable");

        assert_eq!(
            next_revision_number(directory.path())
                .expect("an existing revision directory must have a next number"),
            8,
            "the next revision must follow the highest durable revision"
        );
    }

    #[test]
    fn thread_names_are_encoded_into_one_filesystem_component() {
        assert_eq!(
            sanitize_thread_name("feature/review 100%"),
            "feature%2Freview%20100%25",
            "sanitization must preserve identity without path separators"
        );
    }
}
