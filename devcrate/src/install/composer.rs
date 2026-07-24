//! Placing Composer: a single verified phar, a shim to run it, and the
//! `home\` / `cache\` directories it keeps inside the stack root.
//!
//! The odd one out among the installers, and the layout is why. PHP and nginx
//! are archives unpacked into versioned folders; Composer is one file that runs
//! under whatever `php\current` names, so there is nothing to extract and no
//! version to keep side by side (docs/architecture.md). Composer is a tool, not
//! a runtime the stack serves with, which is why `composer\` is in-root and not
//! versioned.
//!
//! So the whole job is: prove the phar is the one the vendor published (the
//! sha256 sidecar does that, back in [`super::download`]), drop it in
//! `composer\`, and write a `composer.bat` that hands it to `php`. The shim
//! calls *bare* `php`, so Composer still resolves through the PHP `current`
//! junction on `PATH` -- the phar's home moved out of the version folder, but
//! which PHP runs it did not.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// `composer-2.10.2.phar` -> `2.10.2`.
///
/// The download names the cached file this way so two versions do not collide
/// in `_downloads\`; the bare `composer.phar` an offline `--from` usually points
/// at carries no version, so [`probe`] reads it from the phar instead. A name
/// that is not `composer-<version>.phar` is `None` rather than a guess.
pub fn version_from_file_name(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".phar")?;
    let rest = stem.strip_prefix("composer-")?;
    if is_dotted(rest) { Some(rest.to_string()) } else { None }
}

/// A dotted numeric version: `2.10.2`, `1.0`, but not `latest` or an empty part.
fn is_dotted(text: &str) -> bool {
    !text.is_empty()
        && text.chars().all(|c| c.is_ascii_digit() || c == '.')
        && text.contains('.')
        && text.split('.').all(|part| !part.is_empty())
}

/// Is this file a PHP phar at all?
///
/// The most that can be checked without running it: a phar built for the CLI
/// begins with the `#!/usr/bin/env php` stub, and any PHP source with `<?php`.
/// That it is *Composer* specifically, and not some other phar, is what the
/// sha256 verification on the download path and [`probe`] on any path actually
/// settle -- this only rules out pointing `--from` at a zip or a text file.
pub fn check(phar: &Path) -> Result<()> {
    let mut file =
        File::open(phar).with_context(|| format!("reading {}", phar.display()))?;
    let mut head = [0u8; 512];
    let n = file.read(&mut head).with_context(|| format!("reading {}", phar.display()))?;
    let text = String::from_utf8_lossy(&head[..n]);

    if !text.contains("<?php") {
        bail!(
            "{} does not look like a PHP phar: no `<?php` in its first bytes.\n\
             A Composer phar begins with `#!/usr/bin/env php`.",
            phar.display()
        );
    }
    Ok(())
}

/// The shim files written beside the phar, root-relative for the receipt and
/// the printout.
#[derive(Debug, Default)]
pub struct Shims {
    pub written: Vec<String>,
}

/// Write the launchers that put `composer` on the command line.
///
/// Two, because the stack supports two shells (CLAUDE.md): `composer.bat` for
/// cmd / PowerShell / Windows Terminal, and an extension-less `composer` for Git
/// Bash. Both do the same one thing -- run the phar beside them with `php`,
/// passing every argument through -- and both call bare `php` so the active
/// version is whatever `php\current` on `PATH` resolves to.
pub fn write_shims(dir: &Path, rel: impl Fn(&Path) -> String) -> Result<Shims> {
    let mut shims = Shims::default();

    // `%~dp0` expands to this directory with a trailing backslash; `%*` is every
    // argument. cmd returns the last command's exit code, so composer's own
    // status propagates.
    let bat = dir.join("composer.bat");
    std::fs::write(
        &bat,
        "@echo off\r\nphp \"%~dp0composer.phar\" %*\r\n",
    )
    .with_context(|| format!("writing {}", bat.display()))?;
    shims.written.push(rel(&bat));

    let sh = dir.join("composer");
    std::fs::write(
        &sh,
        "#!/bin/sh\nexec php \"$(dirname \"$0\")/composer.phar\" \"$@\"\n",
    )
    .with_context(|| format!("writing {}", sh.display()))?;
    shims.written.push(rel(&sh));

    Ok(shims)
}

/// Create the `home\` and `cache\` directories Composer keeps in the stack root,
/// returning the ones that had to be made.
///
/// `COMPOSER_HOME` and `COMPOSER_CACHE_DIR` are pointed at these in
/// docs/installation.md so nothing Composer writes leaks into `%APPDATA%`.
/// Making them at install time means the very first `composer` run has them,
/// rather than Composer creating a home somewhere else because the configured
/// one is not there yet.
pub fn ensure_home(dir: &Path, rel: impl Fn(&Path) -> String) -> Result<Vec<String>> {
    let mut created = Vec::new();
    for name in ["home", "cache"] {
        let path = dir.join(name);
        if path.is_dir() {
            continue;
        }
        std::fs::create_dir_all(&path)
            .with_context(|| format!("creating {}", path.display()))?;
        created.push(rel(&path));
    }
    Ok(created)
}

/// What running the installed phar under a PHP had to say.
///
/// Advisory, exactly like nginx's `nginx -t`: the install is done and the phar
/// is verified before this runs, so a failure here is information -- the PHP on
/// `PATH` is too old, or there is none -- not a reason to undo anything.
#[derive(Debug)]
pub struct Run {
    pub ok: bool,
    /// Composer's own first line: `Composer version 2.10.2 ...`, or the error.
    pub detail: String,
}

/// Ask a PHP to run `composer --version`.
///
/// `None` means the interpreter could not be launched at all, which is
/// different from Composer running and failing -- the same distinction
/// [`super::nginx::test_config`] draws.
pub fn probe(php_exe: &Path, phar: &Path) -> Option<Run> {
    let output =
        Command::new(php_exe).arg(phar).arg("--version").arg("--no-interaction").output().ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stdout
        .lines()
        .chain(stderr.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string();
    Some(Run { ok: output.status.success(), detail })
}

/// Pull the version out of `Composer version 2.10.2 2024-12-11 ...`.
pub fn version_from_output(line: &str) -> Option<String> {
    let after = line.split("version").nth(1)?;
    let token = after.split_whitespace().next()?;
    if is_dotted(token) { Some(token.to_string()) } else { None }
}

/// `php.exe` under the version `php\current` resolves to, if the stack names
/// one. The interpreter [`probe`] runs the phar with.
pub fn current_php_exe(current_php: Option<PathBuf>) -> Option<PathBuf> {
    let exe = current_php?.join("php.exe");
    exe.is_file().then_some(exe)
}

/// `70205` -> `7.2.5`: the `min-php` the version index encodes as one integer.
pub fn format_min_php(encoded: u32) -> String {
    format!("{}.{}.{}", encoded / 10000, (encoded / 100) % 100, encoded % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_comes_from_the_downloads_own_name() {
        assert_eq!(version_from_file_name("composer-2.10.2.phar"), Some("2.10.2".into()));
        assert_eq!(version_from_file_name("composer-2.2.29.phar"), Some("2.2.29".into()));
        // The bare name an offline `--from` points at carries no version, so it
        // is read from the phar instead rather than guessed here.
        assert_eq!(version_from_file_name("composer.phar"), None);
        assert_eq!(version_from_file_name("composer-latest.phar"), None);
        // A single number is not a version anyone could reinstall by name.
        assert_eq!(version_from_file_name("composer-2.phar"), None);
    }

    #[test]
    fn the_version_is_read_out_of_composers_own_greeting() {
        assert_eq!(
            version_from_output("Composer version 2.10.2 2024-12-11 16:12:00"),
            Some("2.10.2".into())
        );
        // The long form the `--version` banner sometimes carries.
        assert_eq!(
            version_from_output("Composer version 2.2.29 (2.2.29) 2025-01-01"),
            Some("2.2.29".into())
        );
        assert_eq!(version_from_output("PHP Fatal error: ..."), None);
    }

    #[test]
    fn min_php_decodes_the_one_integer_the_index_uses() {
        assert_eq!(format_min_php(70205), "7.2.5");
        assert_eq!(format_min_php(80000), "8.0.0");
        assert_eq!(format_min_php(80109), "8.1.9");
    }

    #[test]
    fn a_php_phar_is_told_apart_from_something_that_is_not() {
        let dir = std::env::temp_dir().join("devcrate-test-composer-check");
        std::fs::create_dir_all(&dir).unwrap();

        let phar = dir.join("looks-like.phar");
        std::fs::write(&phar, "#!/usr/bin/env php\n<?php // phar stub\n").unwrap();
        assert!(check(&phar).is_ok());

        let zip = dir.join("actually.zip");
        std::fs::write(&zip, b"PK\x03\x04 not php at all").unwrap();
        assert!(check(&zip).is_err());
    }
}
