//! Handing something to another program: a URL to the browser, a folder to
//! Explorer, a folder to a new terminal.
//!
//! Each is spawned and left to run, with no handle of ours inherited -- the
//! dashboard calls these, and its console is the screen it has just drawn.
//! Nothing here writes to stdout.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Open `url` in the default browser. `rundll32 url.dll` rather than `start`:
/// no console flashes up, and nothing in the URL is read by cmd.
pub fn browser(url: &str) -> Result<()> {
    let mut command = Command::new("rundll32.exe");
    command.args(["url.dll,FileProtocolHandler", url]);
    spawn(command, CREATE_NO_WINDOW).with_context(|| format!("opening {url}"))
}

/// Show `dir` in Explorer.
pub fn folder(dir: &Path) -> Result<()> {
    let mut command = Command::new("explorer.exe");
    command.arg(dir);
    spawn(command, CREATE_NO_WINDOW).with_context(|| format!("opening {} in Explorer", dir.display()))
}

/// Which terminal [`terminal`] opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminal {
    WindowsTerminal,
    Cmd,
}

/// Open a terminal in `dir`: Windows Terminal where it is installed, a cmd
/// window otherwise. It inherits the dashboard's environment, `PATH` included.
pub fn terminal(dir: &Path) -> Result<Terminal> {
    let mut wt = Command::new("wt.exe");
    wt.arg("-d").arg(dir);
    if spawn(wt, CREATE_NO_WINDOW).is_ok() {
        return Ok(Terminal::WindowsTerminal);
    }

    // `start` gives the new cmd a console of its own. Spawning cmd directly
    // would hand it ours, and it would read the dashboard's keystrokes. The
    // launcher is detached rather than windowless: a hidden console is
    // passed on, and the new cmd would run with no window to see it in.
    let mut cmd = Command::new("cmd.exe");
    #[cfg(windows)]
    cmd.raw_arg(format!("/C start \"\" /D \"{}\" cmd.exe", dir.display()));
    spawn(cmd, DETACHED_PROCESS).with_context(|| format!("opening a terminal in {}", dir.display()))?;
    Ok(Terminal::Cmd)
}

fn spawn(mut command: Command, flags: u32) -> std::io::Result<()> {
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(flags);
    #[cfg(not(windows))]
    let _ = flags;
    command.spawn().map(drop)
}

/// The launcher needs no window; what it opens makes its own.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// No console at all, so none is passed on to what `start` launches.
const DETACHED_PROCESS: u32 = 0x0000_0008;
