//! `devcrate nginx` -- the versions inside the prefix, and moving an older
//! stack into that shape.
//!
//! PHP and nginx are versioned for opposite reasons, and the layout follows
//! from that. PHP versions are meant to *coexist*: three FastCGI workers run at
//! once and each vhost names the port it wants, so every version keeps its own
//! `php.ini` and nothing is shared. Only one nginx runs, and everything that
//! makes it useful -- the vhosts, the certificates, the logs -- belongs to the
//! stack rather than to the build serving them. So nginx gets one stable
//! prefix holding all of that, with the binaries versioned inside it and
//! `current` naming the active one.
//!
//! The payoff is that no vhost conf mentions a version, and none has to be
//! rewritten to switch: `root` and the logs resolve against the prefix,
//! `ssl_certificate` against the conf directory, and both move as a unit
//! because neither is inside the versioned directory.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

use crate::config::Stack;
use crate::probe::ProcessTable;
use crate::{exit, junction, root, term};

/// One nginx build inside the prefix.
#[derive(Debug, Clone)]
pub struct Version {
    /// `1.31.1`, read from the directory name.
    pub version: String,
    /// The directory itself, e.g. `<prefix>\nginx-1.31.1`.
    pub dir: PathBuf,
    /// Whether `nginx.exe` is actually in it.
    pub installed: bool,
}

/// The versions in the prefix, oldest-sorting first.
pub fn versions(stack: &Stack) -> Vec<Version> {
    root::nginx_versions(&stack.nginx_prefix)
        .into_iter()
        .map(|dir| Version {
            version: version_of(&dir),
            installed: dir.join("nginx.exe").is_file(),
            dir,
        })
        .collect()
}

/// `nginx-1.31.1` -> `1.31.1`. A directory that carries no version keeps its
/// own name, which is the honest answer for a hand-made folder.
fn version_of(dir: &Path) -> String {
    let name = dir.file_name().unwrap_or_default().to_string_lossy();
    let trimmed = name.trim_start_matches("nginx").trim_start_matches(['-', '_']);
    match trimmed.is_empty() {
        true => name.into_owned(),
        false => trimmed.to_string(),
    }
}

/// Find a version by any reasonable spelling: `1.31.1`, `nginx-1.31.1`, or an
/// unambiguous prefix such as `1.31`.
///
/// Prefix matching rather than PHP's digits-only rule, because nginx versions
/// have three components: `digits("1.31")` and `digits("1.3.1")` are the same
/// string, and quietly starting the wrong build is worse than an error.
pub fn find<'a>(available: &'a [Version], wanted: &str) -> Result<&'a Version> {
    let wanted = wanted.trim().trim_start_matches("nginx").trim_start_matches(['-', '_']);
    if wanted.is_empty() {
        bail!("no nginx version named");
    }

    if let Some(exact) = available.iter().find(|v| v.version == wanted) {
        return Ok(exact);
    }

    let matches: Vec<&Version> = available
        .iter()
        .filter(|v| v.version.starts_with(&format!("{wanted}.")))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => Err(anyhow!(
            "nginx {wanted} is not installed{}",
            match available.is_empty() {
                true => String::new(),
                false => format!(
                    "; found: {}",
                    available.iter().map(|v| v.version.as_str()).collect::<Vec<_>>().join(", ")
                ),
            }
        )),
        many => Err(anyhow!(
            "{wanted:?} is ambiguous: {}",
            many.iter().map(|v| v.version.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// What a completed switch has to say for itself.
pub struct Switched {
    pub version: String,
    /// Root-relative directory `current` now points at.
    pub dir: String,
    /// First line of `nginx -v` run through the junction -- proof it resolves.
    pub banner: Option<String>,
    /// nginx was running, so the switch does not take effect until it restarts.
    pub restart_needed: bool,
}

/// Repoint `nginx\current`. The printing lives in [`switch`]; this is what the
/// dashboard would call.
pub fn use_version(stack: &Stack, wanted: &str) -> Result<Switched> {
    let available = versions(stack);
    let version = find(&available, wanted)?;

    if !version.installed {
        bail!("{} has no nginx.exe in it", stack.rel(&version.dir));
    }

    let current = stack.nginx_prefix.join("current");
    junction::create(&current, &version.dir).with_context(|| {
        format!("pointing {} at {}", stack.rel(&current), stack.rel(&version.dir))
    })?;

    Ok(Switched {
        version: version.version.clone(),
        dir: stack.rel(&version.dir),
        banner: version_banner(&current.join("nginx.exe")),
        restart_needed: !running(&stack.nginx_prefix).is_empty(),
    })
}

/// `nginx -v` writes its banner to stderr, unlike `php -v`.
fn version_banner(nginx_exe: &Path) -> Option<String> {
    let output = Command::new(nginx_exe).arg("-v").output().ok()?;
    let text = String::from_utf8_lossy(&output.stderr);
    text.lines().next().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string)
}

fn running(dir: &Path) -> Vec<u32> {
    ProcessTable::scan().matching(dir, &[])
}

// ---------------------------------------------------------------------------
// Migration
// ---------------------------------------------------------------------------

/// One filesystem change the migration wants to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Move {
    /// Move a directory, keeping its contents.
    Dir { from: PathBuf, to: PathBuf },
    /// Drop a junction (never what it points at).
    Unlink(PathBuf),
    /// Create a junction.
    Link { link: PathBuf, target: PathBuf },
}

/// What migrating this stack would do, in order. Empty means it is already in
/// the current layout.
///
/// Kept separate from carrying it out so the plan can be shown first and, more
/// usefully, tested without a stack to wreck.
pub fn plan(stack: &Stack) -> Vec<Move> {
    let prefix = stack.root.join("nginx");
    let mut moves = Vec::new();

    for legacy in legacy_dirs(&stack.root) {
        let name = legacy.file_name().unwrap_or_default().to_os_string();

        // Drop the old projects junction rather than dragging it along: it is
        // recreated at the new prefix, and moving a reparse point is a good way
        // to end up with one pointing somewhere surprising.
        if legacy.join("projects").exists() {
            moves.push(Move::Unlink(legacy.join("projects")));
        }

        // The stack's own configuration moves up to the prefix. Wholesale when
        // there is nothing there yet, and otherwise only the parts git does not
        // carry -- which in practice means the certificates, since the keys are
        // deliberately never committed.
        if !prefix.join("conf").exists() {
            push_move(&mut moves, legacy.join("conf"), prefix.join("conf"));
        } else {
            push_move(
                &mut moves,
                legacy.join("conf").join("certs"),
                prefix.join("conf").join("certs"),
            );
        }
        for shared in ["logs", "temp"] {
            push_move(&mut moves, legacy.join(shared), prefix.join(shared));
        }

        // ...and the build itself moves inside the prefix.
        moves.push(Move::Dir { from: legacy.clone(), to: prefix.join(&name) });
    }

    // Whatever ends up in there, name an active version and restore the link
    // the vhost roots resolve through.
    let after: Vec<PathBuf> = match moves.is_empty() {
        true => root::nginx_versions(&prefix),
        false => planned_versions(&moves),
    };
    if let Some(newest) = after.last()
        && junction::target(&prefix.join("current")).as_deref() != Some(newest.as_path())
    {
        moves.push(Move::Link { link: prefix.join("current"), target: newest.clone() });
    }
    if !prefix.join("projects").exists() {
        moves.push(Move::Link {
            link: prefix.join("projects"),
            target: stack.root.join("projects"),
        });
    }

    moves
}

/// Where the versioned directories will be once the moves have run.
fn planned_versions(moves: &[Move]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = moves
        .iter()
        .filter_map(|m| match m {
            Move::Dir { to, .. } => Some(to.clone()),
            _ => None,
        })
        .filter(|to| {
            let name =
                to.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
            name.starts_with("nginx")
        })
        .collect();
    dirs.sort();
    dirs
}

fn push_move(moves: &mut Vec<Move>, from: PathBuf, to: PathBuf) {
    // Nothing to move, or something already in the way: either way the
    // migration leaves it alone rather than merging blind.
    if from.exists() && !to.exists() {
        moves.push(Move::Dir { from, to });
    }
}

/// The pre-restructure nginx directories still sitting in the stack root.
///
/// Looser than [`root::nginx_prefix`] on purpose: after the repository moves
/// `conf\` to its new home, the old directory no longer has a `conf\nginx.conf`
/// to be recognised by, but its binary, logs, and certificates are all still
/// in it and all still need moving.
fn legacy_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name =
                path.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
            // `nginx` itself is the new prefix, not something to migrate.
            path.is_dir()
                && name.starts_with("nginx")
                && name != "nginx"
                && (path.join("nginx.exe").is_file() || path.join("conf").is_dir())
        })
        .collect();
    dirs.sort();
    dirs
}

pub struct Migrated {
    pub moves: Vec<Move>,
    pub prefix: String,
}

/// Carry out [`plan`], refusing while nginx is running.
pub fn migrate(stack: &Stack) -> Result<Migrated> {
    let prefix = stack.root.join("nginx");

    // Every directory the migration would touch, since the running nginx may
    // be the legacy one rather than anything under the new prefix.
    let mut watched = legacy_dirs(&stack.root);
    watched.push(prefix.clone());
    if let Some(pid) = watched.iter().flat_map(|dir| running(dir)).next() {
        bail!(
            "nginx is running (pid {pid}). Stop it first: `devcrate stop nginx`.\n\
             Moving the directory it is executing from would leave it running \
             with no configuration to reload."
        );
    }

    let moves = plan(stack);
    std::fs::create_dir_all(&prefix)
        .with_context(|| format!("creating {}", stack.rel(&prefix)))?;

    for step in &moves {
        apply(step).with_context(|| format!("migrating: {}", describe(step, stack)))?;
    }

    Ok(Migrated { moves, prefix: stack.rel(&prefix) })
}

fn apply(step: &Move) -> Result<()> {
    match step {
        Move::Dir { from, to } => {
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(from, to)?;
            Ok(())
        }
        Move::Unlink(link) => junction::remove(link),
        Move::Link { link, target } => junction::create(link, target),
    }
}

fn describe(step: &Move, stack: &Stack) -> String {
    match step {
        Move::Dir { from, to } => format!("{} -> {}", stack.rel(from), stack.rel(to)),
        Move::Unlink(link) => format!("drop the {} junction", stack.rel(link)),
        Move::Link { link, target } => {
            format!("link {} -> {}", stack.rel(link), stack.rel(target))
        }
    }
}

// ---------------------------------------------------------------------------
// The subcommands
// ---------------------------------------------------------------------------

pub fn list(stack: &Stack) -> Result<u8> {
    let available = versions(stack);
    let current = stack.current_nginx();

    if available.is_empty() {
        // The binary may still be beside conf\, which is the original layout
        // and perfectly serviceable -- just not versioned.
        if stack.nginx_bin.is_file() {
            println!("  nginx.exe sits directly in {}", stack.rel(&stack.nginx_prefix));
            println!();
            println!("That is the original layout: one build, no versions to switch between.");
            println!("`devcrate nginx migrate` moves it inside the prefix as a named version.");
            return Ok(exit::OK);
        }
        println!("No nginx found in {}", stack.rel(&stack.nginx_prefix));
        println!("Unpack one into {}\\nginx-<version>\\", stack.rel(&stack.nginx_prefix));
        return Ok(exit::OK);
    }

    let width = available.iter().map(|v| v.version.chars().count()).max().unwrap_or(0);
    for version in &available {
        let active = current.as_deref() == Some(version.dir.as_path());
        let marker = if active { "*" } else { " " };
        let state = if version.installed { "" } else { "  (nginx.exe missing)" };
        let row =
            format!("{marker} {:<width$}  {}{state}", version.version, stack.rel(&version.dir));
        match active {
            true => println!("{}", term::paint(&row, term::Color::Green)),
            false => println!("{row}"),
        }
    }

    println!();
    match current {
        Some(_) => println!("* = nginx\\current -> the version that runs"),
        None => println!(
            "No nginx\\current junction; the highest version is used. \
             `devcrate nginx use <version>` sets one."
        ),
    }
    Ok(exit::OK)
}

pub fn switch(stack: &Stack, wanted: &str) -> Result<u8> {
    let switched = use_version(stack, wanted)?;
    println!("nginx -> {} ({})", switched.version, switched.dir);
    match switched.banner {
        Some(banner) => println!("  {banner}"),
        None => println!("  (nginx -v produced no output)"),
    }
    if switched.restart_needed {
        println!();
        println!("nginx is running the previous build until it is restarted:");
        println!("  devcrate restart nginx");
    }
    Ok(exit::OK)
}

pub fn migrate_command(stack: &Stack, dry_run: bool) -> Result<u8> {
    let moves = plan(stack);
    if moves.is_empty() {
        println!("Already in the current layout: {}", stack.rel(&stack.nginx_prefix));
        return Ok(exit::OK);
    }

    println!("Migrating the nginx layout in {}", stack.root.display());
    println!();
    for step in &moves {
        println!("  {}", describe(step, stack));
    }
    println!();

    if dry_run {
        println!("Nothing was changed (--dry-run). Run it again without the flag to apply.");
        return Ok(exit::OK);
    }

    let done = migrate(stack)?;
    println!(
        "{} change(s) applied. The prefix is {}, and it stays put from here.",
        done.moves.len(),
        done.prefix
    );
    println!("  check it   devcrate nginx list");
    println!("  start it   devcrate start nginx");
    Ok(exit::OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::root::StackRoot;
    use crate::root::tests::tree;

    fn stack_at(root: PathBuf) -> Stack {
        Stack::open(StackRoot { path: root, source: crate::root::RootSource::Flag }).unwrap()
    }

    fn dir_moves(moves: &[Move]) -> Vec<(String, String)> {
        moves
            .iter()
            .filter_map(|m| match m {
                Move::Dir { from, to } => Some((
                    from.file_name().unwrap().to_string_lossy().into_owned(),
                    to.file_name().unwrap().to_string_lossy().into_owned(),
                )),
                _ => None,
            })
            .collect()
    }

    /// The state a stack is in immediately after pulling the restructure: the
    /// tracked configuration has arrived at its new home, and everything git
    /// does not carry -- the binary, the certificates, the logs -- is still in
    /// the old versioned directory.
    #[test]
    fn migrating_lifts_the_stack_state_out_of_the_versioned_directory() {
        let root = tree(
            "migrate-after-pull",
            &[
                "php/",
                "start.bat",
                "projects/",
                "nginx/conf/nginx.conf",
                "nginx-1.31.1/nginx.exe",
                "nginx-1.31.1/conf/certs/_wildcard.test-key.pem",
                "nginx-1.31.1/logs/access.log",
                "nginx-1.31.1/temp/",
            ],
        );
        let moves = plan(&stack_at(root.clone()));

        // The certificates, logs, and temp move up to the prefix; the build
        // itself moves inside it. The conf/ that git already delivered is left
        // alone -- only certs/ comes across.
        assert_eq!(
            dir_moves(&moves),
            [
                ("certs".to_string(), "certs".to_string()),
                ("logs".to_string(), "logs".to_string()),
                ("temp".to_string(), "temp".to_string()),
                ("nginx-1.31.1".to_string(), "nginx-1.31.1".to_string()),
            ]
        );

        // The build must move last: lifting certs\ and logs\ out of it first
        // is what makes them land in the prefix rather than travel with it.
        let last = moves.iter().rposition(|m| matches!(m, Move::Dir { .. })).unwrap();
        assert!(matches!(&moves[last], Move::Dir { to, .. } if to == &root.join("nginx").join("nginx-1.31.1")));

        // ...and the two junctions the layout depends on are (re)created.
        assert!(moves.contains(&Move::Link {
            link: root.join("nginx").join("current"),
            target: root.join("nginx").join("nginx-1.31.1"),
        }));
        assert!(moves.contains(&Move::Link {
            link: root.join("nginx").join("projects"),
            target: root.join("projects"),
        }));
    }

    /// Migrating a stack that has *not* pulled yet: there is no conf\ at the
    /// prefix, so the whole directory moves rather than only the certificates.
    #[test]
    fn an_unpulled_stack_brings_its_whole_conf_directory() {
        let root = tree(
            "migrate-before-pull",
            &[
                "php/",
                "start.bat",
                "nginx-1.31.1/nginx.exe",
                "nginx-1.31.1/conf/nginx.conf",
                "nginx-1.31.1/conf/sites/myapp.test.conf",
            ],
        );
        let moves = plan(&stack_at(root.clone()));
        assert!(moves.contains(&Move::Dir {
            from: root.join("nginx-1.31.1").join("conf"),
            to: root.join("nginx").join("conf"),
        }));
    }

    /// Nothing left to move once it is in the current layout, which is what
    /// makes running it twice harmless.
    #[test]
    fn a_migrated_stack_has_nothing_left_to_move() {
        let root = tree(
            "migrate-done",
            &["php/", "start.bat", "nginx/conf/nginx.conf", "nginx/nginx-1.31.1/nginx.exe"],
        );
        assert!(dir_moves(&plan(&stack_at(root))).is_empty());
    }

    fn versions_named(names: &[&str]) -> Vec<Version> {
        names
            .iter()
            .map(|v| Version {
                version: v.to_string(),
                dir: PathBuf::from(format!("nginx-{v}")),
                installed: true,
            })
            .collect()
    }

    #[test]
    fn a_version_is_found_by_any_reasonable_spelling() {
        let available = versions_named(&["1.29.4", "1.31.1"]);
        for spelling in ["1.31.1", "nginx-1.31.1", "1.31"] {
            assert_eq!(find(&available, spelling).unwrap().version, "1.31.1", "{spelling}");
        }
    }

    /// PHP matches on digits alone, which cannot work here: `1.31` and `1.3.1`
    /// both reduce to `131`. Matching on the *dotted* prefix keeps them apart --
    /// `1.3` means the 1.3 series and can never select 1.31.something.
    #[test]
    fn a_partial_version_selects_its_own_series_only() {
        let available = versions_named(&["1.3.1", "1.31.1"]);
        assert_eq!(find(&available, "1.3").unwrap().version, "1.3.1");
        assert_eq!(find(&available, "1.31").unwrap().version, "1.31.1");
    }

    /// Two releases of the same series, though, genuinely are ambiguous, and
    /// starting the wrong nginx is worse than saying the spelling was not
    /// specific enough.
    #[test]
    fn an_ambiguous_prefix_is_refused_rather_than_guessed() {
        let available = versions_named(&["1.3.1", "1.3.2"]);
        let err = find(&available, "1.3").unwrap_err().to_string();
        assert!(err.contains("ambiguous"), "{err}");

        assert!(find(&available, "1.99").unwrap_err().to_string().contains("not installed"));
        assert!(find(&[], "1.31.1").is_err());
    }

    #[test]
    fn the_version_is_read_from_the_directory_name() {
        assert_eq!(version_of(Path::new("nginx-1.31.1")), "1.31.1");
        assert_eq!(version_of(Path::new("nginx_1.31.1")), "1.31.1");
        // Nothing version-shaped left: the folder speaks for itself.
        assert_eq!(version_of(Path::new("nginx")), "nginx");
    }
}
