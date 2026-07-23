//! `devcrate php use` -- repointing the `php\current` junction.
//!
//! This is `phpuse.bat`. The junction is what sits on `PATH`, so moving it is
//! what makes `php`, `composer`, and `laravel` resolve to another build; the
//! FastCGI workers are unaffected, since each vhost names its own port.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, anyhow};

use crate::config::{Service, Stack};
use crate::exit;

/// Find a PHP version by any of the spellings someone might reasonably type:
/// `8.5`, `85`, or `php-8.5`.
///
/// Matching on the digits alone is what makes those equivalent, and it is why
/// renaming the directories from `php85` to `php-8.5` needed no change here --
/// or in anybody's existing scripts.
pub fn find<'a>(stack: &'a Stack, wanted: &str) -> Result<&'a Service> {
    let wanted_digits = digits(wanted);
    if wanted_digits.is_empty() {
        return Err(anyhow!("{wanted:?} does not name a PHP version"));
    }

    stack
        .php_services()
        .find(|service| digits(&service.id) == wanted_digits)
        .ok_or_else(|| {
            let installed: Vec<&str> = stack.php_services().map(|s| s.id.as_str()).collect();
            if installed.is_empty() {
                anyhow!("no PHP versions found in {}", stack.rel(&stack.php_dir))
            } else {
                anyhow!("PHP {wanted} is not installed; found: {}", installed.join(", "))
            }
        })
}

/// The version digits of any spelling: `8.5`, `85`, `php85`, `php-8.5` -> `85`.
///
/// This is what makes the directory naming an implementation detail rather than
/// something anyone has to type exactly.
pub fn digits(text: &str) -> String {
    text.chars().filter(char::is_ascii_digit).collect()
}

/// What a completed switch has to say for itself.
pub struct Switched {
    /// Display name of the version now on `PATH`, e.g. `PHP 8.5`.
    pub name: String,
    /// Root-relative directory the junction now points at.
    pub dir: String,
    /// First line of `php -v`, run through the junction -- proof it resolves.
    pub banner: Option<String>,
}

/// Repoint `php\current`, and report where it landed. The printing lives in
/// [`switch`]; this is what the dashboard calls.
pub fn use_version(stack: &Stack, wanted: &str) -> Result<Switched> {
    let service = find(stack, wanted)?;
    let dir = service
        .install_marker
        .parent()
        .ok_or_else(|| anyhow!("{} has no directory", service.install_marker.display()))?;

    // php.exe, not php-cgi.exe: this junction is the *CLI* version. A version
    // can serve FastCGI perfectly well while being useless on the command line.
    let php_exe = dir.join("php.exe");
    if !php_exe.is_file() {
        return Err(anyhow!("{} not found", stack.rel(&php_exe)));
    }

    let current = stack.php_dir.join("current");
    remove_link(&current, stack)?;

    // mklink /J needs no privileges; std's symlink_dir does.
    let output = Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&current)
        .arg(dir)
        .output()
        .context("running mklink")?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        let message = if message.trim().is_empty() {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        } else {
            message.trim().to_string()
        };
        return Err(anyhow!("could not create {}: {message}", stack.rel(&current)));
    }

    Ok(Switched {
        name: service.name.clone(),
        dir: stack.rel(dir),
        banner: version_banner(&current.join("php.exe")),
    })
}

pub fn switch(stack: &Stack, wanted: &str) -> Result<u8> {
    let switched = use_version(stack, wanted)?;
    println!("CLI PHP -> {} ({})", switched.name, switched.dir);
    match switched.banner {
        Some(banner) => println!("  {banner}"),
        None => println!("  (php -v produced no output)"),
    }
    Ok(exit::OK)
}

/// Remove the existing `current` junction, if there is one.
///
/// `remove_dir` deletes the reparse point rather than following it, so the PHP
/// installation it pointed at is untouched. A *real* directory there fails the
/// same call unless it is empty, which is the outcome we want: refusing beats
/// deleting something that is not ours to delete.
fn remove_link(current: &Path, stack: &Stack) -> Result<()> {
    if std::fs::symlink_metadata(current).is_err() {
        return Ok(());
    }
    std::fs::remove_dir(current).with_context(|| {
        format!(
            "removing the existing {} (if it is a real directory rather than a junction, \
             move it aside by hand)",
            stack.rel(current)
        )
    })
}

fn version_banner(php_exe: &Path) -> Option<String> {
    let output = Command::new(php_exe).arg("-v").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().next().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_spellings_are_interchangeable() {
        for spelling in ["8.5", "85", "php85", "PHP8.5"] {
            assert_eq!(digits(spelling), "85", "{spelling} should resolve like php85");
        }
        assert_eq!(digits("7.4"), digits("php74"));
        assert!(digits("current").is_empty());
    }
}
