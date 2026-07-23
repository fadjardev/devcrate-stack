//! Directory junctions -- how the stack names an *active* version.
//!
//! `php\current` and `nginx\current` are junctions rather than copies or
//! configuration, so switching a version is one directory entry being rewritten
//! and nothing else. `mklink /J` is used instead of `std::os::windows::fs::
//! symlink_dir` for one reason: a junction needs no privileges, and a symlink
//! needs either Administrator or Developer Mode.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};

/// Point `link` at `target`, replacing an existing junction there.
pub fn create(link: &Path, target: &Path) -> Result<()> {
    remove(link)?;

    let output = Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .context("running mklink")?;

    if output.status.success() {
        return Ok(());
    }
    // mklink reports failure on stderr, except when it does not.
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let message = if stderr.is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        stderr
    };
    Err(anyhow!("could not link {} -> {}: {message}", link.display(), target.display()))
}

/// Remove a junction, leaving whatever it pointed at alone.
///
/// `remove_dir` deletes the reparse point rather than following it, so the
/// installation on the other end is untouched. A *real* directory in that spot
/// fails the same call unless it is empty, which is the outcome we want:
/// refusing beats deleting something that was not ours to delete.
pub fn remove(link: &Path) -> Result<()> {
    if std::fs::symlink_metadata(link).is_err() {
        return Ok(());
    }
    std::fs::remove_dir(link).with_context(|| {
        format!(
            "removing the existing {} (if it is a real directory rather than a \
             junction, move it aside by hand)",
            link.display()
        )
    })
}

/// What a junction resolves to, or `None` if it is not there.
///
/// `read_link` handles the reparse point; the `canonicalize` fallback covers
/// the case of a real directory sitting where a junction was expected, so a
/// hand-built stack reads the same as a generated one.
pub fn target(link: &Path) -> Option<PathBuf> {
    std::fs::read_link(link).ok().or_else(|| crate::root::clean(link).ok())
}
