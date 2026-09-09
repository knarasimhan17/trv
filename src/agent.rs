use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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
    ControllingTty,
    Tmux,
    Warp,
    ITerm,
    Kitty,
    TerminalApp,
    WezTerm,
    Unavailable,
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
    controlling_tty: bool,
    host: Option<Launch>,
    macos: bool,
    ssh: bool,
) -> Launch {
    if inner || direct_tty {
        Launch::Here
    } else if ssh && controlling_tty {
        // Agent captured stdio, but the SSH session still has a tty.
        Launch::ControllingTty
    } else if let Some(host) = host {
        host
    } else if ssh {
        Launch::Unavailable
    } else if macos {
        Launch::TerminalApp
    } else if controlling_tty {
        Launch::ControllingTty
    } else {
        Launch::Unavailable
    }
}

fn planned_launch() -> Launch {
    let facts = HostFacts::from_env();
    launch_plan(
        inner_handoff().is_some(),
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        controlling_tty_is_terminal(),
        detect_host_from(&facts),
        cfg!(target_os = "macos"),
        facts.ssh,
    )
}

pub(crate) fn should_spawn() -> Result<bool> {
    match planned_launch() {
        Launch::Here => Ok(false),
        Launch::Unavailable => bail!("{}", unavailable_message(session_is_ssh())),
        _ => Ok(true),
    }
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

    let mut child = None;
    match planned_launch() {
        Launch::Tmux => spawn_tmux(&cwd, &script)?,
        Launch::Warp => spawn_warp_tab(&script)?,
        Launch::ITerm => spawn_iterm_tab(&script)?,
        Launch::Kitty => spawn_kitty_tab(&cwd, &script)?,
        Launch::TerminalApp => spawn_terminal_app(&script)?,
        Launch::WezTerm => spawn_wezterm_tab(&cwd, &script)?,
        Launch::ControllingTty => child = Some(spawn_controlling_tty(&script)?),
        Launch::Here | Launch::Unavailable => {
            bail!("{}", unavailable_message(session_is_ssh()))
        }
    }

    eprintln!("trv: review opened; waiting until you quit");
    let status = wait_for_done(&done)?;
    if let Some(mut child) = child {
        let _ = child.wait();
    }
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

#[derive(Debug)]
struct HostFacts {
    ssh: bool,
    inside_tmux: bool,
    tmux_bin: bool,
    warp: bool,
    term_program: Option<String>,
    kitty_window: bool,
}

impl HostFacts {
    fn from_env() -> Self {
        Self {
            ssh: session_is_ssh(),
            inside_tmux: env::var_os("TMUX").is_some(),
            tmux_bin: tmux_available(),
            warp: in_warp(),
            term_program: env::var("TERM_PROGRAM").ok(),
            kitty_window: env::var_os("KITTY_WINDOW_ID").is_some(),
        }
    }
}

fn detect_host_from(facts: &HostFacts) -> Option<Launch> {
    if facts.inside_tmux && facts.tmux_bin {
        return Some(Launch::Tmux);
    }
    if facts.ssh {
        // Ignore leaked local GUI env (Warp, iTerm, …) on the remote.
        return facts.tmux_bin.then_some(Launch::Tmux);
    }
    if facts.warp {
        return Some(Launch::Warp);
    }
    match facts.term_program.as_deref() {
        Some("iTerm.app") | Some("iTerm2") => Some(Launch::ITerm),
        Some("Apple_Terminal") => Some(Launch::TerminalApp),
        Some("WezTerm") => Some(Launch::WezTerm),
        Some("kitty") => Some(Launch::Kitty),
        _ if facts.kitty_window => Some(Launch::Kitty),
        _ => None,
    }
}

fn session_is_ssh() -> bool {
    env::var_os("SSH_CONNECTION").is_some()
        || env::var_os("SSH_TTY").is_some()
        || env::var_os("SSH_CLIENT").is_some()
}

fn controlling_tty_is_terminal() -> bool {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map(|file| file.is_terminal())
        .unwrap_or(false)
}

fn unavailable_message(ssh: bool) -> String {
    if ssh {
        "trv --agent cannot open a review over SSH without a tty. Run it inside tmux or a real terminal (`ssh -t`)."
            .to_owned()
    } else {
        "trv --agent needs a visible terminal. Run it from a tty, inside Warp, tmux, iTerm, Kitty, or WezTerm, or on macOS (falls back to Terminal.app)."
            .to_owned()
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
    if env::var_os("TMUX").is_some() {
        let status = Command::new("tmux")
            .args(tmux_new_window_args(cwd, script))
            .status()
            .context("failed to start tmux window")?;
        if status.success() {
            Ok(())
        } else {
            bail!("tmux new-window exited with {status}")
        }
    } else {
        let output = Command::new("tmux")
            .args(tmux_new_session_args(cwd, script))
            .output()
            .context("failed to start tmux session")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "tmux new-session exited with {}: {}",
                output.status,
                stderr.trim()
            );
        }
        let name = String::from_utf8_lossy(&output.stdout);
        let name = name.trim();
        if name.is_empty() {
            eprintln!("trv: review opened in a tmux session; run `tmux attach` to view it");
        } else {
            eprintln!(
                "trv: review opened in tmux session {name}; run `tmux attach -t {name}` to view it"
            );
        }
        Ok(())
    }
}

fn spawn_controlling_tty(script: &Path) -> Result<Child> {
    let tty_in = fs::OpenOptions::new()
        .read(true)
        .open("/dev/tty")
        .context("controlling terminal is unavailable")?;
    let tty_out = fs::OpenOptions::new()
        .write(true)
        .open("/dev/tty")
        .context("controlling terminal is unavailable")?;
    let tty_err = fs::OpenOptions::new()
        .write(true)
        .open("/dev/tty")
        .context("controlling terminal is unavailable")?;
    Command::new("sh")
        .arg(script)
        .stdin(Stdio::from(tty_in))
        .stdout(Stdio::from(tty_out))
        .stderr(Stdio::from(tty_err))
        .spawn()
        .context("failed to start the review on the controlling terminal")
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

fn tmux_new_session_args(cwd: &Path, script: &Path) -> Vec<String> {
    vec![
        "new-session".to_owned(),
        "-d".to_owned(),
        "-P".to_owned(),
        "-F".to_owned(),
        "#{session_name}".to_owned(),
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
        HostFacts, Launch, detect_host_from, iterm_tab_script, kitty_launch_args, launch_plan,
        runner_script, terminal_app_script, tmux_new_session_args, tmux_new_window_args,
        unavailable_message, warp_open_args, warp_tab_config, wezterm_spawn_args,
    };

    fn local_plan(inner: bool, direct_tty: bool, host: Option<Launch>, macos: bool) -> Launch {
        launch_plan(inner, direct_tty, false, host, macos, false)
    }

    fn ssh_facts(
        inside_tmux: bool,
        tmux_bin: bool,
        warp: bool,
        term_program: Option<&str>,
        kitty_window: bool,
    ) -> HostFacts {
        HostFacts {
            ssh: true,
            inside_tmux,
            tmux_bin,
            warp,
            term_program: term_program.map(str::to_owned),
            kitty_window,
        }
    }

    #[test]
    fn a_real_tty_runs_in_place() {
        assert_eq!(
            local_plan(true, false, Some(Launch::Warp), true),
            Launch::Here
        );
        assert_eq!(
            local_plan(false, true, Some(Launch::ITerm), true),
            Launch::Here
        );
        assert_eq!(
            launch_plan(false, true, true, Some(Launch::Warp), true, true),
            Launch::Here,
            "a direct tty still runs in place over SSH"
        );
    }

    #[test]
    fn agents_without_a_tty_open_a_tab_in_this_window() {
        assert_eq!(
            local_plan(false, false, Some(Launch::Tmux), true),
            Launch::Tmux
        );
        assert_eq!(
            local_plan(false, false, Some(Launch::Warp), true),
            Launch::Warp
        );
        assert_eq!(
            local_plan(false, false, Some(Launch::ITerm), true),
            Launch::ITerm
        );
        assert_eq!(
            local_plan(false, false, Some(Launch::Kitty), true),
            Launch::Kitty
        );
        assert_eq!(
            local_plan(false, false, Some(Launch::WezTerm), true),
            Launch::WezTerm
        );
        assert_eq!(
            launch_plan(false, false, true, Some(Launch::Warp), true, false),
            Launch::Warp,
            "locally, a Warp tab still wins over /dev/tty"
        );
    }

    #[test]
    fn macos_falls_back_to_terminal_app() {
        assert_eq!(
            local_plan(false, false, None, true),
            Launch::TerminalApp,
            "unknown hosts on macOS must still get a Terminal.app window"
        );
        assert_eq!(
            launch_plan(false, false, true, None, true, false),
            Launch::TerminalApp,
            "local macOS still prefers Terminal.app over taking over /dev/tty"
        );
        assert_eq!(
            local_plan(false, false, None, false),
            Launch::Unavailable,
            "non-macOS without a known host cannot invent a window"
        );
        assert_eq!(
            launch_plan(false, false, true, None, false, false),
            Launch::ControllingTty,
            "local Linux can still use /dev/tty when no GUI host exists"
        );
    }

    #[test]
    fn ssh_without_a_tty_uses_the_controlling_terminal() {
        assert_eq!(
            launch_plan(false, false, true, None, false, true),
            Launch::ControllingTty
        );
        assert_eq!(
            launch_plan(false, false, true, Some(Launch::Tmux), false, true),
            Launch::ControllingTty,
            "/dev/tty is preferred over tmux when both exist"
        );
        assert_eq!(
            launch_plan(false, false, true, None, true, true),
            Launch::ControllingTty,
            "SSH onto a Mac must not open Terminal.app"
        );
    }

    #[test]
    fn ssh_ignores_leaked_gui_host_env() {
        for facts in [
            ssh_facts(false, false, true, Some("WarpTerminal"), false),
            ssh_facts(false, false, false, Some("iTerm.app"), false),
            ssh_facts(false, false, false, Some("Apple_Terminal"), false),
            ssh_facts(false, false, false, Some("WezTerm"), false),
            ssh_facts(false, false, false, Some("kitty"), true),
        ] {
            assert_eq!(
                detect_host_from(&facts),
                None,
                "SSH must ignore leaked GUI env {facts:?}",
            );
            assert_eq!(
                launch_plan(false, false, true, detect_host_from(&facts), true, true),
                Launch::ControllingTty,
            );
            assert_eq!(
                launch_plan(false, false, false, detect_host_from(&facts), true, true),
                Launch::Unavailable,
                "leaked Warp/iTerm env must not spawn a GUI over SSH"
            );
        }
    }

    #[test]
    fn ssh_uses_tmux_when_the_controlling_tty_is_missing() {
        let inside = ssh_facts(true, true, true, Some("WarpTerminal"), false);
        assert_eq!(detect_host_from(&inside), Some(Launch::Tmux));
        assert_eq!(
            launch_plan(false, false, false, detect_host_from(&inside), true, true),
            Launch::Tmux
        );

        let available = ssh_facts(false, true, false, None, false);
        assert_eq!(
            detect_host_from(&available),
            Some(Launch::Tmux),
            "SSH may start a tmux session even when not already inside tmux"
        );
        assert_eq!(
            launch_plan(
                false,
                false,
                false,
                detect_host_from(&available),
                false,
                true
            ),
            Launch::Tmux
        );
    }

    #[test]
    fn ssh_with_nothing_errors_instead_of_opening_warp() {
        let facts = ssh_facts(false, false, true, Some("WarpTerminal"), false);
        assert_eq!(detect_host_from(&facts), None);
        assert_eq!(
            launch_plan(false, false, false, detect_host_from(&facts), true, true),
            Launch::Unavailable
        );
        let message = unavailable_message(true);
        assert!(message.contains("SSH"), "{message}");
        assert!(message.contains("tmux"), "{message}");
        assert!(
            !message.contains("Warp"),
            "the SSH error must not tell the user to open Warp: {message}"
        );
    }

    #[test]
    fn local_host_detection_still_sees_warp_and_tmux() {
        let warp = HostFacts {
            ssh: false,
            inside_tmux: false,
            tmux_bin: true,
            warp: true,
            term_program: Some("WarpTerminal".into()),
            kitty_window: false,
        };
        assert_eq!(detect_host_from(&warp), Some(Launch::Warp));

        let tmux = HostFacts {
            ssh: false,
            inside_tmux: true,
            tmux_bin: true,
            warp: false,
            term_program: None,
            kitty_window: false,
        };
        assert_eq!(detect_host_from(&tmux), Some(Launch::Tmux));

        let tmux_installed_only = HostFacts {
            ssh: false,
            inside_tmux: false,
            tmux_bin: true,
            warp: false,
            term_program: None,
            kitty_window: false,
        };
        assert_eq!(
            detect_host_from(&tmux_installed_only),
            None,
            "locally, a tmux binary without TMUX must not steal the macOS Terminal.app fallback"
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
        assert_eq!(
            tmux_new_session_args(Path::new("/repo"), Path::new("/tmp/run.sh")),
            [
                "new-session",
                "-d",
                "-P",
                "-F",
                "#{session_name}",
                "-c",
                "/repo",
                "sh '/tmp/run.sh'"
            ]
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
