//! Reading a runtime archive, safely.
//!
//! Two rules govern everything here, and both come from constraints the stack
//! already lives under:
//!
//! - **Nothing is written outside the destination.** A zip entry carries its own
//!   path, and that path came out of a file fetched from a vendor site, so it is
//!   input rather than instruction. An absolute path, a drive letter, or a `..`
//!   is refused outright rather than sanitised: an archive that asks for one is
//!   not an archive worth unpacking.
//! - **A `..` must never reach a path the stack uses.** PHP-CGI on Windows
//!   rejects any `SCRIPT_FILENAME` containing one -- see
//!   docs/troubleshooting.md -- so a traversal entry would not merely be a
//!   security problem. It would quietly produce a PHP version that answers
//!   nothing but "No input file specified".

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// What an extraction produced.
///
/// Only the file count so far. Which wrapper directory was stripped is worked
/// out on the way through (see [`common_root`]) but nothing consumes it yet, so
/// it is not carried here -- when the nginx installer needs to report it, that
/// is the moment to add it.
#[derive(Debug)]
pub struct Extracted {
    /// Files written, not counting directories.
    pub files: usize,
}

/// Turn a zip entry's name into a path that is safe to join onto a destination,
/// or `None` if it is not one.
///
/// Deliberately stricter than it needs to be. `.` and empty components are
/// dropped because they are harmless noise, but everything else that is not a
/// plain file name -- a leading separator, a drive letter, a colon (which on
/// Windows names an alternate data stream), a `..` even where it would stay
/// inside the destination -- is a refusal. There is no legitimate runtime
/// archive that needs any of them, so treating them as fatal costs nothing and
/// removes the class of bug entirely.
pub fn safe_entry_path(name: &str) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    // Zip stores forward slashes, but an archive built on Windows may well
    // carry backslashes, and Windows treats both as separators. Normalise so
    // one rule covers both rather than checking each separately.
    let normalized = name.replace('\\', "/");
    if normalized.starts_with('/') {
        return None;
    }

    let mut out = PathBuf::new();
    for part in normalized.split('/') {
        match part {
            "" | "." => continue,
            ".." => return None,
            // A colon is a drive letter (`C:`) or an alternate data stream
            // (`file.txt:hidden`). Neither belongs in an extracted runtime.
            _ if part.contains(':') => return None,
            _ => out.push(part),
        }
    }

    (!out.as_os_str().is_empty()).then_some(out)
}

/// Extract `archive` into `dest`, reporting `(done, total)` as it goes.
///
/// `dest` is expected to be empty and to belong to the caller: this writes into
/// it without asking, and the caller is the one that decides when the result is
/// fit to be put in place.
pub fn extract(
    archive: &Path,
    dest: &Path,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Extracted> {
    let file =
        File::open(archive).with_context(|| format!("opening {}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(BufReader::new(file))
        .with_context(|| format!("reading {} as a zip archive", archive.display()))?;

    let total = zip.len();
    if total == 0 {
        bail!("{} contains no entries", archive.display());
    }

    // Names first, in one pass, because deciding whether there is a common root
    // needs to see all of them before the first file is written.
    let mut entries: Vec<(String, bool)> = Vec::with_capacity(total);
    for index in 0..total {
        let entry = zip
            .by_index(index)
            .with_context(|| format!("reading entry {index} of {}", archive.display()))?;
        entries.push((entry.name().to_string(), entry.is_dir()));
    }
    let stripped_root = common_root(&entries);

    let mut files = 0;
    for index in 0..total {
        let mut entry = zip
            .by_index(index)
            .with_context(|| format!("reading entry {index} of {}", archive.display()))?;

        let name = entry.name().to_string();
        let Some(relative) = safe_entry_path(&name) else {
            bail!(
                "{} contains an unsafe entry path and was not unpacked: {name:?}",
                archive.display()
            );
        };

        // Drop the archive's own wrapper directory, when it has one.
        let relative = match &stripped_root {
            Some(root) => match relative.strip_prefix(root) {
                Ok(rest) => rest.to_path_buf(),
                Err(_) => relative,
            },
            None => relative,
        };
        // The wrapper directory's own entry strips to nothing.
        if relative.as_os_str().is_empty() {
            progress(index + 1, total);
            continue;
        }

        let target = dest.join(&relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)
                .with_context(|| format!("creating {}", target.display()))?;
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            let out = File::create(&target)
                .with_context(|| format!("writing {}", target.display()))?;
            let mut out = BufWriter::new(out);
            std::io::copy(&mut entry, &mut out)
                .with_context(|| format!("writing {}", target.display()))?;
            out.flush().with_context(|| format!("writing {}", target.display()))?;
            files += 1;
        }

        progress(index + 1, total);
    }

    Ok(Extracted { files })
}

/// The one directory every entry sits under, if there is one.
///
/// nginx ships `nginx-1.31.1/...`; PHP ships its files at the top level. Both
/// have to end up as a directory of files, so the wrapper is stripped when
/// present. A single file at the top level means there is no wrapper, even if
/// everything else shares a directory -- stripping then would drop that file.
fn common_root(entries: &[(String, bool)]) -> Option<String> {
    let mut root: Option<String> = None;

    for (name, is_dir) in entries {
        let normalized = name.replace('\\', "/");
        let trimmed = normalized.trim_end_matches('/');
        if trimmed.is_empty() {
            continue;
        }

        let first = trimmed.split('/').next()?;
        if !trimmed.contains('/') && !is_dir {
            return None;
        }

        match &root {
            None => root = Some(first.to_string()),
            Some(seen) if seen == first => {}
            Some(_) => return None,
        }
    }

    root
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard that keeps an extraction inside the directory it was given.
    /// Every one of these has been seen in a real malicious archive.
    #[test]
    fn entry_paths_that_would_escape_the_destination_are_refused() {
        assert!(safe_entry_path("../evil.dll").is_none());
        assert!(safe_entry_path("ext/../../evil.dll").is_none());
        assert!(safe_entry_path("/etc/passwd").is_none());
        assert!(safe_entry_path("C:/Windows/System32/evil.dll").is_none());
        assert!(safe_entry_path("..\\evil.dll").is_none());
        assert!(safe_entry_path("php.exe:hidden").is_none());
        assert!(safe_entry_path("").is_none());
    }

    /// A `..` is refused even when it would land back inside the destination.
    /// Nothing legitimate needs one, so there is no case to allow.
    #[test]
    fn traversal_is_refused_even_when_it_would_stay_inside() {
        assert!(safe_entry_path("ext/../php.exe").is_none());
    }

    #[test]
    fn ordinary_entry_paths_survive_intact() {
        assert_eq!(safe_entry_path("php.exe"), Some(PathBuf::from("php.exe")));
        assert_eq!(
            safe_entry_path("ext/php_curl.dll"),
            Some(PathBuf::from("ext/php_curl.dll"))
        );
        // Noise a zip writer may leave behind, rather than anything hostile.
        assert_eq!(safe_entry_path("./php.exe"), Some(PathBuf::from("php.exe")));
        assert_eq!(safe_entry_path("ext//php_gd.dll"), Some(PathBuf::from("ext/php_gd.dll")));
    }

    /// PHP unpacks flat, nginx unpacks under its own directory. Both have to
    /// end up as a directory of files.
    #[test]
    fn a_wrapper_directory_is_detected_only_when_every_entry_is_under_it() {
        let nginx = [
            ("nginx-1.31.1/".to_string(), true),
            ("nginx-1.31.1/nginx.exe".to_string(), false),
            ("nginx-1.31.1/conf/nginx.conf".to_string(), false),
        ];
        assert_eq!(common_root(&nginx), Some("nginx-1.31.1".to_string()));

        let php = [
            ("php.exe".to_string(), false),
            ("php-cgi.exe".to_string(), false),
            ("ext/php_curl.dll".to_string(), false),
        ];
        assert_eq!(common_root(&php), None);
    }

    /// A file sitting beside the candidate wrapper means there is no wrapper:
    /// stripping would silently drop that file.
    #[test]
    fn a_file_beside_the_wrapper_means_there_is_no_wrapper() {
        let mixed = [
            ("nginx-1.31.1/nginx.exe".to_string(), false),
            ("README.txt".to_string(), false),
        ];
        assert_eq!(common_root(&mixed), None);
    }
}
