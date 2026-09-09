use anyhow::Result;

use crate::diff::ParsedDiff;
use crate::export::CommentExportMeta;
use crate::git::{PreparedReview, Repository};
use crate::model::Comment;
use crate::persistence::Revision;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewKind {
    LiveMain,
    LiveSince(u64),
    Frozen(u64),
}

impl ViewKind {
    pub(crate) fn export_label(self) -> String {
        match self {
            Self::LiveMain => "current vs mainline".to_owned(),
            Self::LiveSince(rev) => format!("interdiff since rev-{rev}"),
            Self::Frozen(rev) => format!("frozen rev-{rev}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ViewedDiff {
    pub(crate) base_commit_sha: String,
    pub(crate) reviewed_sha: String,
    pub(crate) base_label: Option<String>,
    pub(crate) reviewed_label: Option<String>,
}

pub(crate) fn viewed_diff(
    view: ViewKind,
    prepared: &PreparedReview,
    revisions: &[Revision],
) -> ViewedDiff {
    match view {
        ViewKind::LiveMain => ViewedDiff {
            base_commit_sha: revisions
                .first()
                .map(|revision| revision.base_commit_sha.clone())
                .unwrap_or_else(|| prepared.base_commit_sha.clone()),
            reviewed_sha: prepared.tree_sha.clone(),
            base_label: None,
            reviewed_label: None,
        },
        ViewKind::LiveSince(rev) => ViewedDiff {
            base_commit_sha: revisions
                .iter()
                .find(|revision| revision.rev == rev)
                .map(|revision| revision.snapshot_commit_sha.clone())
                .unwrap_or_else(|| prepared.base_commit_sha.clone()),
            reviewed_sha: prepared.tree_sha.clone(),
            base_label: Some(format!("rev-{rev}")),
            reviewed_label: None,
        },
        ViewKind::Frozen(rev) => {
            let revision = revisions.iter().find(|revision| revision.rev == rev);
            ViewedDiff {
                base_commit_sha: revision
                    .map(|revision| revision.base_commit_sha.clone())
                    .unwrap_or_else(|| prepared.base_commit_sha.clone()),
                reviewed_sha: revision
                    .map(|revision| revision.snapshot_commit_sha.clone())
                    .unwrap_or_else(|| prepared.tree_sha.clone()),
                base_label: None,
                reviewed_label: Some(format!("rev-{rev}")),
            }
        }
    }
}

pub(crate) fn export_meta(
    repository: &Repository,
    thread: &str,
    prepared: &PreparedReview,
    view: ViewKind,
    revisions: &[Revision],
) -> CommentExportMeta {
    let mut viewed = viewed_diff(view, prepared, revisions);
    if matches!(view, ViewKind::Frozen(_))
        && let Ok(tree) = repository.commit_tree_sha(&viewed.reviewed_sha)
    {
        viewed.reviewed_sha = tree;
    }
    if viewed.base_label.is_none() {
        viewed.base_label = repository.describe_commit(&viewed.base_commit_sha);
    }
    if viewed.reviewed_label.is_none() {
        viewed.reviewed_label = repository
            .describe_tree(&viewed.reviewed_sha)
            .or_else(|| Some("current".to_owned()));
    }

    CommentExportMeta {
        repository: repository.name().to_owned(),
        branch: thread.to_owned(),
        view: view.export_label(),
        base_commit: viewed.base_commit_sha,
        reviewed: viewed.reviewed_sha,
        base_label: viewed.base_label,
        reviewed_label: viewed.reviewed_label,
    }
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

    use super::{ReviewSession, ViewKind, export_meta, viewed_diff};

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

    #[test]
    fn viewed_diff_uses_the_mainline_sticky_base_and_current_tree() {
        let directory = tempfile::tempdir().expect("temporary repository");
        let root = directory.path();
        init_repo(root);
        fs::write(root.join("file.rs"), "mainline\n").unwrap();
        run_git(root, &["add", "file.rs"]);
        run_git(root, &["commit", "--quiet", "-m", "mainline"]);
        let main_sha = git_sha(root, "HEAD");
        run_git(root, &["update-ref", "refs/remotes/origin/main", &main_sha]);
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

        let vs_main = viewed_diff(ViewKind::LiveMain, &current, &revisions);
        assert_eq!(vs_main.base_commit_sha, first.base_commit_sha);
        assert_eq!(vs_main.reviewed_sha, current.tree_sha);
        assert_eq!(vs_main.base_label, None);
        assert_eq!(ViewKind::LiveMain.export_label(), "current vs mainline");

        let since = viewed_diff(ViewKind::LiveSince(1), &current, &revisions);
        assert_eq!(since.base_commit_sha, revisions[0].snapshot_commit_sha);
        assert_eq!(since.reviewed_sha, current.tree_sha);
        assert_eq!(since.base_label.as_deref(), Some("rev-1"));
        assert_eq!(
            ViewKind::LiveSince(1).export_label(),
            "interdiff since rev-1"
        );

        let frozen = viewed_diff(ViewKind::Frozen(1), &current, &revisions);
        assert_eq!(frozen.base_commit_sha, revisions[0].base_commit_sha);
        assert_eq!(frozen.reviewed_sha, revisions[0].snapshot_commit_sha);
        assert_eq!(frozen.reviewed_label.as_deref(), Some("rev-1"));
        assert_eq!(ViewKind::Frozen(1).export_label(), "frozen rev-1");

        let vs_main_meta = export_meta(
            &repository,
            &thread,
            &current,
            ViewKind::LiveMain,
            &revisions,
        );
        assert_eq!(vs_main_meta.repository, repository.name());
        assert_eq!(vs_main_meta.branch, thread);
        assert_eq!(vs_main_meta.view, "current vs mainline");
        assert_eq!(vs_main_meta.base_commit, first.base_commit_sha);
        assert_eq!(vs_main_meta.reviewed, current.tree_sha);
        assert_eq!(vs_main_meta.base_label.as_deref(), Some("origin/main"));
        assert_eq!(vs_main_meta.reviewed_label.as_deref(), Some("current"));

        let since_meta = export_meta(
            &repository,
            &thread,
            &current,
            ViewKind::LiveSince(1),
            &revisions,
        );
        assert_eq!(since_meta.view, "interdiff since rev-1");
        assert_eq!(since_meta.base_commit, revisions[0].snapshot_commit_sha);
        assert_eq!(since_meta.base_label.as_deref(), Some("rev-1"));
        assert_eq!(since_meta.reviewed_label.as_deref(), Some("current"));

        let frozen_meta = export_meta(
            &repository,
            &thread,
            &current,
            ViewKind::Frozen(1),
            &revisions,
        );
        let frozen_tree = repository
            .commit_tree_sha(&revisions[0].snapshot_commit_sha)
            .unwrap();
        assert_eq!(frozen_meta.view, "frozen rev-1");
        assert_eq!(frozen_meta.reviewed, frozen_tree);
        assert_eq!(frozen_meta.reviewed_label.as_deref(), Some("rev-1"));
        assert_eq!(frozen_meta.base_label.as_deref(), Some("origin/main"));
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

    fn git_sha(root: &Path, revision: &str) -> String {
        let output = Command::new("git")
            .current_dir(root)
            .args(["rev-parse", revision])
            .output()
            .unwrap_or_else(|error| panic!("git rev-parse {revision} failed to start: {error}"));
        assert!(
            output.status.success(),
            "git rev-parse {revision} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
}
