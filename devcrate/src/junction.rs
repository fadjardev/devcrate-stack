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
///
/// The new junction is made beside `link` first and only then swapped in, so a
/// `mklink` that fails leaves the old one -- `php\current`, say -- where it
/// was, rather than leaving nothing. The swap itself is two renames of
/// directory entries, with the old junction put back if the second one fails.
pub fn create(link: &Path, target: &Path) -> Result<()> {
    let fresh = beside(link, "devcrate-new");
    remove(&fresh)?;
    mklink(&fresh, target)?;

    // A real directory where the junction goes keeps the old rule: an empty
    // one is removed, anything in it refuses (see `remove`).
    let is_junction = std::fs::read_link(link).is_ok();
    if !is_junction && let Err(err) = remove(link) {
        let _ = remove(&fresh);
        return Err(err);
    }

    let aside = beside(link, "devcrate-old");
    if is_junction {
        remove(&aside)?;
        if let Err(err) = std::fs::rename(link, &aside) {
            let _ = remove(&fresh);
            return Err(err).with_context(|| format!("moving the existing {} aside", link.display()));
        }
    }
    if let Err(err) = std::fs::rename(&fresh, link) {
        if is_junction {
            let _ = std::fs::rename(&aside, link);
        }
        let _ = remove(&fresh);
        return Err(err).with_context(|| format!("putting the new {} in place", link.display()));
    }
    if is_junction {
        let _ = remove(&aside);
    }
    Ok(())
}

/// `<link>.<suffix>`, in the same directory, so a rename is a rename.
fn beside(link: &Path, suffix: &str) -> PathBuf {
    let name = link.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    link.with_file_name(format!("{name}.{suffix}"))
}

/// `mklink /J link target`, through cmd since mklink is one of its built-ins.
///
/// The command line is written out by hand: Rust quotes an argument only when
/// it has a space, so a path like `C:\R&D\php` reached cmd bare and the `&`
/// split it into two commands. Quoted, and with `/s` so cmd strips just the
/// outer pair, `&`, `^`, `(` and `)` stay part of the path. `/d` skips any
/// AutoRun command. A path cannot hold `"`, so no quoting can be broken out of;
/// `%NAME%` naming a set variable is still expanded, as cmd always does.
fn mklink(link: &Path, target: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;

    let line = format!("/d /s /c \"mklink /J \"{}\" \"{}\"\"", link.display(), target.display());
    let output = Command::new("cmd").raw_arg(&line).output().context("running mklink")?;

    if output.status.success() && std::fs::read_link(link).is_ok() {
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::root::tests::tree;

    /// Everything that asks "is *this* the active version" -- the `*` in
    /// `nginx list` and `php list`, whether an install may take `current`,
    /// whether `migrate` still has a junction to create -- compares this
    /// against a path built from the prefix. So the two have to be the same
    /// string, which is a property of `read_link` rather than of anything here:
    /// the reparse point stores a verbatim path (`\??\C:\...`) and `read_link`
    /// hands back an ordinary one. Worth a test precisely because nothing in
    /// the code makes it true.
    #[test]
    fn a_junctions_target_compares_equal_to_the_path_it_was_made_from() {
        let base = tree("junction-target", &["nginx-1.31.1/nginx.exe"]);
        let version = crate::root::clean(&base.join("nginx-1.31.1")).unwrap();
        let link = base.join("current");

        create(&link, &version).unwrap();
        assert_eq!(target(&link).as_deref(), Some(version.as_path()));

        // Repointing replaces rather than nests, and removing the link leaves
        // what it pointed at alone.
        remove(&link).unwrap();
        assert_eq!(target(&link), None);
        assert!(version.join("nginx.exe").is_file());
    }

    /// Paths with characters cmd treats as syntax are linked as they are: an
    /// `&` used to split the mklink line in two.
    #[test]
    fn a_path_with_cmd_syntax_in_it_is_linked_as_is() {
        let base = tree("junction-amp", &["R&D^/php-8.5/php.exe"]);
        let version = crate::root::clean(&base.join("R&D^").join("php-8.5")).unwrap();
        let link = base.join("R&D^").join("current");

        create(&link, &version).unwrap();
        assert_eq!(target(&link).as_deref(), Some(version.as_path()));
        assert!(link.join("php.exe").is_file());
    }

    /// Repointing an existing junction swaps it in one step, and a mklink that
    /// fails leaves the old one in place instead of nothing.
    #[test]
    fn a_failed_repoint_keeps_the_old_junction() {
        let base = tree("junction-keep", &["php-8.2/php.exe", "php-8.5/php.exe"]);
        let old = crate::root::clean(&base.join("php-8.2")).unwrap();
        let new = crate::root::clean(&base.join("php-8.5")).unwrap();
        let link = base.join("current");

        create(&link, &old).unwrap();
        create(&link, &new).unwrap();
        assert_eq!(target(&link).as_deref(), Some(new.as_path()), "repointed");
        assert!(!beside(&link, "devcrate-new").exists() && !beside(&link, "devcrate-old").exists());

        // A junction cannot point at a network share, so mklink refuses this
        // at once ("Local volumes are required"), without looking it up.
        let refused = create(&link, Path::new(r"\\nohost-devcrate\share")).unwrap_err();
        assert!(format!("{refused:#}").contains("could not link"), "{refused:#}");
        assert_eq!(target(&link).as_deref(), Some(new.as_path()), "the old junction survives");
        assert!(!beside(&link, "devcrate-new").exists(), "no half-made link left");
    }
}
