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

pub(crate) fn launch_plan(inner: bool, in_tmux: bool, macos: bool) -> Launch {
    if inner {
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
    if !formatted.is_empty() {
        let mut output = io::stdout().lock();
        writeln!(output, "{formatted}").context("failed to write comments to stdout")?;
    }
    Ok(())
}

pub(crate) fn deliver(formatted: &str) -> Result<()> {
    if let Some(handoff) = inner_handoff() {
        return complete_handoff(&handoff, formatted);
    }
    if !formatted.is_empty() {
        let mut output = io::stdout().lock();
        writeln!(output, "{formatted}").context("failed to write comments to stdout")?;
    }
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
    use std::path::Path;
    use std::thread;
    use std::time::Duration;

    use super::{
        DONE_ENV, Handoff, HandoffGuard, Launch, SINK_ENV, complete_handoff, launch_plan,
        runner_script, shell_single_quote, terminal_app_script, tmux_split_args, wait_for_done,
    };

    #[test]
    fn inner_process_stays_in_the_visible_terminal() {
        assert_eq!(launch_plan(true, true, true), Launch::Here);
    }

    #[test]
    fn tmux_sessions_split_a_pane() {
        assert_eq!(launch_plan(false, true, true), Launch::Tmux);
    }

    #[test]
    fn macos_opens_terminal_when_not_in_tmux() {
        assert_eq!(launch_plan(false, false, true), Launch::TerminalApp);
    }

    #[test]
    fn other_environments_fall_back_to_the_current_tty() {
        assert_eq!(launch_plan(false, false, false), Launch::Here);
    }

    #[test]
    fn shell_quotes_paths_with_spaces_and_quotes() {
        assert_eq!(shell_single_quote("a b"), "'a b'");
        assert_eq!(shell_single_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn runner_script_passes_agent_flags_and_handoff_paths() {
        let script = runner_script(
            Path::new("/opt/trv"),
            Path::new("/repo with space"),
            Path::new("/tmp/sink"),
            Path::new("/tmp/done"),
            true,
            Some("HEAD~1"),
        );
        assert!(script.contains("export TRV_AGENT_SINK='/tmp/sink'"));
        assert!(script.contains("export TRV_AGENT_DONE='/tmp/done'"));
        assert!(script.contains("cd '/repo with space'"));
        assert!(script.contains("'/opt/trv' --agent -w -r 'HEAD~1'"));
        assert_eq!(SINK_ENV, "TRV_AGENT_SINK");
        assert_eq!(DONE_ENV, "TRV_AGENT_DONE");
    }

    #[test]
    fn tmux_split_runs_the_handoff_script() {
        let args = tmux_split_args(Path::new("/repo"), Path::new("/tmp/run.sh"));
        assert_eq!(
            args,
            ["split-window", "-h", "-c", "/repo", "sh '/tmp/run.sh'",]
        );
    }

    #[test]
    fn terminal_app_script_closes_the_window_after_review() {
        let script = terminal_app_script(Path::new("/tmp/run.sh"));
        assert!(script.contains("tell application \"Terminal\""));
        assert!(script.contains("do script \"sh '/tmp/run.sh'; exit\""));
    }

    #[test]
    fn handoff_writes_comments_then_marks_the_review_done() {
        let directory = tempfile::tempdir().expect("handoff directory");
        let handoff = Handoff {
            sink: directory.path().join("sink"),
            done: directory.path().join("done"),
        };
        complete_handoff(&handoff, "src/main.rs:1: fix this").expect("handoff write");
        assert_eq!(
            fs::read_to_string(&handoff.sink).expect("sink"),
            "src/main.rs:1: fix this"
        );
        assert_eq!(fs::read_to_string(&handoff.done).expect("done"), "ok\n");
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
    fn wait_for_done_returns_once_the_file_exists() {
        let directory = tempfile::tempdir().expect("done directory");
        let done = directory.path().join("done");
        let writer = done.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(80));
            fs::write(writer, "ok\n").expect("write done");
        });
        assert_eq!(wait_for_done(&done).expect("wait"), "ok\n");
    }

    #[test]
    fn handoff_guard_records_an_error_when_the_ui_never_submits() {
        let directory = tempfile::tempdir().expect("guard directory");
        let done = directory.path().join("done");
        drop(HandoffGuard { done: done.clone() });
        assert_eq!(
            fs::read_to_string(&done).expect("guard done file"),
            "error: review UI exited without sending comments\n"
        );
    }

    #[test]
    fn handoff_guard_leaves_a_successful_done_file_alone() {
        let directory = tempfile::tempdir().expect("guard directory");
        let done = directory.path().join("done");
        fs::write(&done, "ok\n").expect("preexisting done");
        drop(HandoffGuard { done: done.clone() });
        assert_eq!(fs::read_to_string(&done).expect("done"), "ok\n");
    }
}
