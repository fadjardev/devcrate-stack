//! What turns an unpacked nginx archive into an nginx the stack can serve with.
//!
//! Much less than PHP needs, and for a reason the layout already encodes: an
//! nginx *build* carries no configuration this stack uses. The `nginx.conf`,
//! the vhosts, the certificates, and the logs all belong to the prefix, so a
//! build is a folder of binaries and there is nothing per-version to generate.
//! The vendor's own `conf\` comes along inside the versioned folder and is
//! simply never read -- `-p` and `-c` point at the prefix's.
//!
//! So the work is the other way round from [`super::php`]: not configuring the
//! thing installed, but making sure the prefix around it is one nginx can
//! actually start in. That is [`prepare_prefix`], and every directory it
//! creates is one nginx will not create for itself.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// `nginx-1.31.3.zip` -> `1.31.3`.
///
/// The whole version, unlike PHP: nginx has no branch/release split, and
/// `nginx-1.31.3\` is the folder name because a build *is* its version. A
/// major alone is refused for the same reason PHP refuses one -- `nginx-1\`
/// names nothing anybody could switch back to.
pub fn version_from_file_name(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".zip").unwrap_or(&lower);
    let rest = stem.strip_prefix("nginx-")?;

    // Anything after the version is somebody's renaming, not part of it.
    let version = rest.split('-').next()?;
    if !version.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() < 2 || parts.iter().any(|part| part.is_empty()) {
        return None;
    }

    Some(version.to_string())
}

/// Is this unpacked directory an nginx for Windows?
///
/// One file settles it. There is no equivalent of PHP's thread-safety trap
/// here -- nginx ships exactly one Windows build, and the wrong-build class of
/// mistake the PHP installer has to guard against does not exist.
pub fn check(dir: &Path, archive: &Path) -> Result<()> {
    if !dir.join("nginx.exe").is_file() {
        bail!(
            "{} does not contain nginx.exe; it does not look like an nginx \
             distribution for Windows",
            archive.display()
        );
    }
    Ok(())
}

/// What the prefix was missing, and now is not.
#[derive(Debug, Default)]
pub struct Prepared {
    /// Root-relative names of what had to be created.
    pub created: Vec<String>,
    /// `conf\nginx.conf` is not there. Reported rather than generated -- see
    /// [`prepare_prefix`].
    pub config_missing: bool,
}

/// Make the prefix startable: the directories nginx needs and the junction the
/// vhost roots resolve through.
///
/// `logs\` is the load-bearing one. nginx opens `logs/error.log` before it
/// creates any of the paths in its configuration, so a prefix without that
/// directory fails at startup with a `CreateFile()` error and no server. It
/// creates its own temp paths, but one made here costs nothing and keeps the
/// prefix looking like the one the docs describe.
///
/// `conf\nginx.conf` is deliberately *not* written. It is tracked in the
/// repository, it is the stack's own file rather than any build's, and seeding
/// it from the vendor's default would produce a config that includes no
/// `sites\` and serves none of your vhosts -- a working nginx that silently
/// does the wrong thing. A missing one means something is wrong with the
/// checkout, which is worth saying instead of papering over.
pub fn prepare_prefix(prefix: &Path, projects: &Path, rel: impl Fn(&Path) -> String) -> Result<Prepared> {
    let mut prepared = Prepared {
        config_missing: !prefix.join("conf").join("nginx.conf").is_file(),
        ..Prepared::default()
    };

    for name in ["logs", "temp"] {
        let dir = prefix.join(name);
        if dir.is_dir() {
            continue;
        }
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating {}", dir.display()))?;
        prepared.created.push(rel(&dir));
    }

    let link = prefix.join("projects");
    if !link.exists() {
        if !projects.is_dir() {
            std::fs::create_dir_all(projects)
                .with_context(|| format!("creating {}", projects.display()))?;
            prepared.created.push(rel(projects));
        }
        crate::junction::create(&link, projects)
            .with_context(|| format!("linking {} -> {}", rel(&link), rel(projects)))?;
        prepared.created.push(rel(&link));
    }

    Ok(prepared)
}

/// The verdict of `nginx -t` on the stack's own configuration.
#[derive(Debug)]
pub struct ConfigTest {
    pub ok: bool,
    /// nginx's own words, one line.
    pub detail: String,
}

/// Ask the freshly installed build to parse the stack's configuration.
///
/// Advisory, never fatal, and that is the whole point of running it here. A
/// fresh stack has no certificates yet, so `ssl_certificate` names a file that
/// does not exist and the test fails for a reason that has nothing to do with
/// the install. What it does catch is the reason worth catching early: a newer
/// nginx that no longer accepts a directive the existing vhosts use, found now
/// rather than at the next `devcrate start`.
///
/// `None` means the test could not be run at all -- the binary would not
/// execute -- which is different from it having failed.
pub fn test_config(exe: &Path, prefix: &Path) -> Option<ConfigTest> {
    let output = Command::new(exe).arg("-p").arg(prefix).arg("-t").output().ok()?;
    // nginx writes its verdict, including the offending file and line, to
    // stderr whether it passed or failed.
    let text = String::from_utf8_lossy(&output.stderr);
    let detail = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    Some(ConfigTest { ok: output.status.success(), detail })
}

/// The prefix directory an installed build should land in, or an explanation
/// of why this stack has nowhere to put one.
///
/// A stack that has not been migrated is using its *versioned* directory as
/// the prefix (`nginx-1.31.1\nginx.exe` beside `nginx-1.31.1\conf\`).
/// Installing into that would nest one version inside another and leave the
/// stack's certificates and logs under a build that is no longer the only one
/// -- exactly the tangle `nginx migrate` exists to undo. So it is refused,
/// with the one command that fixes it.
pub fn install_prefix(resolved: &Path) -> Result<&Path> {
    let name = resolved.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
    if name.starts_with("nginx-") || name.starts_with("nginx_") {
        bail!(
            "this stack still uses {} as the nginx prefix, which is the layout \
             from before the builds moved inside it.\n\
             Installing a second version there would nest it inside the first. \
             Migrate once, then install:\n  \
             devcrate stop nginx\n  devcrate nginx migrate --dry-run\n  \
             devcrate nginx migrate",
            resolved.display()
        );
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_is_the_whole_version_from_the_vendors_file_name() {
        assert_eq!(version_from_file_name("nginx-1.31.3.zip"), Some("1.31.3".into()));
        assert_eq!(version_from_file_name("nginx-0.8.55.zip"), Some("0.8.55".into()));
        // The patch level is part of the folder name here, unlike PHP: nginx
        // has no branch a build could be filed under.
        assert_eq!(version_from_file_name("nginx-1.30.4.zip"), Some("1.30.4".into()));
    }

    #[test]
    fn a_name_without_a_version_is_not_guessed() {
        assert_eq!(version_from_file_name("nginx.zip"), None);
        assert_eq!(version_from_file_name("nginx-latest.zip"), None);
        assert_eq!(version_from_file_name("nginx-1.zip"), None);
        // ...and a PHP archive is not an nginx one.
        assert_eq!(version_from_file_name("php-8.4.23-Win32-vs17-x64.zip"), None);
    }

    /// Installing into an unmigrated stack would nest a version inside a
    /// version. The refusal is the whole feature.
    #[test]
    fn an_unmigrated_prefix_is_refused_with_the_command_that_fixes_it() {
        let root = Path::new("C:\\devcrate");
        assert_eq!(install_prefix(&root.join("nginx")).unwrap(), root.join("nginx"));

        let err = install_prefix(&root.join("nginx-1.31.1")).unwrap_err().to_string();
        assert!(err.contains("nginx migrate"), "{err}");
    }

    /// A prefix configured to somewhere else entirely is the user's business,
    /// not a legacy layout, so it installs.
    #[test]
    fn a_deliberately_relocated_prefix_still_installs() {
        let elsewhere = Path::new("C:\\devcrate").join("web").join("server");
        assert_eq!(install_prefix(&elsewhere).unwrap(), elsewhere);
    }
}
