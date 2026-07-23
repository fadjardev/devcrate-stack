//! What the terminal on the other end can actually do.
//!
//! Two questions, asked once and cached: how wide is it, and will it render
//! ANSI colour. Both have to be answered rather than assumed, because the same
//! binary is run from `cmd.exe`, PowerShell, Windows Terminal, Git Bash, an
//! IDE's integrated terminal, and out of a pipe into a file -- and the last of
//! those must not receive escape sequences at all.

use std::io::{IsTerminal, stdout};
use std::sync::OnceLock;

/// The foreground colours used anywhere in the output. Deliberately few: colour
/// here is a second channel for the state column, not decoration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Green,
    Yellow,
    Red,
    Dim,
}

impl Color {
    fn code(self) -> &'static str {
        match self {
            Color::Green => "\x1b[32m",
            Color::Yellow => "\x1b[33m",
            Color::Red => "\x1b[31m",
            Color::Dim => "\x1b[2m",
        }
    }
}

/// Wrap `text` in `color`, or hand it back untouched when the terminal cannot
/// show it.
pub fn paint(text: &str, color: Color) -> String {
    if color_enabled() {
        format!("{}{text}\x1b[0m", color.code())
    } else {
        text.to_string()
    }
}

/// Whether ANSI sequences are worth emitting.
///
/// The order is the conventional one: an explicit `NO_COLOR` beats everything,
/// an explicit `CLICOLOR_FORCE` beats the auto-detection, and otherwise colour
/// needs both a terminal and one that can render it. Redirected output gets
/// none, which is what makes `devcrate status > report.txt` readable.
pub fn color_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        if std::env::var("CLICOLOR_FORCE").is_ok_and(|v| v != "0") {
            return true;
        }
        if std::env::var("TERM").is_ok_and(|term| term == "dumb") {
            return false;
        }
        stdout().is_terminal() && enable_ansi()
    })
}

/// The terminal's width in columns, or `None` when there is no terminal to
/// measure -- a pipe, a file, or a console that will not say.
///
/// `None` means "do not wrap", not "assume 80": truncating output to a width
/// nobody asked for is worse than a long line, and a file has no width at all.
pub fn width() -> Option<usize> {
    static WIDTH: OnceLock<Option<usize>> = OnceLock::new();
    *WIDTH.get_or_init(|| {
        // Honoured first, and honoured even when stdout is redirected: it is an
        // explicit statement about the display, which is exactly what a CI job
        // or a `COLUMNS=120 devcrate status | less` is making.
        if let Some(columns) = std::env::var("COLUMNS").ok().and_then(|v| v.parse().ok())
            && columns > 0
        {
            return Some(columns);
        }
        if !stdout().is_terminal() {
            return None;
        }
        console_width()
    })
}

/// Break `text` into lines no wider than the terminal, splitting at spaces.
/// Every line returned carries `indent`, first one included, so the caller can
/// print them as they come.
///
/// Without a known width the text stays on one line: wrapping to a guess would
/// mangle output that is on its way into a file or another program.
pub fn wrap(text: &str, indent: usize) -> Vec<String> {
    let pad = " ".repeat(indent);
    let Some(width) = width().filter(|w| *w > indent + 8) else {
        return vec![format!("{pad}{text}")];
    };

    let mut lines: Vec<String> = Vec::new();
    let mut line = pad.clone();
    for word in text.split_whitespace() {
        let filled = line.chars().count() > indent;
        if filled && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::replace(&mut line, pad.clone()));
        } else if filled {
            line.push(' ');
        }
        line.push_str(word);
    }
    if line.chars().count() > indent {
        lines.push(line);
    }
    lines
}

// ---------------------------------------------------------------------------
// Platform detail
// ---------------------------------------------------------------------------

/// Turn on the console's ANSI interpreter, and report whether it took.
///
/// A Windows console renders escape sequences only once
/// `ENABLE_VIRTUAL_TERMINAL_PROCESSING` is set on it, which is why the same
/// program can look right in Windows Terminal and print gibberish in a plain
/// `cmd.exe` window. Where the handle is not a console at all -- Git Bash's
/// mintty hands us a pipe -- the call fails and we fall through to the answer
/// `IsTerminal` already gave, which is correct for a pty.
#[cfg(windows)]
fn enable_ansi() -> bool {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::System::Console::{
        ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, SetConsoleMode,
    };

    let handle = stdout().as_raw_handle();
    let mut mode = 0u32;
    // Safety: the handle is this process's stdout and stays valid for its
    // lifetime; both calls only read and write the mode word passed in.
    unsafe {
        if GetConsoleMode(handle as _, &mut mode) == 0 {
            // Not a console. mintty and friends handle ANSI themselves.
            return true;
        }
        if mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING != 0 {
            return true;
        }
        SetConsoleMode(handle as _, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
    }
}

#[cfg(not(windows))]
fn enable_ansi() -> bool {
    true
}

#[cfg(windows)]
fn console_width() -> Option<usize> {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::System::Console::{
        CONSOLE_SCREEN_BUFFER_INFO, GetConsoleScreenBufferInfo,
    };

    let handle = stdout().as_raw_handle();
    // Safety: zeroed is a valid CONSOLE_SCREEN_BUFFER_INFO (it is all integers
    // and small POD structs), and the call fills it in or fails without
    // touching it.
    let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
    if unsafe { GetConsoleScreenBufferInfo(handle as _, &mut info) } == 0 {
        return None;
    }
    // The visible window, not the scrollback buffer: the buffer is often far
    // wider than what anyone can see.
    let columns = info.srWindow.Right as i32 - info.srWindow.Left as i32 + 1;
    usize::try_from(columns).ok().filter(|&c| c > 0)
}

#[cfg(not(windows))]
fn console_width() -> Option<usize> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wrapping is width-driven, and without a width it must not invent one.
    #[test]
    fn text_is_left_whole_when_the_width_is_unknown() {
        // The test binary's stdout is captured, so `width()` is None here.
        let long = "a ".repeat(200);
        assert_eq!(wrap(long.trim(), 4).len(), 1);
    }

    #[test]
    fn colour_codes_are_balanced() {
        // Whether colour is on depends on how the tests are run; either answer
        // has to produce a string that ends the way it started.
        let painted = paint("up", Color::Green);
        assert_eq!(painted.contains("\x1b["), painted.ends_with("\x1b[0m"));
        assert!(painted.contains("up"));
    }
}
