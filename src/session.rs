use anyhow::Result;

use crate::diff::ParsedDiff;
use crate::git::{PreparedReview, Repository};
use crate::model::Comment;
use crate::persistence::Revision;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewKind {
    LiveMain,
    LiveSince(u64),
    Frozen(u64),
}

#[derive(Clone, Debug)]
pub(crate) struct LiveReview {
    pub(crate) vs_main: ParsedDiff,
    pub(crate) vs_previous: Option<(u64, ParsedDiff)>,
    pub(crate) comments: Vec<Comment>,
}

#[derive(Clone, Debug)]
pub(crate) struct FrozenReview {
    pub(crate) rev: u64,
    pub(crate) diff: ParsedDiff,
    pub(crate) comments: Vec<Comment>,
}

#[derive(Clone, Debug)]
pub(crate) struct ReviewSession {
    pub(crate) live: Option<LiveReview>,
    pub(crate) frozen: Vec<FrozenReview>,
    pub(crate) initial: ViewKind,
}

impl ReviewSession {
    #[cfg(test)]
    pub(crate) fn live_only(diff: ParsedDiff) -> Self {
        Self {
            live: Some(LiveReview {
                vs_main: diff,
                vs_previous: None,
                comments: Vec::new(),
            }),
            frozen: Vec::new(),
            initial: ViewKind::LiveMain,
        }
    }

    pub(crate) fn open(
        repository: &Repository,
        prepared: &PreparedReview,
        revisions: &[Revision],
    ) -> Result<Self> {
        let frozen = revisions
            .iter()
            .map(|revision| {
                let tree = repository.commit_tree_sha(&revision.snapshot_commit_sha)?;
                let diff = repository.diff_trees(&revision.base_commit_sha, &tree)?;
                Ok(FrozenReview {
                    rev: revision.rev,
                    diff: ParsedDiff::parse(&diff),
                    comments: revision.comments.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let last = revisions.last();
        let same_tree = last
            .map(|revision| {
                repository
                    .commit_tree_sha(&revision.snapshot_commit_sha)
                    .map(|tree| tree == prepared.tree_sha)
            })
            .transpose()?
            .unwrap_or(false);

        if same_tree {
            return Ok(Self {
                live: None,
                frozen,
                initial: ViewKind::Frozen(last.expect("same-tree requires a stored revision").rev),
            });
        }

        let vs_previous = last
            .map(|revision| {
                let diff =
                    repository.diff_trees(&revision.snapshot_commit_sha, &prepared.tree_sha)?;
                Ok::<_, anyhow::Error>((revision.rev, ParsedDiff::parse(&diff)))
            })
            .transpose()?;

        let mainline = last
            .map(|revision| revision.base_commit_sha.as_str())
            .unwrap_or(prepared.base_commit_sha.as_str());
        let vs_main = if mainline == prepared.base_commit_sha {
            ParsedDiff::parse(&prepared.diff)
        } else {
            ParsedDiff::parse(&repository.diff_trees(mainline, &prepared.tree_sha)?)
        };

        Ok(Self {
            live: Some(LiveReview {
                vs_main,
                vs_previous,
                comments: Vec::new(),
            }),
            frozen,
            initial: ViewKind::LiveMain,
        })
    }

    pub(crate) fn next_rev(&self) -> u64 {
        self.frozen
            .last()
            .map(|revision| revision.rev + 1)
            .unwrap_or(1)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use crate::git::Repository;
    use crate::model::{Comment, Side};
    use crate::persistence::{list_revisions, persist_revision};

    use super::{ReviewSession, ViewKind};

    #[test]
    fn unreviewed_edits_stay_on_the_next_draft_instead_of_minting_revisions() {
        let directory = tempfile::tempdir().expect("temporary repository");
        let root = directory.path();
        init_repo(root);
        fs::write(root.join("file.rs"), "first\n").unwrap();
        run_git(root, &["add", "file.rs"]);
        run_git(root, &["commit", "--quiet", "-m", "first"]);

        fs::write(root.join("file.rs"), "reviewed\n").unwrap();
        let repository = Repository::discover(root).unwrap();
        let thread = repository.current_thread().unwrap();
        let first = repository.prepare_review(None).unwrap();
        persist_revision(
            &repository,
            &thread,
            &first,
            vec![Comment::open(
                "file.rs".to_owned(),
                1,
                Side::New,
                "looks good".to_owned(),
            )],
        )
        .unwrap();

        fs::write(root.join("file.rs"), "agent one\n").unwrap();
        fs::write(root.join("file.rs"), "agent two\n").unwrap();
        let current = repository.prepare_review(None).unwrap();
        let revisions = list_revisions(repository.git_dir(), &thread).unwrap();
        let session = ReviewSession::open(&repository, &current, &revisions).unwrap();

        assert_eq!(
            revisions.len(),
            1,
            "unreviewed agent edits must not create rev-2"
        );
        assert_eq!(session.initial, ViewKind::LiveMain);
        let live = session
            .live
            .as_ref()
            .expect("changed tree must open a live draft");
        assert!(
            live.comments.is_empty(),
            "the current round must start clean"
        );
        assert_eq!(live.vs_previous.as_ref().map(|(rev, _)| *rev), Some(1));
        assert_eq!(session.frozen[0].comments[0].body, "looks good");
        assert!(
            current.diff.contains("agent two"),
            "the live review must show the latest unreviewed tree"
        );
    }

    #[test]
    fn later_working_tree_reviews_stay_against_the_original_mainline() {
        let directory = tempfile::tempdir().expect("temporary repository");
        let root = directory.path();
        init_repo(root);
        fs::write(root.join("file.rs"), "mainline\n").unwrap();
        run_git(root, &["add", "file.rs"]);
        run_git(root, &["commit", "--quiet", "-m", "mainline"]);
        fs::write(root.join("file.rs"), "reviewed\n").unwrap();
        run_git(root, &["add", "file.rs"]);
        run_git(root, &["commit", "--quiet", "-m", "pr"]);

        let repository = Repository::discover(root).unwrap();
        let thread = repository.current_thread().unwrap();
        let first = repository.prepare_review(Some("HEAD~1..HEAD")).unwrap();
        persist_revision(
            &repository,
            &thread,
            &first,
            vec![Comment::open(
                "file.rs".to_owned(),
                1,
                Side::New,
                "from rev-1".to_owned(),
            )],
        )
        .unwrap();

        fs::write(root.join("file.rs"), "reviewed\nplus\n").unwrap();
        let current = repository.prepare_review(None).unwrap();
        let revisions = list_revisions(repository.git_dir(), &thread).unwrap();
        let session = ReviewSession::open(&repository, &current, &revisions).unwrap();
        let live = session.live.as_ref().expect("changed tree must stay live");

        assert_eq!(session.initial, ViewKind::LiveMain);
        assert!(
            current.diff.contains("plus") && !current.diff.contains("mainline"),
            "prepare_review of a dirty tree is vs HEAD, not vs the original mainline"
        );
        assert!(
            live.vs_main
                .files
                .iter()
                .any(|file| file.lines.iter().any(|line| line.text == "plus")),
            "current vs mainline must include the new uncommitted line"
        );
        assert!(
            live.vs_main
                .files
                .iter()
                .any(|file| file.lines.iter().any(|line| line.text == "mainline")),
            "current vs mainline must still show the original mainline change, not only rev-1"
        );
        let since = live
            .vs_previous
            .as_ref()
            .map(|(_, diff)| diff)
            .expect("interdiff since rev-1");
        assert!(
            since
                .files
                .iter()
                .any(|file| file.lines.iter().any(|line| line.text == "plus"))
        );
        assert!(
            !since
                .files
                .iter()
                .any(|file| file.lines.iter().any(|line| line.text == "mainline")),
            "current vs rev-1 must not be used as the mainline view"
        );
    }

    #[test]
    fn reopening_the_same_tree_loads_the_frozen_revision() {
        let directory = tempfile::tempdir().expect("temporary repository");
        let root = directory.path();
        init_repo(root);
        fs::write(root.join("file.rs"), "body\n").unwrap();
        run_git(root, &["add", "file.rs"]);
        run_git(root, &["commit", "--quiet", "-m", "first"]);

        let repository = Repository::discover(root).unwrap();
        let thread = repository.current_thread().unwrap();
        let prepared = repository.prepare_review(None).unwrap();
        persist_revision(
            &repository,
            &thread,
            &prepared,
            vec![Comment::open(
                "file.rs".to_owned(),
                1,
                Side::New,
                "nit".to_owned(),
            )],
        )
        .unwrap();

        let again = repository.prepare_review(None).unwrap();
        let revisions = list_revisions(repository.git_dir(), &thread).unwrap();
        let session = ReviewSession::open(&repository, &again, &revisions).unwrap();

        assert!(session.live.is_none(), "the same tree is not a new round");
        assert_eq!(session.initial, ViewKind::Frozen(1));
        assert_eq!(session.frozen[0].comments[0].body, "nit");
    }

    fn init_repo(root: &Path) {
        run_git(root, &["init", "--quiet", "-b", "main"]);
        run_git(root, &["config", "user.name", "trv"]);
        run_git(root, &["config", "user.email", "trv@localhost"]);
    }

    fn run_git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap_or_else(|error| panic!("git {} failed to start: {error}", args.join(" ")));
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
