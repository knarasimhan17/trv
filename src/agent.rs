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
    ITerm,
    Kitty,
    TerminalApp,
    WezTerm,
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

pub(crate) fn launch_plan(
    inner: bool,
    direct_tty: bool,
    host: Option<Launch>,
    macos: bool,
) -> Launch {
    if inner || direct_tty {
        Launch::Here
    } else if let Some(host) = host {
        host
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
            io::stdin().is_terminal() && io::stdout().is_terminal(),
            detect_host(),
            cfg!(target_os = "macos"),
        ),
        Launch::Here
    )
}

pub(crate) fn spawn_and_forward(
    working_tree: bool,
    branch: bool,
    revset: Option<&str>,
) -> Result<()> {
    let dir = tempfile::tempdir().context("failed to create agent handoff directory")?;
    let sink = dir.path().join("sink");
    let done = dir.path().join("done");
    let script = dir.path().join("run.sh");
    let exe = env::current_exe().context("failed to resolve trv executable")?;
    let cwd = env::current_dir().context("current directory is unavailable")?;

    fs::write(
        &script,
        runner_script(&exe, &cwd, &sink, &done, working_tree, branch, revset),
    )
    .with_context(|| format!("failed to write {}", script.display()))?;

    match launch_plan(false, false, detect_host(), cfg!(target_os = "macos")) {
        Launch::Tmux => spawn_tmux(&cwd, &script)?,
        Launch::Warp => spawn_warp_tab(&script)?,
        Launch::ITerm => spawn_iterm_tab(&script)?,
        Launch::Kitty => spawn_kitty_tab(&cwd, &script)?,
        Launch::TerminalApp => spawn_terminal_app(&script)?,
        Launch::WezTerm => spawn_wezterm_tab(&cwd, &script)?,
        Launch::Here => bail!(
            "trv --agent needs a visible terminal. Run it from a tty, inside Warp, tmux, iTerm, Kitty, or WezTerm, or on macOS (falls back to Terminal.app)."
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

fn detect_host() -> Option<Launch> {
    if env::var_os("TMUX").is_some() && tmux_available() {
        return Some(Launch::Tmux);
    }
    if in_warp() {
        return Some(Launch::Warp);
    }
    match env::var("TERM_PROGRAM").ok().as_deref() {
        Some("iTerm.app") | Some("iTerm2") => Some(Launch::ITerm),
        Some("Apple_Terminal") => Some(Launch::TerminalApp),
        Some("WezTerm") => Some(Launch::WezTerm),
        Some("kitty") => Some(Launch::Kitty),
        _ if env::var_os("KITTY_WINDOW_ID").is_some() => Some(Launch::Kitty),
        _ => None,
    }
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
        .args(tmux_new_window_args(cwd, script))
        .status()
        .context("failed to start tmux window")?;
    if status.success() {
        Ok(())
    } else {
        bail!("tmux new-window exited with {status}")
    }
}

fn spawn_iterm_tab(script: &Path) -> Result<()> {
    run_osascript(&iterm_tab_script(script), "iTerm")
}

fn spawn_terminal_app(script: &Path) -> Result<()> {
    run_osascript(&terminal_app_script(script), "Terminal.app")
}

fn spawn_kitty_tab(cwd: &Path, script: &Path) -> Result<()> {
    let status = Command::new("kitty")
        .args(kitty_launch_args(cwd, script))
        .status()
        .context("failed to launch a Kitty tab")?;
    if status.success() {
        Ok(())
    } else {
        bail!("kitty @ launch exited with {status}")
    }
}

fn spawn_wezterm_tab(cwd: &Path, script: &Path) -> Result<()> {
    let status = Command::new("wezterm")
        .args(wezterm_spawn_args(cwd, script))
        .status()
        .context("failed to spawn a WezTerm tab")?;
    if status.success() {
        Ok(())
    } else {
        bail!("wezterm cli spawn exited with {status}")
    }
}

fn run_osascript(script: &str, host: &str) -> Result<()> {
    let status = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .status()
        .with_context(|| format!("failed to open a {host} tab"))?;
    if status.success() {
        Ok(())
    } else {
        bail!("osascript exited with {status} while opening a {host} tab")
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

fn tmux_new_window_args(cwd: &Path, script: &Path) -> Vec<String> {
    vec![
        "new-window".to_owned(),
        "-d".to_owned(),
        "-c".to_owned(),
        cwd.display().to_string(),
        format!("sh {}", shell_single_quote(&script.display().to_string())),
    ]
}

fn kitty_launch_args(cwd: &Path, script: &Path) -> Vec<String> {
    vec![
        "@".to_owned(),
        "launch".to_owned(),
        "--type=tab".to_owned(),
        "--cwd".to_owned(),
        cwd.display().to_string(),
        "sh".to_owned(),
        script.display().to_string(),
    ]
}

fn wezterm_spawn_args(cwd: &Path, script: &Path) -> Vec<String> {
    vec![
        "cli".to_owned(),
        "spawn".to_owned(),
        "--cwd".to_owned(),
        cwd.display().to_string(),
        "sh".to_owned(),
        script.display().to_string(),
    ]
}

fn iterm_tab_script(script: &Path) -> String {
    let command = format!("sh {}", shell_single_quote(&script.display().to_string()));
    format!(
        "tell application \"iTerm\"\ntell current window\ncreate tab with default profile command {}\nend tell\nend tell",
        applescript_quote(&command)
    )
}

fn terminal_app_script(script: &Path) -> String {
    let command = format!(
        "exec sh {}",
        shell_single_quote(&script.display().to_string())
    );
    format!(
        "tell application \"Terminal\"\ndo script {}\nend tell",
        applescript_quote(&command)
    )
}

fn applescript_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
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
    branch: bool,
    revset: Option<&str>,
) -> String {
    let mut command = format!(
        "exec {} --agent",
        shell_single_quote(&exe.display().to_string())
    );
    if working_tree {
        command.push_str(" -w");
    }
    if branch {
        command.push_str(" -b");
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
        Launch, iterm_tab_script, kitty_launch_args, launch_plan, runner_script,
        terminal_app_script, tmux_new_window_args, warp_open_args, warp_tab_config,
        wezterm_spawn_args,
    };

    #[test]
    fn a_real_tty_runs_in_place() {
        assert_eq!(
            launch_plan(true, false, Some(Launch::Warp), true),
            Launch::Here
        );
        assert_eq!(
            launch_plan(false, true, Some(Launch::ITerm), true),
            Launch::Here
        );
    }

    #[test]
    fn agents_without_a_tty_open_a_tab_in_this_window() {
        assert_eq!(
            launch_plan(false, false, Some(Launch::Tmux), true),
            Launch::Tmux
        );
        assert_eq!(
            launch_plan(false, false, Some(Launch::Warp), true),
            Launch::Warp
        );
        assert_eq!(
            launch_plan(false, false, Some(Launch::ITerm), true),
            Launch::ITerm
        );
        assert_eq!(
            launch_plan(false, false, Some(Launch::Kitty), true),
            Launch::Kitty
        );
        assert_eq!(
            launch_plan(false, false, Some(Launch::WezTerm), true),
            Launch::WezTerm
        );
    }

    #[test]
    fn macos_falls_back_to_terminal_app() {
        assert_eq!(
            launch_plan(false, false, None, true),
            Launch::TerminalApp,
            "unknown hosts on macOS must still get a Terminal.app window"
        );
        assert_eq!(
            launch_plan(false, false, None, false),
            Launch::Here,
            "non-macOS without a known host cannot invent a window"
        );
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
    fn tmux_opens_a_background_window() {
        assert_eq!(
            tmux_new_window_args(Path::new("/repo"), Path::new("/tmp/run.sh")),
            ["new-window", "-d", "-c", "/repo", "sh '/tmp/run.sh'"]
        );
    }

    #[test]
    fn iterm_opens_a_tab_without_activating() {
        let script = iterm_tab_script(Path::new("/tmp/run.sh"));
        assert!(script.contains("create tab with default profile command"));
        assert!(!script.contains("activate"));
    }

    #[test]
    fn kitty_and_wezterm_open_a_tab() {
        assert_eq!(
            kitty_launch_args(Path::new("/repo"), Path::new("/tmp/run.sh")),
            [
                "@",
                "launch",
                "--type=tab",
                "--cwd",
                "/repo",
                "sh",
                "/tmp/run.sh"
            ]
        );
        assert_eq!(
            wezterm_spawn_args(Path::new("/repo"), Path::new("/tmp/run.sh")),
            ["cli", "spawn", "--cwd", "/repo", "sh", "/tmp/run.sh"]
        );
    }

    #[test]
    fn terminal_app_opens_a_new_window() {
        let script = terminal_app_script(Path::new("/tmp/run.sh"));
        assert!(script.contains("tell application \"Terminal\""));
        assert!(script.contains("do script"));
        assert!(
            !script.contains("in front window"),
            "fallback must create a window even if Terminal.app is not already open"
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
            true,
            None,
        );
        assert!(script.contains("exec '/opt/trv' --agent -b"));
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
