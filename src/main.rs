mod agent;
mod cli;
mod diff;
mod export;
mod git;
mod model;
mod persistence;
mod session;
mod tui;

use std::env;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;

use crate::cli::{Cli, Command, ReviewLaunch, review_launch};
use crate::export::{copy_to_clipboard, format_comments};
use crate::git::{PreparedReview, Repository};
use crate::persistence::{list_revisions, persist_revision};
use crate::session::ReviewSession;
use crate::tui::{CommitPickerOutcome, ReviewOutcome};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("trv: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let current_dir = env::current_dir().context("current directory is unavailable")?;
    let repository = Repository::discover(&current_dir)?;

    match cli.command {
        Some(Command::Revs) => print_revisions(&repository),
        None => run_review(
            &repository,
            cli.revset.as_deref(),
            cli.working_tree,
            cli.agent_mode(),
        ),
    }
}

fn print_revisions(repository: &Repository) -> Result<()> {
    let thread = repository.current_thread()?;
    for revision in list_revisions(repository.git_dir(), &thread)? {
        println!(
            "{}\t{}\t{}\t{}",
            revision.rev,
            revision.timestamp,
            revision.comments.len(),
            revision.base_commit_sha
        );
    }
    Ok(())
}

fn run_review(
    repository: &Repository,
    revset: Option<&str>,
    working_tree: bool,
    agent: bool,
) -> Result<()> {
    let thread = repository.current_thread()?;
    match review_launch(revset, working_tree, repository.has_uncommitted_changes()?) {
        ReviewLaunch::Direct { revset } => {
            let prepared = repository.prepare_review(revset.as_deref())?;
            let session = open_review_session(repository, &thread, &prepared)?;
            let outcome = tui::run(session, agent)?;
            finish_review(repository, &thread, prepared, outcome, agent)
        }
        ReviewLaunch::Picker => {
            let commits = repository.recent_commits()?;
            let outcome = tui::run_picker(
                commits,
                |target| {
                    let revset = format!("{}..{}", target.base_sha, target.source_sha);
                    repository.prepare_review(Some(&revset))
                },
                |prepared| open_review_session(repository, &thread, prepared),
                agent,
            )?;

            match outcome {
                CommitPickerOutcome::Reviewed { prepared, outcome } => {
                    finish_review(repository, &thread, prepared, outcome, agent)
                }
                CommitPickerOutcome::Quit => {
                    if agent {
                        agent::deliver("")?;
                    }
                    Ok(())
                }
            }
        }
    }
}

fn finish_review(
    repository: &Repository,
    thread: &str,
    prepared: PreparedReview,
    outcome: ReviewOutcome,
    agent: bool,
) -> Result<()> {
    let ReviewOutcome::Export(comments) = outcome else {
        if agent {
            agent::deliver("")?;
        }
        return Ok(());
    };

    if agent && comments.is_empty() {
        return agent::deliver("");
    }

    let revision = persist_revision(repository, thread, &prepared, comments)?;
    let formatted = format_comments(&revision.comments);

    if agent {
        agent::deliver(&formatted)?;
        eprintln!("saved rev-{}", revision.rev);
        return Ok(());
    }

    let method = copy_to_clipboard(&formatted)?;
    eprintln!("saved rev-{}; copied comments via {method}", revision.rev);
    Ok(())
}

fn open_review_session(
    repository: &Repository,
    thread: &str,
    prepared: &PreparedReview,
) -> Result<ReviewSession> {
    let revisions = list_revisions(repository.git_dir(), thread)?;
    ReviewSession::open(repository, prepared, &revisions)
}
