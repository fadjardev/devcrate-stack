//! What turns an unpacked PHP archive into a PHP the stack can serve with.
//!
//! Three things the vendor's zip does not do for itself: say which version it
//! holds in a form the folder naming can use, prove it is the thread-safe build
//! the FastCGI workers need, and carry a usable `php.ini`.

use std::path::Path;

use anyhow::{Context, Result};

/// The extensions a Devcrate PHP is expected to have on.
///
/// This is the set the stack's existing `php\php-8.5\php.ini` enables, and the
/// list issue #2 fixes. It is written here rather than discovered because the
/// point of generating the file is that a fresh install matches the ones that
/// are already working.
pub const EXTENSIONS: [&str; 12] = [
    "curl",
    "exif",
    "fileinfo",
    "gd",
    "intl",
    "mbstring",
    "openssl",
    "pdo_mysql",
    "pdo_sqlite",
    "sodium",
    "sqlite3",
    "zip",
];

/// Which of the two Windows builds an unpacked PHP is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Build {
    ThreadSafe,
    NonThreadSafe,
}

/// `php-8.4.3-Win32-vs17-x64.zip` -> (`8.4`, `8.4.3`).
///
/// The stack knows a version by its major.minor, because that is what both the
/// folder name and the FastCGI port derive from -- `php-8.4` serves on 9084.
/// The patch level is kept separately for the install receipt, where it is a
/// fact about the download rather than part of a path.
pub fn version_from_file_name(name: &str) -> Option<(String, String)> {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".zip").unwrap_or(&lower);
    let rest = stem.strip_prefix("php-")?;

    // `php-8.4.3-Win32-vs17-x64` and `php-8.4.3-nts-Win32-vs17-x64` both put the
    // release first, so everything from the next dash on is irrelevant here.
    let release = rest.split('-').next()?;
    if release.is_empty() || !release.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }

    let mut parts = release.split('.');
    let major = parts.next().filter(|p| !p.is_empty())?;
    let minor = parts.next().filter(|p| !p.is_empty())?;

    Some((format!("{major}.{minor}"), release.to_string()))
}

/// Thread-safe or not, read from the unpacked directory.
///
/// A TS build ships `php8ts.dll` (or `php7ts.dll`); an NTS build ships
/// `php8.dll`. Reading the directory rather than the file name is what makes
/// this survive a renamed download -- and the file name is a poor signal
/// anyway, because the TS build is the one with *no* marker in it while NTS
/// carries `-nts-`. Guessing that from a name gets it backwards as often as
/// not.
pub fn detect_build(dir: &Path) -> Build {
    let thread_safe = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            name.starts_with("php") && name.ends_with("ts.dll")
        });

    match thread_safe {
        true => Build::ThreadSafe,
        false => Build::NonThreadSafe,
    }
}

/// What generating a `php.ini` came to.
#[derive(Debug)]
pub struct Ini {
    pub enabled: Vec<String>,
    /// Extensions [`EXTENSIONS`] asks for that the template had no line for.
    /// Reported rather than swallowed: a version that silently came up without
    /// `intl` is a confusing afternoon later on.
    pub missing: Vec<String>,
}

/// Write `php.ini` beside the binaries, seeded from the shipped
/// `php.ini-development`.
///
/// Seeding from the vendor's own template rather than writing one from scratch
/// keeps every comment and every default the release came with, so the result
/// is the file a person would have produced by hand -- which is exactly what
/// the existing `php\php-8.5\php.ini` is.
pub fn write_ini(dir: &Path) -> Result<Ini> {
    let template = dir.join("php.ini-development");
    let text = std::fs::read_to_string(&template).with_context(|| {
        format!(
            "reading {} -- every PHP release ships one, so an archive without it \
             is not a PHP distribution",
            template.display()
        )
    })?;

    let (configured, enabled) = configure(&text);

    let ini = dir.join("php.ini");
    std::fs::write(&ini, configured).with_context(|| format!("writing {}", ini.display()))?;

    let missing = EXTENSIONS
        .iter()
        .filter(|wanted| !enabled.iter().any(|got| got == *wanted))
        .map(|wanted| (*wanted).to_string())
        .collect();

    Ok(Ini { enabled, missing })
}

/// Apply the stack's settings to a `php.ini` template, returning the new text
/// and the extensions that were switched on.
fn configure(text: &str) -> (String, Vec<String>) {
    let mut enabled = Vec::new();
    let ends_with_newline = text.ends_with('\n');

    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        match uncomment(line) {
            Some((replacement, extension)) => {
                if let Some(extension) = extension {
                    enabled.push(extension);
                }
                out.push(replacement);
            }
            None => out.push(line.to_string()),
        }
    }

    let mut text = out.join("\n");
    if ends_with_newline {
        text.push('\n');
    }
    (text, enabled)
}

/// Turn one commented directive into an active one, when it is a directive this
/// stack wants on. Returns the replacement line, and the extension name when
/// the line was one.
///
/// Matching on the key *and* the value is deliberate. `php.ini-development`
/// carries several commented settings for the same key: `extension_dir` appears
/// once as `"./"` and once as `"ext"`, `error_log` as both `php_errors.log` and
/// `syslog`. Uncommenting on the key alone would enable whichever happened to
/// come last in the file.
fn uncomment(line: &str) -> Option<(String, Option<String>)> {
    let body = line.trim_start().strip_prefix(';')?;
    let (key, value) = body.split_once('=')?;
    let key = key.trim().to_ascii_lowercase();
    let value = value.trim();

    match (key.as_str(), value) {
        // Relative, because that is what the stack's existing php.ini files use
        // and what lets a version directory be moved with the stack root.
        ("extension_dir", "\"ext\"") => Some(("extension_dir = \"ext\"".to_string(), None)),
        ("error_log", "php_errors.log") => {
            Some(("error_log = php_errors.log".to_string(), None))
        }
        ("extension", extension) if EXTENSIONS.contains(&extension) => {
            Some((format!("extension={extension}"), Some(extension.to_string())))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_come_from_the_vendors_own_file_name() {
        assert_eq!(
            version_from_file_name("php-8.4.3-Win32-vs17-x64.zip"),
            Some(("8.4".to_string(), "8.4.3".to_string()))
        );
        // The non-thread-safe build names itself; the thread-safe one does not.
        assert_eq!(
            version_from_file_name("php-8.4.3-nts-Win32-vs17-x64.zip"),
            Some(("8.4".to_string(), "8.4.3".to_string()))
        );
        assert_eq!(
            version_from_file_name("php-7.4.33-Win32-vc15-x64.zip"),
            Some(("7.4".to_string(), "7.4.33".to_string()))
        );
    }

    /// A renamed download cannot be guessed at, and saying so beats installing
    /// it as the wrong version.
    #[test]
    fn a_name_without_a_version_is_not_guessed() {
        assert_eq!(version_from_file_name("php.zip"), None);
        assert_eq!(version_from_file_name("nginx-1.31.1.zip"), None);
        assert_eq!(version_from_file_name("php-latest-x64.zip"), None);
        // A major with no minor cannot name a folder: php-8 has no port.
        assert_eq!(version_from_file_name("php-8-Win32.zip"), None);
    }

    /// The stack runs php-cgi.exe as a long-lived FastCGI listener with
    /// PHP_FCGI_CHILDREN, so a non-thread-safe build is the wrong one -- and
    /// telling them apart by file name gets it backwards.
    #[test]
    fn thread_safety_is_read_from_the_files_not_the_name() {
        let dir = std::env::temp_dir().join("devcrate-install-build");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(dir.join("php.exe"), b"").unwrap();
        std::fs::write(dir.join("php8.dll"), b"").unwrap();
        assert_eq!(detect_build(&dir), Build::NonThreadSafe);

        std::fs::write(dir.join("php8ts.dll"), b"").unwrap();
        assert_eq!(detect_build(&dir), Build::ThreadSafe);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_extension_set_matches_the_stacks_existing_php_ini() {
        let template = "\
;extension=bz2\n\
;extension=curl\n\
;extension=ffi\n\
;extension=fileinfo\n\
;extension=mysqli\n\
;extension=pdo_sqlite\n\
;extension=sqlite3\n";

        let (text, enabled) = configure(template);
        assert!(text.contains("\nextension=curl"));
        assert!(text.contains("\nextension=pdo_sqlite"));
        assert!(text.contains("\nextension=sqlite3"));
        // Not on the list, so left exactly as the vendor shipped it.
        assert!(text.contains(";extension=bz2"));
        assert!(text.contains(";extension=ffi"));
        assert!(text.contains(";extension=mysqli"));
        assert_eq!(enabled, ["curl", "fileinfo", "pdo_sqlite", "sqlite3"]);
    }

    /// The template comments the same key more than once with different values.
    /// Uncommenting on the key alone would enable whichever came last.
    #[test]
    fn only_the_intended_value_of_a_repeated_key_is_enabled() {
        let template = "\
;extension_dir = \"./\"\n\
; extension_dir = \"ext\"\n\
;error_log = php_errors.log\n\
;error_log = syslog\n";

        let (text, _) = configure(template);
        assert!(text.contains("\nextension_dir = \"ext\""));
        assert!(text.contains(";extension_dir = \"./\""));
        assert!(text.contains("\nerror_log = php_errors.log"));
        assert!(text.contains(";error_log = syslog"));
    }

    /// Everything the generator did not deliberately change has to survive,
    /// including prose comments that happen to contain an equals sign.
    #[test]
    fn the_rest_of_the_template_comes_through_untouched() {
        let template = "\
; This is prose, and it mentions foo=bar in passing.\n\
memory_limit = 128M\n\
;extension=curl\n";

        let (text, enabled) = configure(template);
        assert!(text.contains("; This is prose, and it mentions foo=bar in passing."));
        assert!(text.contains("memory_limit = 128M"));
        assert_eq!(enabled, ["curl"]);
    }
}
