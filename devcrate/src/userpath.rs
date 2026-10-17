//! `devcrate path` -- the stack's folders on the user's `PATH`.
//!
//! The runtimes are reached through stable folders -- `php\current`,
//! `python\current`, `mariadb\bin` -- so a folder put on `PATH` once keeps
//! answering through every `php use` that follows. This puts them there: in
//! the user's own `PATH` (`HKCU\Environment`), which needs no Administrator
//! and touches no other account.
//!
//! The value is read and written in the registry directly rather than through
//! .NET's `SetEnvironmentVariable`, which rewrites a `REG_EXPAND_SZ` as
//! `REG_SZ` -- after which every `%USERPROFILE%\...` entry on it stops
//! expanding. The type found is the type written back. A change is announced
//! with `WM_SETTINGCHANGE`, so terminals opened afterwards see it; ones already
//! open keep the `PATH` they started with.
//!
//! New entries go to the *front* of the user `PATH`, so `php` is the stack's
//! PHP rather than another one listed earlier; entries already there are left
//! where they are. The system `PATH` comes before every user entry, so
//! something on it can still win -- `devcrate path` says when it does.
//!
//! `install` puts a runtime's folders here the first time it installs that
//! runtime (see [`register`]); what that adds is recorded in the root's
//! `devcrate-setup.json`, so `setup --undo` takes it back out.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::config::{ServiceKind, Stack};
use crate::install::Runtime;
use crate::{exit, integration};

/// One folder the stack wants on `PATH`.
#[derive(Debug, Clone)]
pub struct Entry {
    /// What it answers to in `devcrate path add <name>`: `php`, `postgres`.
    pub name: &'static str,
    pub dir: PathBuf,
    /// The commands it makes global, for the listing.
    pub gives: &'static str,
    /// The file whose resolution decides whether this entry wins, or `None`
    /// for a folder of whatever npm puts in it.
    pub probe: Option<&'static str>,
}

/// Every folder the stack can put on `PATH`, in the order they are added.
/// Services that are started rather than run (nginx, RabbitMQ, Mailpit) have
/// none: `devcrate start` runs them from where they are.
pub fn entries(stack: &Stack) -> Vec<Entry> {
    let root = &stack.root;
    // A service's binaries are wherever the stack found its executable, which
    // devcrate.toml can move; the default layout otherwise.
    let bin_of = |kind: ServiceKind, default: PathBuf| {
        stack
            .by_kind(kind)
            .and_then(|service| service.install_marker.parent().map(Path::to_path_buf))
            .unwrap_or(default)
    };
    let entry = |name, dir, gives, probe| Entry { name, dir, gives, probe };
    vec![
        entry("devcrate", root.clone(), "devcrate, phpuse, new-vhost, mkcert", Some("devcrate.exe")),
        entry("php", stack.php_dir.join("current"), "php", Some("php.exe")),
        entry("composer", root.join("composer"), "composer", Some("composer.bat")),
        entry(
            "mariadb",
            bin_of(ServiceKind::MariaDb, root.join("mariadb").join("bin")),
            "mariadb, mariadb-dump",
            Some("mariadb.exe"),
        ),
        entry(
            "postgres",
            bin_of(ServiceKind::Postgres, root.join("postgres").join("bin")),
            "psql, pg_dump, createdb",
            Some("psql.exe"),
        ),
        entry("redis", bin_of(ServiceKind::Redis, root.join("redis")), "redis-cli", Some("redis-cli.exe")),
        entry("python", root.join("python").join("current"), "python", Some("python.exe")),
        entry("python", root.join("python").join("current").join("Scripts"), "pip", Some("pip.exe")),
        entry("node", root.join("node").join("current"), "node, npm, npx", Some("node.exe")),
        entry("node", root.join("node").join("npm-global"), "npm global packages", None),
        entry("bun", root.join("bun").join("current"), "bun", Some("bun.exe")),
    ]
}

/// The entry names `path add` and `path remove` take, besides the runtime
/// spellings `install` accepts.
fn names(stack: &Stack, wanted: &[String]) -> Result<Vec<Entry>> {
    let all = entries(stack);
    if wanted.is_empty() {
        return Ok(all);
    }
    let mut out: Vec<Entry> = Vec::new();
    for name in wanted {
        let key = match name.trim().to_ascii_lowercase().as_str() {
            "devcrate" | "root" => "devcrate",
            other => match Runtime::parse(other) {
                Ok(runtime) => runtime.as_str(),
                Err(_) => bail!(
                    "{name:?} is not something devcrate puts on PATH.\n\
                     Known: devcrate, php, composer, mariadb, postgres, redis, python, node, bun"
                ),
            },
        };
        let found: Vec<Entry> = all.iter().filter(|e| e.name == key).cloned().collect();
        if found.is_empty() {
            bail!("{key} has nothing to put on PATH: it is a service, run by `devcrate start` from where it is");
        }
        for entry in found {
            if !out.iter().any(|known| same_dir(&known.dir.to_string_lossy(), &entry.dir.to_string_lossy())) {
                out.push(entry);
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The list itself
// ---------------------------------------------------------------------------

/// `C:\Tools\` and `c:/tools` are the same entry.
fn normalise(dir: &str) -> String {
    dir.trim().trim_matches('"').replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase()
}

fn same_dir(a: &str, b: &str) -> bool {
    normalise(a) == normalise(b)
}

/// The non-empty entries of a `PATH` value.
fn split(value: &str) -> Vec<&str> {
    value.split(';').filter(|part| !part.trim().is_empty()).collect()
}

/// `value` with each of `dirs` it lacks put in front, in order; and which
/// those were. An entry is compared after `expand`, so `%USERPROFILE%\x` and
/// the folder it names count as one. The rest of `value` is kept exactly as
/// written, stray `;` included.
fn with_added(value: &str, dirs: &[String], expand: &dyn Fn(&str) -> String) -> (String, Vec<String>) {
    let existing: Vec<String> = split(value).iter().map(|part| expand(part)).collect();
    let mut added: Vec<String> = Vec::new();
    for dir in dirs {
        let known = existing.iter().chain(added.iter()).any(|e| same_dir(e, dir));
        if !known {
            added.push(dir.clone());
        }
    }
    let joined = match (added.is_empty(), value.is_empty()) {
        (true, _) => value.to_string(),
        (false, true) => added.join(";"),
        (false, false) => format!("{};{value}", added.join(";")),
    };
    (joined, added)
}

/// `value` without any of `dirs`; and the entries taken out, as written.
/// Everything else is kept as it was, empty entries included.
fn without(value: &str, dirs: &[String], expand: &dyn Fn(&str) -> String) -> (String, Vec<String>) {
    let mut kept = Vec::new();
    let mut removed = Vec::new();
    for part in value.split(';') {
        let full = expand(part);
        match !part.trim().is_empty() && dirs.iter().any(|dir| same_dir(&full, dir)) {
            true => removed.push(part.to_string()),
            false => kept.push(part),
        }
    }
    (kept.join(";"), removed)
}

// ---------------------------------------------------------------------------
// What is true now
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct Row {
    pub entry: Entry,
    pub installed: bool,
    pub on_path: bool,
    /// A folder earlier in the effective `PATH` (system, then user) that
    /// answers to the same command, so this entry never gets to.
    pub shadowed_by: Option<String>,
}

pub fn status(stack: &Stack) -> Result<Vec<Row>> {
    let user = reg::read_user_path()?.map(|(value, _)| value).unwrap_or_default();
    let system = reg::read_system_path().unwrap_or_default();
    let user_dirs: Vec<String> = split(&user).iter().map(|p| reg::expand(p)).collect();
    // What a new terminal searches, in its order.
    let search: Vec<String> = split(&system).iter().map(|p| reg::expand(p)).chain(user_dirs.iter().cloned()).collect();

    let rows = entries(stack)
        .into_iter()
        .map(|entry| {
            let dir = entry.dir.to_string_lossy().into_owned();
            let on_path = user_dirs.iter().any(|known| same_dir(known, &dir));
            let shadowed_by = match (on_path, entry.probe) {
                (true, Some(file)) if entry.dir.join(file).is_file() => search
                    .iter()
                    .find(|candidate| Path::new(candidate.as_str()).join(file).is_file())
                    .filter(|winner| !same_dir(winner, &dir))
                    .cloned(),
                _ => None,
            };
            Row { installed: entry.dir.is_dir(), on_path, shadowed_by, entry }
        })
        .collect();
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Changing it
// ---------------------------------------------------------------------------

/// What an add or a remove did, as root-independent full paths.
#[derive(Debug, Default)]
pub struct Changed {
    pub changed: Vec<String>,
    /// Asked for, but its folder does not exist yet: not added.
    pub not_installed: Vec<String>,
}

/// Put `entries` on the user `PATH` -- the ones not already there, and with
/// `existing_only` only those whose folder exists -- and record them in the
/// receipt.
fn add(stack: &Stack, entries: &[Entry], existing_only: bool) -> Result<Changed> {
    let mut result = Changed::default();
    let mut dirs = Vec::new();
    for entry in entries {
        let dir = entry.dir.to_string_lossy().into_owned();
        match !existing_only || entry.dir.is_dir() {
            true => dirs.push(dir),
            false => result.not_installed.push(dir),
        }
    }
    if dirs.is_empty() {
        return Ok(result);
    }
    let (current, kind) = reg::read_user_path()?.unwrap_or_default();
    let (value, added) = with_added(&current, &dirs, &reg::expand);
    if !added.is_empty() {
        reg::write_user_path(&value, kind)?;
        integration::record_path(stack, &added, &[])?;
    }
    result.changed = added;
    Ok(result)
}

/// Take `dirs` off the user `PATH`, however they were spelled there.
pub fn remove_dirs(stack: &Stack, dirs: &[String]) -> Result<Vec<String>> {
    let Some((current, kind)) = reg::read_user_path()? else {
        return Ok(Vec::new());
    };
    let (value, removed) = without(&current, dirs, &reg::expand);
    if !removed.is_empty() {
        reg::write_user_path(&value, kind)?;
    }
    integration::record_path(stack, &[], dirs)?;
    Ok(removed)
}

/// After installing `runtime`: put its folders, and the stack root, on the
/// user `PATH` where they are missing. `install` calls this for a fresh
/// install only, so taking an entry off again sticks until that runtime is
/// next installed from nothing.
///
/// A folder that does not exist yet is added all the same: a first PHP,
/// Python, Node.js, or Bun has no `current` junction until `use` makes one,
/// and the entry is what makes that `use` reach the command line.
pub fn register(stack: &Stack, runtime: Runtime) -> Result<Changed> {
    let wanted: Vec<Entry> = entries(stack)
        .into_iter()
        .filter(|entry| entry.name == runtime.as_str() || entry.name == "devcrate")
        .collect();
    if wanted.iter().all(|entry| entry.name == "devcrate") {
        return Ok(Changed::default());
    }
    add(stack, &wanted, false)
}

// ---------------------------------------------------------------------------
// The CLI
// ---------------------------------------------------------------------------

/// `devcrate path`.
pub fn print_status(stack: &Stack) -> Result<u8> {
    let rows = status(stack)?;
    println!("User PATH entries for {} (HKCU\\Environment):", stack.root.display());
    let width = rows.iter().map(|r| r.entry.dir.to_string_lossy().chars().count()).max().unwrap_or(0);
    let mut missing = false;
    for row in &rows {
        let dir = row.entry.dir.to_string_lossy();
        let state = match (row.on_path, row.installed) {
            (true, _) => "on ",
            (false, true) => {
                missing = true;
                "off"
            }
            (false, false) => " - ",
        };
        let mut note = row.entry.gives.to_string();
        if !row.installed {
            note.push_str(if row.on_path { "  (folder missing)" } else { "  (not installed)" });
        }
        if let Some(winner) = &row.shadowed_by {
            note.push_str(&format!("  -- but {winner} comes first"));
        }
        println!("  {state}  {dir:<width$}  {note}");
    }
    println!();
    match missing {
        true => println!("`devcrate path add` puts the ones marked off on PATH; open a new terminal afterwards."),
        false => println!("Everything installed is on PATH."),
    }
    Ok(exit::OK)
}

/// `devcrate path add [name...]`.
pub fn print_add(stack: &Stack, wanted: &[String]) -> Result<u8> {
    let entries = names(stack, wanted)?;
    let done = add(stack, &entries, true)?;
    for dir in &done.changed {
        println!("  added    {dir}");
    }
    // Naming one that is not installed deserves a word; `add` with no names
    // asks for everything, and the absent ones are no surprise.
    if !wanted.is_empty() {
        for dir in &done.not_installed {
            println!("  skipped  {dir} (not installed)");
        }
    }
    let asked_for_absent_only = !wanted.is_empty() && done.not_installed.len() == entries.len();
    match (done.changed.is_empty(), asked_for_absent_only) {
        (true, true) => println!("Nothing added: install it first."),
        (true, false) => println!("Nothing to add: already on your user PATH."),
        (false, _) => println!("Your user PATH now has them. Open a new terminal to use them."),
    }
    Ok(exit::OK)
}

/// `devcrate path remove [name...]`.
pub fn print_remove(stack: &Stack, wanted: &[String]) -> Result<u8> {
    let dirs: Vec<String> = names(stack, wanted)?.iter().map(|e| e.dir.to_string_lossy().into_owned()).collect();
    let removed = remove_dirs(stack, &dirs)?;
    for dir in &removed {
        println!("  removed  {dir}");
    }
    match removed.is_empty() {
        true => println!("Nothing to remove: none of them is on your user PATH."),
        false => println!("Taken off your user PATH. Terminals already open keep the old one."),
    }
    Ok(exit::OK)
}

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod reg {
    use anyhow::{Result, bail};
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_EXPAND_SZ, REG_SZ,
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    };

    const USER_KEY: &str = "Environment";
    const SYSTEM_KEY: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    struct Key(HKEY);

    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: the handle came from a successful RegOpenKeyExW.
            unsafe { RegCloseKey(self.0) };
        }
    }

    fn open(root: HKEY, path: &str, access: u32) -> Result<Key> {
        let path = wide(path);
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: `path` is NUL-terminated and outlives the call.
        let status = unsafe { RegOpenKeyExW(root, path.as_ptr(), 0, access, &mut key) };
        if status != ERROR_SUCCESS {
            bail!("opening the registry key {path:?} failed (error {status})", path = String::from_utf16_lossy(&path[..path.len() - 1]));
        }
        Ok(Key(key))
    }

    /// `Path` under `key`, unexpanded, and its type; `None` when it is not set.
    fn read(key: &Key) -> Result<Option<(String, u32)>> {
        let name = wide("Path");
        let mut kind = 0u32;
        let mut size = 0u32;
        // SAFETY: a size query -- no buffer is passed.
        let status = unsafe {
            RegQueryValueExW(key.0, name.as_ptr(), std::ptr::null(), &mut kind, std::ptr::null_mut(), &mut size)
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            bail!("reading Path from the registry failed (error {status})");
        }
        let mut buffer = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut bytes = (buffer.len() * 2) as u32;
        // SAFETY: `buffer` holds `bytes` bytes.
        let status = unsafe {
            RegQueryValueExW(key.0, name.as_ptr(), std::ptr::null(), &mut kind, buffer.as_mut_ptr().cast(), &mut bytes)
        };
        if status != ERROR_SUCCESS {
            bail!("reading Path from the registry failed (error {status})");
        }
        if kind != REG_SZ && kind != REG_EXPAND_SZ {
            bail!("Path in the registry is not a string (type {kind}); leaving it alone");
        }
        let len = (bytes as usize / 2).min(buffer.len());
        let text = &buffer[..len];
        let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
        Ok(Some((String::from_utf16_lossy(&text[..end]), kind)))
    }

    /// The user `PATH` and its registry type (`REG_EXPAND_SZ` when unset).
    pub fn read_user_path() -> Result<Option<(String, u32)>> {
        read(&open(HKEY_CURRENT_USER, USER_KEY, KEY_QUERY_VALUE)?)
    }

    pub fn read_system_path() -> Option<String> {
        let key = open(HKEY_LOCAL_MACHINE, SYSTEM_KEY, KEY_QUERY_VALUE).ok()?;
        read(&key).ok().flatten().map(|(value, _)| value)
    }

    /// Write the user `PATH` as `kind` (0: not set before, so `REG_EXPAND_SZ`),
    /// then tell running programs the environment changed.
    pub fn write_user_path(value: &str, kind: u32) -> Result<()> {
        let kind = if kind == REG_SZ { REG_SZ } else { REG_EXPAND_SZ };
        let key = open(HKEY_CURRENT_USER, USER_KEY, KEY_SET_VALUE)?;
        let name = wide("Path");
        let data = wide(value);
        // SAFETY: `data` is the NUL-terminated value, `data.len() * 2` bytes long.
        let status = unsafe {
            RegSetValueExW(key.0, name.as_ptr(), 0, kind, data.as_ptr().cast(), (data.len() * 2) as u32)
        };
        if status != ERROR_SUCCESS {
            bail!("writing Path to the registry failed (error {status})");
        }
        drop(key);
        let environment = wide("Environment");
        let mut result = 0usize;
        // SAFETY: a broadcast with a string lParam that outlives the call; a
        // window that does not answer within the timeout is skipped.
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                environment.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                2000,
                &mut result,
            );
        }
        Ok(())
    }

    /// `%USERPROFILE%\bin` -> `C:\Users\you\bin`, as a new terminal would see it.
    pub fn expand(text: &str) -> String {
        if !text.contains('%') {
            return text.to_string();
        }
        let source = wide(text);
        // SAFETY: a size query; then a buffer of the size it asked for.
        let needed = unsafe { ExpandEnvironmentStringsW(source.as_ptr(), std::ptr::null_mut(), 0) };
        if needed == 0 {
            return text.to_string();
        }
        let mut buffer = vec![0u16; needed as usize];
        let written = unsafe { ExpandEnvironmentStringsW(source.as_ptr(), buffer.as_mut_ptr(), needed) };
        if written == 0 || written > needed {
            return text.to_string();
        }
        String::from_utf16_lossy(&buffer[..written as usize - 1])
    }
}

#[cfg(not(windows))]
mod reg {
    use anyhow::{Result, bail};

    pub fn read_user_path() -> Result<Option<(String, u32)>> {
        bail!("the user PATH lives in the Windows registry")
    }
    pub fn read_system_path() -> Option<String> {
        None
    }
    pub fn write_user_path(_value: &str, _kind: u32) -> Result<()> {
        bail!("the user PATH lives in the Windows registry")
    }
    pub fn expand(text: &str) -> String {
        text.to_string()
    }
}

/// Run an add, reporting through `line`; a failure is a line too, never an
/// error -- an install is done whether or not `PATH` could be changed.
pub fn register_after_install(stack: &Stack, runtime: Runtime, line: &mut dyn FnMut(&str)) {
    match register(stack, runtime).context("updating the user PATH") {
        Ok(done) if done.changed.is_empty() => {}
        Ok(done) => {
            line("");
            for dir in &done.changed {
                line(&format!("  on PATH   {dir}"));
            }
            line("  (your user PATH; open a new terminal to use it -- `devcrate path remove` takes it off)");
        }
        Err(err) => {
            line("");
            line(&format!("NOTE: {err:#}; `devcrate path add` tries again."));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> String {
        text.replace("%HOME%", r"C:\Users\you")
    }

    #[test]
    fn new_entries_go_first_and_known_ones_stay_put() {
        let (value, added) = with_added(
            r"C:\Windows;E:\dev\;%HOME%\bin",
            &[r"E:\dev".into(), r"E:\dev\postgres\bin".into(), r"c:/users/YOU/bin".into()],
            &plain,
        );
        assert_eq!(added, vec![r"E:\dev\postgres\bin".to_string()]);
        assert_eq!(value, r"E:\dev\postgres\bin;C:\Windows;E:\dev\;%HOME%\bin");
    }

    #[test]
    fn adding_nothing_new_leaves_the_value_alone() {
        let (value, added) = with_added(r"E:\dev;;E:\dev\php\current;", &[r"E:\DEV\PHP\Current\".into()], &plain);
        assert!(added.is_empty());
        assert_eq!(value, r"E:\dev;;E:\dev\php\current;", "stray separators and all");
    }

    #[test]
    fn a_dir_asked_for_twice_is_added_once() {
        let (value, added) = with_added("", &[r"E:\dev".into(), r"e:\dev\".into()], &plain);
        assert_eq!(added, vec![r"E:\dev".to_string()]);
        assert_eq!(value, r"E:\dev");
    }

    #[test]
    fn removing_matches_any_spelling_and_keeps_the_rest_in_order() {
        let (value, removed) = without(
            r"%HOME%\bin;E:\dev\postgres\bin\;C:\Windows;e:/dev/postgres/bin",
            &[r"E:\dev\postgres\bin".into(), r"C:\Users\you\bin".into()],
            &plain,
        );
        assert_eq!(removed, vec![r"%HOME%\bin", r"E:\dev\postgres\bin\", "e:/dev/postgres/bin"]);
        assert_eq!(value, r"C:\Windows");
    }

    /// An add and the remove of the same entry give back the value as it was,
    /// down to the trailing `;` many PATHs end with.
    #[test]
    fn an_add_then_a_remove_round_trips_exactly() {
        let original = r"C:\Windows;;%HOME%\bin;";
        let dirs = [r"E:\dev\postgres\bin".to_string()];
        let (added, _) = with_added(original, &dirs, &plain);
        assert_eq!(added, r"E:\dev\postgres\bin;C:\Windows;;%HOME%\bin;");
        let (back, removed) = without(&added, &dirs, &plain);
        assert_eq!(removed.len(), 1);
        assert_eq!(back, original);
    }

    #[test]
    fn names_resolve_to_their_folders() {
        use crate::root::{RootSource, StackRoot};
        let root = std::env::temp_dir().join(format!("devcrate-userpath-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("nginx").join("conf")).unwrap();
        std::fs::write(root.join("nginx").join("conf").join("nginx.conf"), "").unwrap();
        let stack = Stack::open(StackRoot { path: root.clone(), source: RootSource::Flag }).unwrap();

        let python = names(&stack, &["python".into()]).unwrap();
        assert_eq!(python.len(), 2, "python\\current and its Scripts");
        let pg = names(&stack, &["PostgreSQL".into(), "postgres".into()]).unwrap();
        assert_eq!(pg.len(), 1, "two spellings, one entry");
        assert!(pg[0].dir.ends_with(r"postgres\bin"));
        let err = names(&stack, &["nginx".into()]).unwrap_err().to_string();
        assert!(err.contains("service"), "{err}");
        assert!(names(&stack, &["perl".into()]).is_err());
        assert_eq!(names(&stack, &[]).unwrap().len(), entries(&stack).len());
        let _ = std::fs::remove_dir_all(&root);
    }
}
