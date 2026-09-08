use std::env;
use std::fs::{File, OpenOptions};
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::{self, Command};

use anyhow::{Context, Result, bail};

pub(crate) struct HostPause {
    pid: u32,
}

impl HostPause {
    pub(crate) fn pause_session_host() -> Result<Option<Self>> {
        let Some(pid) = host_tui_pid() else {
            return Ok(None);
        };
        #[cfg(unix)]
        {
            let result = unsafe { libc::kill(pid as i32, libc::SIGSTOP) };
            if result != 0 {
                return Err(io::Error::last_os_error())
                    .context("failed to pause the agent UI while the review runs");
            }
        }
        Ok(Some(Self { pid }))
    }
}

impl Drop for HostPause {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(self.pid as i32, libc::SIGCONT);
        }
    }
}

pub(crate) fn session_tty() -> Result<File> {
    ignore_background_tty_signals();
    if term_is_unusable() {
        // SAFETY: trv is single-threaded during terminal attach.
        unsafe { env::set_var("TERM", "xterm-256color") };
    }
    let path = session_tty_path()?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("failed to open session terminal {}", path.display()))?;
    attach_stdin(&file)?;
    Ok(file)
}

pub(crate) fn winsize(file: &File) -> io::Result<(u16, u16)> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let mut size = libc::winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let result = unsafe { libc::ioctl(file.as_raw_fd(), libc::TIOCGWINSZ, &mut size) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        if size.ws_col == 0 || size.ws_row == 0 {
            return Err(io::Error::other("session terminal has no size"));
        }
        Ok((size.ws_col, size.ws_row))
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Err(io::Error::other("session terminal size requires Unix"))
    }
}

pub(crate) fn session_tty_path() -> Result<PathBuf> {
    if OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .is_ok()
    {
        return Ok(PathBuf::from("/dev/tty"));
    }
    ancestor_tty_path().context(
        "no session terminal: run trv from a tty, or from an agent that has a parent terminal",
    )
}

pub(crate) fn tty_device(name: &str) -> Option<PathBuf> {
    let name = name.trim();
    if name.is_empty() || name == "?" || name == "??" || name == "-" {
        return None;
    }
    if name.starts_with('/') {
        Some(PathBuf::from(name))
    } else {
        Some(Path::new("/dev").join(name))
    }
}

fn ancestor_tty_path() -> Result<PathBuf> {
    let mut pid = process::id();
    for _ in 0..64 {
        let info = process_info(pid)?;
        if let Some(path) = info.tty {
            return Ok(path);
        }
        if info.ppid <= 1 {
            break;
        }
        pid = info.ppid;
    }
    bail!("no ancestor process has a terminal")
}

fn host_tui_pid() -> Option<u32> {
    let self_pid = process::id();
    let mut pid = self_pid;
    for _ in 0..64 {
        let info = process_info(pid).ok()?;
        if pid != self_pid && is_host_tui(&info.comm) {
            return Some(pid);
        }
        if info.ppid <= 1 {
            break;
        }
        pid = info.ppid;
    }
    None
}

pub(crate) fn is_host_tui(comm: &str) -> bool {
    let name = Path::new(comm)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(comm);
    matches!(
        name,
        "grok" | "claude" | "codex" | "cursor" | "cursor-agent" | "gemini" | "amp" | "opencode"
    )
}

struct ProcessInfo {
    ppid: u32,
    tty: Option<PathBuf>,
    comm: String,
}

fn process_info(pid: u32) -> Result<ProcessInfo> {
    let output = Command::new("ps")
        .args([
            "-p",
            &pid.to_string(),
            "-o",
            "pid=",
            "-o",
            "ppid=",
            "-o",
            "tty=",
            "-o",
            "comm=",
        ])
        .output()
        .context("failed to inspect process terminal")?;
    if !output.status.success() {
        bail!("ps exited with {} for pid {pid}", output.status);
    }
    parse_ps_tty(&String::from_utf8_lossy(&output.stdout))
        .with_context(|| format!("failed to parse tty for pid {pid}"))
}

fn parse_ps_tty(line: &str) -> Result<ProcessInfo> {
    let mut parts = line.split_whitespace();
    let _pid = parts.next().context("ps pid is missing")?;
    let ppid = parts
        .next()
        .context("ps ppid is missing")?
        .parse::<u32>()
        .context("ps ppid is not a number")?;
    let tty = parts.next().and_then(tty_device);
    let comm = parts.collect::<Vec<_>>().join(" ");
    Ok(ProcessInfo { ppid, tty, comm })
}

fn attach_stdin(tty: &File) -> Result<()> {
    if io::stdin().is_terminal() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: the agent piped stdin; keyboard input has to come from the
        // session terminal. stdout stays the pipe so comments still return.
        let result = unsafe { libc::dup2(tty.as_raw_fd(), libc::STDIN_FILENO) };
        if result < 0 {
            return Err(io::Error::last_os_error())
                .context("failed to attach stdin to the session terminal");
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = tty;
        bail!("trv --agent requires a Unix session terminal")
    }
}

fn ignore_background_tty_signals() {
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGTTIN, libc::SIG_IGN);
        libc::signal(libc::SIGTTOU, libc::SIG_IGN);
    }
}

fn term_is_unusable() -> bool {
    match env::var("TERM") {
        Ok(term) => term.is_empty() || term == "dumb",
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::{is_host_tui, parse_ps_tty, tty_device};

    #[test]
    fn host_tui_matches_agent_clis_not_shells() {
        assert!(is_host_tui("grok"));
        assert!(is_host_tui("/opt/homebrew/bin/claude"));
        assert!(is_host_tui("codex"));
        assert!(!is_host_tui("zsh"));
        assert!(!is_host_tui("herdr"));
        assert!(!is_host_tui("stable"));
    }

    #[test]
    fn tty_device_skips_detached_processes() {
        assert_eq!(tty_device("??"), None);
        assert_eq!(tty_device("?"), None);
        assert_eq!(tty_device("-"), None);
    }

    #[test]
    fn tty_device_resolves_macos_and_linux_names() {
        assert_eq!(
            tty_device("ttys004").as_deref(),
            Some(std::path::Path::new("/dev/ttys004"))
        );
        assert_eq!(
            tty_device("pts/0").as_deref(),
            Some(std::path::Path::new("/dev/pts/0"))
        );
        assert_eq!(
            tty_device("/dev/ttys004").as_deref(),
            Some(std::path::Path::new("/dev/ttys004"))
        );
    }

    #[test]
    fn parse_ps_tty_reads_parent_and_device() {
        let info = parse_ps_tty("  9150  8996 ttys004").expect("ps line");
        assert_eq!(info.ppid, 8996);
        assert_eq!(
            info.tty.as_deref(),
            Some(std::path::Path::new("/dev/ttys004"))
        );
    }

    #[test]
    fn parse_ps_tty_walks_past_a_detached_child() {
        let info = parse_ps_tty("12577 9150 ??").expect("detached child");
        assert_eq!(info.ppid, 9150);
        assert_eq!(info.tty, None);
    }
}
