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
/// `php` directory alongside `start.bat` or an nginx prefix.
pub fn looks_like_root(dir: &Path) -> bool {
    if dir.join(CONFIG_FILE).is_file() {
        return true;
    }
    dir.join("php").is_dir() && (dir.join("start.bat").is_file() || nginx_prefix(dir).is_some())
}

/// The nginx *prefix*: the directory `nginx.exe -p` is given, holding `conf\`,
/// `logs\`, `temp\`, and the `projects` junction.
///
/// Two layouts answer to this, and both have to keep working.
///
/// **Current:** one stable `nginx\` prefix with the versions inside it
/// (`nginx\nginx-1.31.1\nginx.exe`, `nginx\current` naming the active one).
/// The prefix never moves, so neither do the vhosts, the certificates, or the
/// logs -- which is the whole point, since those belong to the *stack* and not
/// to whichever nginx build is serving them this week.
///
/// **Original:** the versioned directory *was* the prefix
/// (`nginx-1.31.1\nginx.exe` beside `nginx-1.31.1\conf\`). Still detected, so
/// a stack that has not been migrated keeps running -- see `devcrate nginx
/// migrate`.
///
/// A directory qualifies by holding `conf\nginx.conf`, not by its name: that is
/// the file `-c conf/nginx.conf` resolves to, so it is the thing that actually
/// makes a directory usable as a prefix.
pub fn nginx_prefix(root: &Path) -> Option<PathBuf> {
    let stable = root.join("nginx");
    if is_nginx_prefix(&stable) {
        return Some(stable);
    }

    let mut found: Option<PathBuf> = None;
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy().into_owned();
        if !name.starts_with("nginx") || !is_nginx_prefix(&entry.path()) {
            continue;
        }
        // Highest-sorting name wins, so nginx-1.32 beats nginx-1.31.1.
        if found.as_ref().is_none_or(|f| {
            f.file_name().map(|n| n.to_string_lossy().into_owned()) < Some(name.clone())
        }) {
            found = Some(entry.path());
        }
    }
    found
}

fn is_nginx_prefix(dir: &Path) -> bool {
    dir.join("conf").join("nginx.conf").is_file()
}

/// `nginx.exe` for a prefix, in order of how deliberately it was put there:
///
/// 1. `<prefix>\current\nginx.exe` - the active version, which `nginx use` sets.
/// 2. `<prefix>\nginx.exe` - the original layout, the binary beside `conf\`.
/// 3. `<prefix>\nginx-<version>\nginx.exe` - installed but with nothing marked
///    active; the highest-sorting version wins, as version discovery has always
///    done.
///
/// The result is canonicalized, so a path found through `current` comes back as
/// the real versioned one. That matters twice over: Windows reports a running
/// process by its resolved image path, so matching one against a junction path
/// would never succeed, and `php\current` is deliberately handled the same way
/// -- the junction says *which* version, and everything else uses the real
/// directory.
pub fn nginx_exe(prefix: &Path) -> Option<PathBuf> {
    let direct = [prefix.join("current").join("nginx.exe"), prefix.join("nginx.exe")];
    for candidate in direct {
        if candidate.is_file() {
            return Some(clean(&candidate).unwrap_or(candidate));
        }
    }

    let mut versions: Vec<PathBuf> = nginx_versions(prefix);
    versions.sort();
    let newest = versions.pop()?;
    let exe = newest.join("nginx.exe");
    exe.is_file().then(|| clean(&exe).unwrap_or(exe))
}

/// The versioned nginx directories inside a prefix: `nginx\nginx-1.31.1`, and
/// any others installed beside it. `current` is excluded -- it is the junction
/// naming one of these, not a version of its own, exactly as `php\current` is.
pub fn nginx_versions(prefix: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(prefix)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
            path.is_dir() && name.starts_with("nginx") && name != "current"
        })
        .collect();
    dirs.sort();
    dirs
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A throwaway directory tree. Paths are given as `dir/file` strings; a
    /// trailing `/` means "directory", anything else is an empty file.
    pub(crate) fn tree(label: &str, entries: &[&str]) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("devcrate-{label}-{unique}"));
        let _ = std::fs::remove_dir_all(&base);
        for entry in entries {
            let path = base.join(entry);
            if entry.ends_with('/') {
                std::fs::create_dir_all(&path).unwrap();
            } else {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, "").unwrap();
            }
        }
        base
    }

    /// The restructured layout: one stable prefix, versions inside it.
    #[test]
    fn the_prefix_is_the_stable_nginx_directory_when_there_is_one() {
        let root = tree(
            "prefix-new",
            &["nginx/conf/nginx.conf", "nginx/nginx-1.31.1/nginx.exe"],
        );
        assert_eq!(nginx_prefix(&root), Some(root.join("nginx")));
    }

    /// ...and a stack that has not been migrated still resolves, which is the
    /// whole reason both layouts are detected.
    #[test]
    fn the_original_versioned_directory_is_still_a_prefix() {
        let root = tree(
            "prefix-old",
            &["nginx-1.31.1/conf/nginx.conf", "nginx-1.31.1/nginx.exe"],
        );
        assert_eq!(nginx_prefix(&root), Some(root.join("nginx-1.31.1")));
    }

    /// A directory qualifies by holding the config `-c conf/nginx.conf`
    /// resolves to, not by being named something nginx-ish.
    #[test]
    fn a_directory_without_a_config_is_not_a_prefix() {
        let root = tree("prefix-none", &["nginx/nginx-1.31.1/nginx.exe", "nginx-notes/"]);
        assert_eq!(nginx_prefix(&root), None);
    }

    #[test]
    fn the_highest_version_wins_when_nothing_is_marked_active() {
        let prefix = tree(
            "exe-highest",
            &["nginx-1.29.4/nginx.exe", "nginx-1.31.1/nginx.exe", "conf/nginx.conf"],
        );
        assert_eq!(nginx_exe(&prefix), Some(prefix.join("nginx-1.31.1").join("nginx.exe")));

        let versions = nginx_versions(&prefix);
        assert_eq!(versions.len(), 2, "current and conf are not versions");
    }

    /// The original layout put the binary beside `conf\`, and that is still one
    /// of the places it is looked for.
    #[test]
    fn a_binary_beside_the_config_is_found() {
        let prefix = tree("exe-flat", &["nginx.exe", "conf/nginx.conf"]);
        assert_eq!(nginx_exe(&prefix), Some(prefix.join("nginx.exe")));
    }

    #[test]
    fn no_binary_anywhere_reads_as_not_installed() {
        let prefix = tree("exe-missing", &["conf/nginx.conf", "nginx-1.31.1/"]);
        assert_eq!(nginx_exe(&prefix), None);
    }
}
