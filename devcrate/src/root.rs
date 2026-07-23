//! Finding the stack root.
//!
//! The batch scripts each resolve the stack root from their own location
//! (`%~dp0`). A single binary on `PATH` cannot do only that -- it may well live
//! outside the stack it manages -- so it tries, in order:
//!
//! 1. an explicit `--root <path>`,
//! 2. the `DEVCRATE_HOME` environment variable,
//! 3. the executable's own directory and its ancestors,
//! 4. the working directory and its ancestors.
//!
//! Steps 3 and 4 walk upward looking for a directory that *is* a stack root, so
//! `devcrate status` works from anywhere inside the tree, and a `cargo run` from
//! `<root>\devcrate` finds `<root>` on the way up.

use std::env;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Name of the tool's own config file, and the strongest stack-root marker.
pub const CONFIG_FILE: &str = "devcrate.toml";

/// Where the stack root came from. Reported by `devcrate status` so that a
/// wrongly guessed root is visible rather than mysterious.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootSource {
    Flag,
    Env,
    Exe,
    Cwd,
}

impl RootSource {
    pub fn label(self) -> &'static str {
        match self {
            RootSource::Flag => "--root flag",
            RootSource::Env => "DEVCRATE_HOME",
            RootSource::Exe => "executable location",
            RootSource::Cwd => "working directory",
        }
    }
}

#[derive(Debug, Clone)]
pub struct StackRoot {
    pub path: PathBuf,
    pub source: RootSource,
}

/// Does this directory look like a Devcrate stack root?
///
/// `devcrate.toml` settles it. Without one -- which is every stack built by the
/// batch scripts so far -- fall back to the layout those scripts create: a
/// `php` directory alongside `start.bat` or an unpacked `nginx-*` directory.
pub fn looks_like_root(dir: &Path) -> bool {
    if dir.join(CONFIG_FILE).is_file() {
        return true;
    }
    dir.join("php").is_dir() && (dir.join("start.bat").is_file() || nginx_dir(dir).is_some())
}

/// The unpacked nginx directory (`nginx-1.31.1`, or whatever version is there).
pub fn nginx_dir(root: &Path) -> Option<PathBuf> {
    let mut found: Option<PathBuf> = None;
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("nginx") && entry.path().is_dir() {
            // Highest-sorting name wins, so nginx-1.32 beats nginx-1.31.1.
            if found.as_ref().is_none_or(|f| {
                f.file_name().map(|n| n.to_string_lossy().into_owned()) < Some(name.to_string())
            }) {
                found = Some(entry.path());
            }
        }
    }
    found
}

/// Resolve the stack root, or explain why we could not.
pub fn resolve(flag: Option<PathBuf>) -> Result<StackRoot> {
    if let Some(path) = flag {
        return explicit(path, RootSource::Flag);
    }

    if let Some(value) = env::var_os("DEVCRATE_HOME") {
        let value = PathBuf::from(value);
        if !value.as_os_str().is_empty() {
            return explicit(value, RootSource::Env);
        }
    }

    if let Ok(exe) = env::current_exe()
        && let Some(dir) = exe.parent()
        && let Some(path) = search_upward(dir)
    {
        return Ok(StackRoot { path, source: RootSource::Exe });
    }

    if let Ok(cwd) = env::current_dir()
        && let Some(path) = search_upward(&cwd)
    {
        return Ok(StackRoot { path, source: RootSource::Cwd });
    }

    bail!(
        "could not find a Devcrate stack root.\n\
         Looked for {CONFIG_FILE} (or a php\\ directory beside start.bat) in the \n\
         executable's folder and the working directory, and every folder above them.\n\
         Set DEVCRATE_HOME, or pass --root <path>."
    );
}

/// An explicitly named root is taken at its word -- it only has to exist. The
/// caller asked for this directory; second-guessing the layout would just get
/// in the way of pointing the tool at a half-built stack.
fn explicit(path: PathBuf, source: RootSource) -> Result<StackRoot> {
    let path = clean(&path).with_context(|| {
        format!("stack root from {} does not exist: {}", source.label(), path.display())
    })?;
    if !path.is_dir() {
        bail!("stack root from {} is not a directory: {}", source.label(), path.display());
    }
    Ok(StackRoot { path, source })
}

fn search_upward(start: &Path) -> Option<PathBuf> {
    let start = clean(start).unwrap_or_else(|_| start.to_path_buf());
    start.ancestors().find(|dir| looks_like_root(dir)).map(PathBuf::from)
}

/// Canonicalize, then drop Windows' `\\?\` verbatim prefix so printed paths
/// look like the ones in the docs.
pub fn clean(path: &Path) -> std::io::Result<PathBuf> {
    let canonical = path.canonicalize()?;
    Ok(strip_verbatim(&canonical))
}

fn strip_verbatim(path: &Path) -> PathBuf {
    let mut components = path.components();
    if let Some(Component::Prefix(prefix)) = components.next()
        && let std::path::Prefix::VerbatimDisk(letter) = prefix.kind()
    {
        let mut out = PathBuf::from(format!("{}:\\", letter as char));
        out.extend(components.filter(|c| !matches!(c, Component::RootDir)));
        return out;
    }
    path.to_path_buf()
}

/// Render `path` relative to `root` for display, falling back to the full path.
pub fn display_relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string()
}
