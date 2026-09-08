use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "trv",
    version,
    about = "Review Git changes in the terminal",
    args_conflicts_with_subcommands = true
)]
pub(crate) struct Cli {
    #[arg(short = 'r', long, value_name = "REVSET")]
    pub(crate) revset: Option<String>,

    #[arg(short = 'w', long, conflicts_with = "revset")]
    pub(crate) working_tree: bool,

    /// Review the current branch against its mainline (origin/main, main, …).
    #[arg(short = 'b', long, conflicts_with_all = ["revset", "working_tree"])]
    pub(crate) branch: bool,

    /// Write comments to stdout. Alias for --agent.
    #[arg(long)]
    pub(crate) stdout: bool,

    /// Open a review for an AI agent. Comments print on stdout when you quit.
    #[arg(long)]
    pub(crate) agent: bool,

    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

impl Cli {
    pub(crate) fn agent_mode(&self) -> bool {
        self.agent || self.stdout
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    Revs,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReviewLaunch {
    Direct { revset: Option<String> },
    Branch,
    Picker,
}

pub(crate) fn review_launch(
    revset: Option<&str>,
    working_tree: bool,
    branch: bool,
    agent: bool,
    has_uncommitted: bool,
    has_branch_stack: bool,
) -> ReviewLaunch {
    if let Some(revset) = revset {
        ReviewLaunch::Direct {
            revset: Some(revset.to_owned()),
        }
    } else if working_tree {
        ReviewLaunch::Direct { revset: None }
    } else if branch || (has_branch_stack && (agent || has_uncommitted)) {
        ReviewLaunch::Branch
    } else if has_uncommitted {
        ReviewLaunch::Direct { revset: None }
    } else {
        ReviewLaunch::Picker
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, ReviewLaunch, review_launch};

    #[test]
    fn agent_and_stdout_flags_enable_agent_mode() {
        assert!(
            Cli::parse_from(["trv", "--agent"]).agent_mode(),
            "--agent must close the review loop"
        );
        assert!(
            Cli::parse_from(["trv", "--stdout"]).agent_mode(),
            "--stdout is an alias for --agent"
        );
        assert!(
            !Cli::parse_from(["trv"]).agent_mode(),
            "interactive reviews must keep clipboard export"
        );
        assert!(
            Cli::parse_from(["trv", "-b"]).branch && Cli::parse_from(["trv", "--branch"]).branch,
            "-b / --branch must request a stack review against mainline"
        );
    }

    #[test]
    fn dirty_working_tree_opens_uncommitted_changes() {
        assert_eq!(
            review_launch(None, false, false, false, true, false),
            ReviewLaunch::Direct { revset: None },
            "uncommitted changes on mainline must skip the commit picker"
        );
    }

    #[test]
    fn clean_working_tree_opens_the_commit_picker() {
        assert_eq!(
            review_launch(None, false, false, false, false, false),
            ReviewLaunch::Picker,
            "a clean tree must let the user choose commits"
        );
    }

    #[test]
    fn working_tree_flag_skips_the_picker_even_when_clean() {
        assert_eq!(
            review_launch(None, true, false, false, false, true),
            ReviewLaunch::Direct { revset: None },
            "-w must still review the working tree directly"
        );
    }

    #[test]
    fn explicit_revset_wins_over_uncommitted_changes() {
        assert_eq!(
            review_launch(Some("HEAD~1"), false, false, false, true, true),
            ReviewLaunch::Direct {
                revset: Some("HEAD~1".to_owned()),
            },
            "-r must review the requested range even when the working tree is dirty"
        );
    }

    #[test]
    fn branch_flag_reviews_the_stack_against_mainline() {
        assert_eq!(
            review_launch(None, false, true, false, false, true),
            ReviewLaunch::Branch,
            "-b must review the current branch against mainline"
        );
    }

    #[test]
    fn a_dirty_feature_branch_reviews_the_whole_stack() {
        assert_eq!(
            review_launch(None, false, false, false, true, true),
            ReviewLaunch::Branch,
            "uncommitted work on a feature branch must join the stack vs mainline"
        );
    }

    #[test]
    fn agent_mode_reviews_a_feature_branch_as_one_stack() {
        assert_eq!(
            review_launch(None, false, false, true, false, true),
            ReviewLaunch::Branch,
            "--agent on a branch ahead of mainline must skip the commit picker"
        );
    }

    #[test]
    fn a_clean_feature_branch_still_opens_the_picker() {
        assert_eq!(
            review_launch(None, false, false, false, false, true),
            ReviewLaunch::Picker,
            "interactive reviews on a clean feature branch must still offer the picker"
        );
    }
}
