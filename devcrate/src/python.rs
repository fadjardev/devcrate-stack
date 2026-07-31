//! `devcrate python use` -- repointing the `python\current` junction.
//!
//! Python is a *toolchain*, not a service. It has no long-lived listener to
//! start, stop, or watch on a port, so the whole of "using" a version is which
//! one sits on `PATH` -- and that is `python\current`, a junction moved exactly
//! as `php\current` is. So this reads as a close relative of [`crate::php`] with
//! the FastCGI half removed: there is a `current` to repoint and a version to
//! resolve, but no worker to launch and nothing for `devcrate status`'s service
//! table to hold.
//!
//! Versions are discovered from the folder names, `python\python-<X.Y>`, the
//! same way PHP's are: unpacking `python-3.8\` is the whole of installing one,
//! and 3.8, 38, and python-3.8 all name it, matched on the digits alone.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};

use crate::config::{self, Stack};
use crate::{exit, junction, php, term};

/// The installed Python versions, one per `python\python-*` directory, sorted,
/// paired with the version their folder name spells (`python-3.8` -> `3.8`).
pub fn versions(stack: &Stack) -> Vec<(PathBuf, String)> {
    stack
        .python_versions()
        .into_iter()
        .map(|dir| {
            let tag = dir.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
            let version = config::version_from_tag(&tag);
            (dir, version)
        })
        .collect()
}

/// Find a Python version by any spelling: `3.8`, `38`, or `python-3.8`.
///
/// Matching on the digits alone -- the same rule [`php::find`] uses -- is what
/// makes those equivalent and keeps the directory naming an implementation
/// detail rather than something anyone has to type exactly.
pub fn find(stack: &Stack, wanted: &str) -> Result<(PathBuf, String)> {
    let wanted_digits = php::digits(wanted);
    if wanted_digits.is_empty() {
        return Err(anyhow!("{wanted:?} does not name a Python version"));
    }

    versions(stack)
        .into_iter()
        .find(|(_, version)| php::digits(version) == wanted_digits)
        .ok_or_else(|| {
            let installed: Vec<String> = versions(stack).into_iter().map(|(_, v)| v).collect();
            if installed.is_empty() {
                anyhow!("no Python versions found in {}", stack.rel(&stack.python_dir))
            } else {
                anyhow!("Python {wanted} is not installed; found: {}", installed.join(", "))
            }
        })
}

/// What a completed switch has to say for itself.
pub struct Switched {
    /// Display version now on `PATH`, e.g. `3.8`.
    pub version: String,
    /// Root-relative directory the junction now points at.
    pub dir: String,
    /// First line of `python --version`, run through the junction -- proof it
    /// resolves.
    pub banner: Option<String>,
}

/// Repoint `python\current`, and report where it landed. The printing lives in
/// [`switch`]; this is what a dashboard would call.
pub fn use_version(stack: &Stack, wanted: &str) -> Result<Switched> {
    let (dir, version) = find(stack, wanted)?;
    let python_exe = dir.join("python.exe");
    if !python_exe.is_file() {
        return Err(anyhow!("{} not found", stack.rel(&python_exe)));
    }

    let current = stack.python_dir.join("current");
    junction::create(&current, &dir)
        .with_context(|| format!("pointing {} at {}", stack.rel(&current), stack.rel(&dir)))?;

    Ok(Switched {
        version,
        dir: stack.rel(&dir),
        banner: version_banner(&current.join("python.exe")),
    })
}

pub fn switch(stack: &Stack, wanted: &str) -> Result<u8> {
    let switched = use_version(stack, wanted)?;
    println!("CLI Python -> {} ({})", switched.version, switched.dir);
    match switched.banner {
        Some(banner) => println!("  {banner}"),
        None => println!("  (python --version produced no output)"),
    }
    Ok(exit::OK)
}

pub fn list(stack: &Stack) -> Result<u8> {
    let current = stack.current_python();
    let current_name =
        current.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());

    let found = versions(stack);
    if found.is_empty() {
        println!("No Python versions found in {}", stack.rel(&stack.python_dir));
        println!("Install one with   devcrate install python 3.8");
        return Ok(exit::OK);
    }

    let width = found
        .iter()
        .filter_map(|(dir, _)| dir.file_name().map(|n| n.to_string_lossy().chars().count()))
        .max()
        .unwrap_or(0);
    for (dir, version) in &found {
        let tag = dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let is_current = current_name.as_deref() == Some(tag.as_str());
        let active = if is_current { "*" } else { " " };
        let state = if dir.join("python.exe").is_file() { "" } else { "  (python.exe missing)" };
        let row = format!("{active} Python {version:<6}  {tag:<width$}{state}");
        match is_current {
            true => println!("{}", term::paint(&row, term::Color::Green)),
            false => println!("{row}"),
        }
    }

    match current_name {
        Some(name) => println!("\n* = python\\current -> {name} (what the CLI resolves to)"),
        None => println!("\npython\\current is not set; run `devcrate python use 3.8` to create it"),
    }
    Ok(exit::OK)
}

fn version_banner(python_exe: &Path) -> Option<String> {
    let output = Command::new(python_exe).arg("--version").output().ok()?;
    // Older Pythons printed the banner to stderr; 3.8 prints it to stdout.
    // Read both so the proof-of-resolution works whichever it uses.
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    combined.lines().next().map(str::trim).filter(|line| !line.is_empty()).map(str::to_string)
}
