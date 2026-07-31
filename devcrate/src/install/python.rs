//! Turning an unpacked embeddable Python into one the stack can develop against.
//!
//! The embeddable distribution is deliberately minimal: it is a `python.exe`, a
//! zipped standard library, and a `python<XY>._pth` file that pins `sys.path`
//! and -- crucially -- leaves `import site` commented out. With it commented,
//! `site` never runs, which means no `site-packages` and no `pip`. So the one
//! local step here is [`enable_site`], uncommenting that line so the version can
//! grow a `pip` at all.
//!
//! Growing it is [`run_get_pip`], and that needs the network (python.org's
//! bootstrap script), so it lives outside the offline part of the install and
//! degrades to a message when there is nothing to fetch with -- see
//! [`super::python_from_archive`].

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Where python.org hosts the pip bootstrap, keyed by branch.
///
/// The version in the path is load-bearing: the generic `get-pip.py` targets the
/// newest Python and installs a pip that has dropped support for old ones, so a
/// 3.8 install must fetch `pip/3.8/get-pip.py`. pypa keeps a per-branch script
/// for exactly this, EOL branches included.
const GET_PIP_BASE: &str = "https://bootstrap.pypa.io/pip/";

/// The pip bootstrap URL for a given branch, e.g. `3.8` -> the 3.8-specific one.
pub fn get_pip_url(branch: &str) -> String {
    format!("{GET_PIP_BASE}{branch}/get-pip.py")
}

/// `python-3.8.10-embed-amd64.zip` -> (`3.8`, `3.8.10`).
///
/// The branch (`3.8`) is the folder name and what the stack switches by, so it
/// is derived the same way PHP's is -- two parts, digits only. The release
/// (`3.8.10`) is kept for the receipt and the printout.
pub fn version_from_file_name(name: &str) -> Option<(String, String)> {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".zip").unwrap_or(&lower);
    let rest = stem.strip_prefix("python-")?;

    let release = rest.split('-').next()?;
    let parts: Vec<&str> = release.split('.').collect();
    if parts.len() < 2
        || parts.iter().any(|part| part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let branch = format!("{}.{}", parts[0], parts[1]);
    Some((branch, release.to_string()))
}

/// Is this unpacked directory a Python for Windows?
pub fn check(dir: &Path, archive: &Path) -> Result<()> {
    if !dir.join("python.exe").is_file() {
        bail!(
            "{} does not contain python.exe; it does not look like a Python \
             distribution for Windows.\n\
             The zip to install is the embeddable one \
             (python-3.8.10-embed-amd64.zip).",
            archive.display()
        );
    }
    Ok(())
}

/// Uncomment `import site` in the distribution's `._pth`, so `site` runs and a
/// `pip` can be installed and found.
///
/// Returns whether a `._pth` was found to edit. A full (non-embeddable)
/// distribution has none and needs no such surgery -- `site` runs by default --
/// so its absence is `Ok(false)`, not an error.
pub fn enable_site(dir: &Path) -> Result<bool> {
    let Some(pth) = find_pth(dir) else {
        return Ok(false);
    };

    let text = std::fs::read_to_string(&pth)
        .with_context(|| format!("reading {}", pth.display()))?;
    if text.lines().any(|line| line.trim() == "import site") {
        return Ok(true);
    }

    let mut changed = false;
    let updated: Vec<String> = text
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed == "#import site" || trimmed == "# import site" {
                changed = true;
                "import site".to_string()
            } else {
                line.to_string()
            }
        })
        .collect();

    // The commented line is always in the vendor's file; if it was not, the
    // format has changed under us and enabling site by guesswork is worse than
    // saying so.
    if !changed {
        bail!(
            "{} has no `#import site` line to enable; the embeddable \
             distribution's format may have changed",
            pth.display()
        );
    }

    std::fs::write(&pth, updated.join("\r\n") + "\r\n")
        .with_context(|| format!("writing {}", pth.display()))?;
    Ok(true)
}

fn find_pth(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("_pth")))
}

/// What trying to install pip came to.
#[derive(Debug)]
pub enum Pip {
    /// get-pip.py ran and pip is now installed.
    Installed,
    /// It could not be done now -- no network to fetch the bootstrap, or the run
    /// failed. Carries why. The Python itself is installed regardless; this is
    /// the one piece that needs the network, and it can be finished later.
    Skipped(String),
}

/// Run a fetched `get-pip.py` under the installed Python.
///
/// `current_dir` is the Python directory so pip's console scripts land in
/// `Scripts\` beside it, and `--no-warn-script-location` quiets the note that
/// that directory is not yet on `PATH` -- which is expected, and handled the
/// same way `php\current` is.
pub fn run_get_pip(dir: &Path, get_pip: &Path) -> Pip {
    let python = dir.join("python.exe");
    let output = Command::new(&python)
        .arg(get_pip)
        .arg("--no-warn-script-location")
        .current_dir(dir)
        .output();

    match output {
        Ok(out) if out.status.success() => Pip::Installed,
        Ok(out) => Pip::Skipped(first_line(&out.stderr, &out.stdout)),
        Err(err) => Pip::Skipped(format!("running get-pip.py: {err}")),
    }
}

/// The first non-empty line of stderr, then stdout -- whichever says something.
fn first_line(stderr: &[u8], stdout: &[u8]) -> String {
    [stderr, stdout]
        .iter()
        .flat_map(|stream| {
            String::from_utf8_lossy(stream)
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .next()
        .unwrap_or_else(|| "get-pip.py failed with no output".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_branch_and_release_come_from_the_vendors_file_name() {
        assert_eq!(
            version_from_file_name("python-3.8.10-embed-amd64.zip"),
            Some(("3.8".into(), "3.8.10".into()))
        );
        assert_eq!(
            version_from_file_name("python-3.12.4-embed-amd64.zip"),
            Some(("3.12".into(), "3.12.4".into()))
        );
    }

    #[test]
    fn a_name_without_a_version_is_not_guessed() {
        assert_eq!(version_from_file_name("python-embed-amd64.zip"), None);
        assert_eq!(version_from_file_name("python-3-embed-amd64.zip"), None);
        // ...and a PHP archive is not a Python one.
        assert_eq!(version_from_file_name("php-8.4.23-Win32-vs17-x64.zip"), None);
    }

    /// The commented line the embeddable distribution ships is uncommented; a
    /// file that already has it is left as it is.
    #[test]
    fn enabling_site_uncomments_the_import_line() {
        let dir = std::env::temp_dir().join(format!(
            "devcrate-pth-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let pth = dir.join("python38._pth");
        std::fs::write(&pth, "python38.zip\r\n.\r\n\r\n# Uncomment to run site.main()\r\n#import site\r\n").unwrap();

        assert!(enable_site(&dir).unwrap());
        let after = std::fs::read_to_string(&pth).unwrap();
        assert!(after.lines().any(|l| l == "import site"), "{after:?}");
        assert!(!after.lines().any(|l| l.trim() == "#import site"));

        // Idempotent: a second pass finds it already enabled and changes nothing.
        assert!(enable_site(&dir).unwrap());

        // A directory with no ._pth is a full distribution -- site already runs.
        std::fs::remove_file(&pth).unwrap();
        assert!(!enable_site(&dir).unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
