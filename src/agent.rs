use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
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
    Warp,
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

pub(crate) fn launch_plan(inner: bool, direct_tty: bool, in_tmux: bool, in_warp: bool) -> Launch {
    if inner || direct_tty {
        Launch::Here
    } else if in_tmux {
        Launch::Tmux
    } else if in_warp {
        Launch::Warp
    } else {
        Launch::Here
    }
}

pub(crate) fn should_spawn() -> bool {
    !matches!(
        launch_plan(
            inner_handoff().is_some(),
            io::stdin().is_terminal() && io::stdout().is_terminal(),
            env::var_os("TMUX").is_some() && tmux_available(),
            in_warp(),
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
        in_warp(),
    ) {
        Launch::Tmux => spawn_tmux(&cwd, &script)?,
        Launch::Warp => spawn_warp_tab(&script)?,
        Launch::Here => bail!(
            "trv --agent needs a pane in this window (Warp or tmux). Run it from a tty, inside tmux, or inside Warp."
        ),
    }

    eprintln!("trv: review opened; waiting until you quit");
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

fn in_warp() -> bool {
    env::var("TERM_PROGRAM").is_ok_and(|term| term == "WarpTerminal")
        || env::var("WARP_IS_LOCAL_SHELL_SESSION").is_ok()
        || env::var("__CFBundleIdentifier").is_ok_and(|id| id.contains("warp.Warp"))
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

fn spawn_warp_tab(script: &Path) -> Result<()> {
    let home = env::var_os("HOME").context("HOME is unset")?;
    let dir = PathBuf::from(home).join(".warp/tab_configs");
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join("trv-review.toml");
    fs::write(&path, warp_tab_config(script))
        .with_context(|| format!("failed to write {}", path.display()))?;
    let status = Command::new("open")
        .args(warp_open_args())
        .status()
        .context("failed to open a Warp tab for the review")?;
    if status.success() {
        Ok(())
    } else {
        let _ = fs::remove_file(&path);
        bail!("failed to open a Warp tab for the review ({status})")
    }
}

fn warp_open_args() -> [&'static str; 2] {
    // -g keeps Warp in the background so the review is ready when you
    // switch back, instead of yanking focus from whatever you're doing.
    ["-g", "warp://tab_config/trv-review"]
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

fn warp_tab_config(script: &Path) -> String {
    let command = format!(
        "exec sh {}",
        shell_single_quote(&script.display().to_string())
    );
    format!(
        "name = \"trv review\"\ntitle = \"trv\"\n\n[[panes]]\nid = \"review\"\ntype = \"terminal\"\ncommands = [{}]\nis_focused = true\n",
        toml_quote(&command)
    )
}

fn toml_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn runner_script(
    exe: &Path,
    cwd: &Path,
    sink: &Path,
    done: &Path,
    working_tree: bool,
    revset: Option<&str>,
) -> String {
    let mut command = format!(
        "exec {} --agent",
        shell_single_quote(&exe.display().to_string())
    );
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        Launch, launch_plan, runner_script, tmux_split_args, warp_open_args, warp_tab_config,
    };

    #[test]
    fn a_real_tty_runs_in_place() {
        assert_eq!(launch_plan(true, false, true, true), Launch::Here);
        assert_eq!(launch_plan(false, true, false, true), Launch::Here);
    }

    #[test]
    fn agents_without_a_tty_split_this_window() {
        assert_eq!(launch_plan(false, false, true, true), Launch::Tmux);
        assert_eq!(launch_plan(false, false, false, true), Launch::Warp);
    }

    #[test]
    fn warp_opens_the_review_tab_without_stealing_focus() {
        assert_eq!(
            warp_open_args(),
            ["-g", "warp://tab_config/trv-review"],
            "open -g must keep Warp in the background"
        );
        let config = warp_tab_config(Path::new("/tmp/trv-agent/run.sh"));
        assert!(config.contains("type = \"terminal\""));
        assert!(config.contains("exec sh '/tmp/trv-agent/run.sh'"));
        assert!(config.contains("is_focused = true"));
    }

    #[test]
    fn tmux_split_runs_the_handoff_script() {
        assert_eq!(
            tmux_split_args(Path::new("/repo"), Path::new("/tmp/run.sh")),
            ["split-window", "-h", "-c", "/repo", "sh '/tmp/run.sh'"]
        );
    }

    #[test]
    fn runner_script_execs_agent_mode_in_the_new_pane() {
        let script = runner_script(
            Path::new("/opt/trv"),
            Path::new("/repo"),
            Path::new("/tmp/sink"),
            Path::new("/tmp/done"),
            false,
            None,
        );
        assert!(script.contains("exec '/opt/trv' --agent"));
        assert!(script.contains("export TRV_AGENT_SINK='/tmp/sink'"));
    }

    #[test]
    fn bundled_skill_teaches_the_split_pane_review_loop() {
        let skill = include_str!("../skills/trv/SKILL.md");
        assert!(skill.contains("name: trv\n"));
        assert!(skill.contains("trv --agent"));
        assert!(skill.contains("3600000"));
        assert!(
            skill.contains("without stealing focus") || skill.contains("tab"),
            "agents must open the review without yanking the user's current window"
        );
        assert!(!skill.contains("Terminal.app"));
    }
}
