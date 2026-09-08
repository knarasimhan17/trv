use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};

pub(crate) const SINK_ENV: &str = "TRV_AGENT_SINK";
pub(crate) const DONE_ENV: &str = "TRV_AGENT_DONE";

pub(crate) struct Handoff {
    sink: PathBuf,
    done: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Launch {
    Here,
    Tmux,
    TerminalApp,
}

pub(crate) struct HandoffGuard {
    done: PathBuf,
}

impl HandoffGuard {
    pub(crate) fn from_env() -> Option<Self> {
        inner_handoff().map(|handoff| Self { done: handoff.done })
    }
}

impl Drop for HandoffGuard {
    fn drop(&mut self) {
        if self.done.exists() {
            return;
        }
        let _ = fs::write(
            &self.done,
            "error: review UI exited without sending comments\n",
        );
    }
}

pub(crate) fn inner_handoff() -> Option<Handoff> {
    let sink = env::var_os(SINK_ENV).map(PathBuf::from)?;
    let done = env::var_os(DONE_ENV).map(PathBuf::from)?;
    Some(Handoff { sink, done })
}

pub(crate) fn controlling_terminal_available() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .is_ok()
}

pub(crate) fn launch_plan(inner: bool, has_tty: bool, in_tmux: bool, macos: bool) -> Launch {
    if inner || has_tty {
        Launch::Here
    } else if in_tmux {
        Launch::Tmux
    } else if macos {
        Launch::TerminalApp
    } else {
        Launch::Here
    }
}

pub(crate) fn should_spawn() -> bool {
    !matches!(
        launch_plan(
            inner_handoff().is_some(),
            controlling_terminal_available(),
            env::var_os("TMUX").is_some() && tmux_available(),
            cfg!(target_os = "macos"),
        ),
        Launch::Here
    )
}

pub(crate) fn spawn_and_forward(working_tree: bool, revset: Option<&str>) -> Result<()> {
    let dir = tempfile::tempdir().context("failed to create agent handoff directory")?;
    let sink = dir.path().join("sink");
    let done = dir.path().join("done");
    let script = dir.path().join("run.sh");
    let exe = env::current_exe().context("failed to resolve trv executable")?;
    let cwd = env::current_dir().context("current directory is unavailable")?;

    fs::write(
        &script,
        runner_script(&exe, &cwd, &sink, &done, working_tree, revset),
    )
    .with_context(|| format!("failed to write {}", script.display()))?;

    match launch_plan(
        false,
        false,
        env::var_os("TMUX").is_some() && tmux_available(),
        cfg!(target_os = "macos"),
    ) {
        Launch::Tmux => spawn_tmux(&cwd, &script)?,
        Launch::TerminalApp => spawn_terminal_app(&script)?,
        Launch::Here => bail!(
            "trv --agent needs a visible terminal; run it from a tty, inside tmux, or on macOS"
        ),
    }

    eprintln!("trv: waiting for you to finish the review");
    let status = wait_for_done(&done)?;
    let status = status.trim();
    if status != "ok" {
        bail!("{status}");
    }
    let formatted = fs::read_to_string(&sink).context("failed to read review comments")?;
    deliver(&formatted)
}

pub(crate) fn deliver(formatted: &str) -> Result<()> {
    if let Some(handoff) = inner_handoff() {
        return complete_handoff(&handoff, formatted);
    }
    if formatted.is_empty() {
        return Ok(());
    }
    let mut output = io::stdout().lock();
    writeln!(output, "{formatted}").context("failed to write comments to stdout")?;
    Ok(())
}

fn complete_handoff(handoff: &Handoff, formatted: &str) -> Result<()> {
    write_file_atomic(&handoff.sink, formatted)
        .with_context(|| format!("failed to write {}", handoff.sink.display()))?;
    write_file_atomic(&handoff.done, "ok\n")
        .with_context(|| format!("failed to write {}", handoff.done.display()))?;
    Ok(())
}

fn spawn_tmux(cwd: &Path, script: &Path) -> Result<()> {
    let status = Command::new("tmux")
        .args(tmux_split_args(cwd, script))
        .status()
        .context("failed to start tmux split")?;
    if status.success() {
        Ok(())
    } else {
        bail!("tmux split-window exited with {status}")
    }
}

fn spawn_terminal_app(script: &Path) -> Result<()> {
    let status = Command::new("osascript")
        .arg("-e")
        .arg(terminal_app_script(script))
        .status()
        .context("failed to open Terminal.app")?;
    if status.success() {
        Ok(())
    } else {
        bail!("osascript exited with {status}")
    }
}

fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn tmux_split_args(cwd: &Path, script: &Path) -> Vec<String> {
    vec![
        "split-window".to_owned(),
        "-h".to_owned(),
        "-c".to_owned(),
        cwd.display().to_string(),
        format!("sh {}", shell_single_quote(&script.display().to_string())),
    ]
}

fn terminal_app_script(script: &Path) -> String {
    let command = format!(
        "sh {}; exit",
        shell_single_quote(&script.display().to_string())
    );
    format!(
        "tell application \"Terminal\"\nactivate\ndo script {}\nend tell",
        applescript_quote(&command)
    )
}

fn runner_script(
    exe: &Path,
    cwd: &Path,
    sink: &Path,
    done: &Path,
    working_tree: bool,
    revset: Option<&str>,
) -> String {
    let mut command = format!("{} --agent", shell_single_quote(&exe.display().to_string()));
    if working_tree {
        command.push_str(" -w");
    }
    if let Some(revset) = revset {
        command.push_str(" -r ");
        command.push_str(&shell_single_quote(revset));
    }
    format!(
        "#!/bin/sh\nexport {SINK_ENV}={sink}\nexport {DONE_ENV}={done}\ntrap 'if [ ! -f {done} ]; then printf \"%s\\n\" \"error: review UI exited without sending comments\" > {done}; fi' EXIT\ncd {cwd}\n{command}\n",
        sink = shell_single_quote(&sink.display().to_string()),
        done = shell_single_quote(&done.display().to_string()),
        cwd = shell_single_quote(&cwd.display().to_string()),
    )
}

fn wait_for_done(done: &Path) -> Result<String> {
    loop {
        match fs::read_to_string(done) {
            Ok(text) => return Ok(text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                return Err(error).with_context(|| format!("failed to read {}", done.display()));
            }
        }
    }
}

fn write_file_atomic(path: &Path, contents: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents).with_context(|| format!("failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

fn shell_single_quote(value: &str) -> String {
    let mut quoted = String::from("'");
    for (index, part) in value.split('\'').enumerate() {
        if index > 0 {
            quoted.push_str("'\\''");
        }
        quoted.push_str(part);
    }
    quoted.push('\'');
    quoted
}

fn applescript_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{Handoff, Launch, complete_handoff, launch_plan};

    #[test]
    fn a_tty_keeps_the_review_in_this_session() {
        assert_eq!(launch_plan(false, true, false, true), Launch::Here);
        assert_eq!(launch_plan(true, false, true, true), Launch::Here);
    }

    #[test]
    fn no_tty_opens_a_visible_review() {
        assert_eq!(launch_plan(false, false, true, true), Launch::Tmux);
        assert_eq!(launch_plan(false, false, false, true), Launch::TerminalApp);
    }

    #[test]
    fn bundled_skill_teaches_the_in_session_review_loop() {
        let skill = include_str!("../skills/trv/SKILL.md");
        assert!(
            skill.contains("name: trv\n"),
            "the skill name must match skills/trv/"
        );
        assert!(
            skill.contains("trv --agent"),
            "agents must run trv --agent after making code changes"
        );
        assert!(
            skill.contains("3600000"),
            "agents must wait long enough for a human review"
        );
        assert!(
            skill.contains("this terminal"),
            "the skill must prefer the agent session over a popup"
        );
    }

    #[test]
    fn empty_handoff_still_completes_so_the_agent_unblocks() {
        let directory = tempfile::tempdir().expect("handoff directory");
        let handoff = Handoff {
            sink: directory.path().join("sink"),
            done: directory.path().join("done"),
        };
        complete_handoff(&handoff, "").expect("empty handoff");
        assert_eq!(fs::read_to_string(&handoff.sink).expect("sink"), "");
        assert_eq!(fs::read_to_string(&handoff.done).expect("done"), "ok\n");
    }

    #[test]
    fn runner_prefers_here_when_a_tty_exists_even_on_macos() {
        assert_ne!(
            launch_plan(false, true, false, true),
            Launch::TerminalApp,
            "a popup must not be the default when this session has a terminal"
        );
    }
}
