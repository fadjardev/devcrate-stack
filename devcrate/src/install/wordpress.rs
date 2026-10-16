//! Fetching WordPress for `devcrate new wordpress`: the current release, into
//! a folder the caller owns.
//!
//! Not a runtime -- nothing is recorded as installed. The zip is cached in
//! `_downloads\` under its version like any other download, and checked
//! against the `.sha1` WordPress publishes beside it, every time it is used:
//! a cached file is re-hashed too, since `fetch` alone could only take it at
//! its name. SHA-1 is weak, and it is said as SHA-1 -- but it is the vendor's
//! own hash, which is more than the length check nginx gets.

use std::path::Path;

use anyhow::{Result, bail};

use super::{DOWNLOADS_DIR, archive, download};
use crate::config::Stack;

pub use super::download::WordPressRelease;

pub struct Fetched {
    pub version: String,
    pub cached: bool,
}

/// The current release's details, without downloading it -- the preflight
/// wants `min_php` before anything is written.
pub fn release() -> Result<WordPressRelease> {
    download::wordpress_release()
}

/// Read a version-check response -- [`release`] without the network.
#[cfg(test)]
pub fn parse(json_text: &str) -> Result<WordPressRelease> {
    download::parse_wordpress_release(json_text)
}

/// Download (or reuse) `release` and unpack it into `dest`, which must be an
/// empty folder the caller owns. The zip's own `wordpress\` wrapper is
/// stripped, so `dest` ends up holding `index.php` and `wp-config-sample.php`.
pub fn unpack(
    stack: &Stack,
    release: &WordPressRelease,
    dest: &Path,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<Fetched> {
    let want = download::wordpress_sha1(release)?;
    let fetched = download::fetch(&release.download(), &stack.root.join(DOWNLOADS_DIR), progress)?;
    let got = download::sha1_of(&fetched.path)?;
    if !got.eq_ignore_ascii_case(&want) {
        let _ = std::fs::remove_file(&fetched.path);
        bail!(
            "SHA-1 mismatch for {}:\n  expected {want}\n  got      {got}\n\
             The download was discarded; run the command again.",
            release.download().file_name
        );
    }
    archive::extract(&fetched.path, dest, &mut |_, _| {})?;
    if !dest.join("wp-config-sample.php").is_file() {
        bail!("{} unpacked without a wp-config-sample.php; it is not a WordPress release", fetched.path.display());
    }
    Ok(Fetched { version: release.version.clone(), cached: fetched.cached })
}
