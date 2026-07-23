//! The downloading half of `devcrate install`: the version catalogue on
//! windows.php.net, the fetch, and the checksum that ties them together.
//!
//! The catalogue is `releases.json`, which lists the current release of every
//! branch -- EOL branches included, so everything this stack runs (7.4 up) is
//! on it. Each entry carries the release's sha256, which is what makes the
//! download verifiable: the hash and the bytes come from the same feed, so a
//! truncated or tampered transfer cannot install. Superseded patch releases
//! move to the vendor's `archives/` and are deliberately not offered -- an
//! older build can still be installed with `--from`.
//!
//! Nothing here prints. The catalogue and the fetch return structured results
//! and report progress through a callback, same as [`super::from_archive`],
//! so the dashboard can drive them without touching stdout.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

/// Where the catalogue and the archives live. windows.php.net answers with a
/// redirect to downloads.php.net; ureq follows it.
const RELEASES_URL: &str = "https://windows.php.net/downloads/releases/";

/// One installable PHP release, as the feed describes it.
#[derive(Debug, Clone)]
pub struct PhpRelease {
    /// The branch, and the name the stack knows the version by: `8.4`.
    pub version: String,
    /// The branch's current release: `8.4.23`.
    pub release: String,
    /// The thread-safe x64 zip: `php-8.4.23-Win32-vs17-x64.zip`.
    pub file_name: String,
    /// Lowercase hex sha256 of that zip, from the feed.
    pub sha256: String,
    /// The feed's human-readable size label, e.g. `31.99MB`. Display only.
    pub size: String,
}

impl PhpRelease {
    pub fn url(&self) -> String {
        format!("{RELEASES_URL}{}", self.file_name)
    }
}

/// A verified archive on disk, ready for [`super::from_archive`].
#[derive(Debug)]
pub struct Fetched {
    pub path: PathBuf,
    /// The file was already present with the right checksum; nothing was
    /// transferred.
    pub cached: bool,
}

/// The timeouts cover the phases with a bounded cost -- connecting and waiting
/// for headers. The body transfer itself is deliberately uncapped: a whole-
/// request timeout long enough for a PHP zip on a slow connection would be
/// too long to be worth anything, and the progress callback already shows
/// whether bytes are moving.
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .user_agent(concat!("devcrate/", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

/// The releases windows.php.net offers, oldest branch first.
pub fn php_catalogue() -> Result<Vec<PhpRelease>> {
    let url = format!("{RELEASES_URL}releases.json");
    let mut response = agent()
        .get(&url)
        .call()
        .with_context(|| format!("fetching the PHP release list from {url}"))?;
    let json = response
        .body_mut()
        .read_to_string()
        .with_context(|| format!("reading the PHP release list from {url}"))?;
    parse_php_catalogue(&json)
}

/// Pull the thread-safe x64 zips out of the feed.
///
/// The build keys embed the compiler the release was built with (`ts-vc15-x64`,
/// `ts-vs16-x64`, `ts-vs17-x64`, ...), which changes across branches, so the
/// match is on the `ts-` and `-x64` around it and never on the middle. A
/// branch with no such build is skipped rather than an error -- the feed also
/// carries x86 and NTS builds this stack cannot use.
fn parse_php_catalogue(json: &str) -> Result<Vec<PhpRelease>> {
    let root: serde_json::Value =
        serde_json::from_str(json).context("releases.json is not valid JSON")?;
    let branches = root
        .as_object()
        .context("releases.json is not the version-keyed object it used to be")?;

    let mut catalogue = Vec::new();
    for (branch, info) in branches {
        let Some(release) = info.get("version").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(zip) = info
            .as_object()
            .into_iter()
            .flatten()
            .find(|(key, _)| key.starts_with("ts-") && key.ends_with("-x64"))
            .and_then(|(_, build)| build.get("zip"))
        else {
            continue;
        };
        let (Some(file_name), Some(sha256)) = (
            zip.get("path").and_then(|v| v.as_str()),
            zip.get("sha256").and_then(|v| v.as_str()),
        ) else {
            continue;
        };

        catalogue.push(PhpRelease {
            version: branch.clone(),
            release: release.to_string(),
            file_name: file_name.to_string(),
            sha256: sha256.to_ascii_lowercase(),
            size: zip.get("size").and_then(|v| v.as_str()).unwrap_or("?").to_string(),
        });
    }

    if catalogue.is_empty() {
        bail!(
            "releases.json lists no thread-safe x64 builds at all; \
             its format has probably changed"
        );
    }

    // The branch keys are strings, so "10.0" would sort before "7.4" -- order
    // by the numbers instead.
    catalogue.sort_by_key(|release| numeric_version(&release.version));
    Ok(catalogue)
}

/// `8.4` -> (8, 4), for ordering. Anything unparseable sorts first.
fn numeric_version(version: &str) -> (u32, u32) {
    let mut parts = version.split('.');
    let mut next = || parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (next(), next())
}

/// Download a release into `dir`, verify it, and hand back the path.
///
/// The transfer is written to a `.part` file and hashed as it streams; only a
/// transfer whose sha256 matches the feed is renamed to the real name. A file
/// already there under the final name is therefore a verified one -- it is
/// re-hashed, and kept if it still matches, so an archive downloaded once
/// keeps working offline. One that no longer matches is thrown away and
/// fetched again.
pub fn fetch_php(
    release: &PhpRelease,
    dir: &Path,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<Fetched> {
    let dest = dir.join(&release.file_name);
    if dest.is_file() {
        if sha256_of(&dest)?.eq_ignore_ascii_case(&release.sha256) {
            return Ok(Fetched { path: dest, cached: true });
        }
        std::fs::remove_file(&dest)
            .with_context(|| format!("removing the corrupt {}", dest.display()))?;
    }

    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let part = dir.join(format!("{}.part", release.file_name));

    let result = stream_to(&part, release, progress);
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result?;

    std::fs::rename(&part, &dest)
        .with_context(|| format!("moving the download to {}", dest.display()))?;
    Ok(Fetched { path: dest, cached: false })
}

fn stream_to(
    part: &Path,
    release: &PhpRelease,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<()> {
    let url = release.url();
    let mut response =
        agent().get(&url).call().with_context(|| format!("downloading {url}"))?;
    let total = response.body().content_length();

    let mut reader = response.body_mut().as_reader();
    let mut file =
        File::create(part).with_context(|| format!("creating {}", part.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut done = 0u64;

    loop {
        let n = reader.read(&mut buffer).with_context(|| format!("downloading {url}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        file.write_all(&buffer[..n])
            .with_context(|| format!("writing {}", part.display()))?;
        done += n as u64;
        progress(done, total);
    }
    file.flush()?;
    drop(file);

    let got = hex(&hasher.finalize());
    if !got.eq_ignore_ascii_case(&release.sha256) {
        bail!(
            "checksum mismatch for {}:\n  expected {}\n  got      {got}\n\
             The transfer was discarded; run the install again.",
            release.file_name,
            release.sha256
        );
    }
    Ok(())
}

/// Streaming sha256 of a file on disk, as lowercase hex.
pub fn sha256_of(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped like the real feed: build keys embed the compiler, which varies
    /// by branch; each branch also carries builds the stack cannot use.
    const FEED: &str = r#"{
        "7.4": {
            "version": "7.4.33",
            "nts-vc15-x64": {"zip": {"path": "php-7.4.33-nts-Win32-vc15-x64.zip", "sha256": "AA", "size": "24.92MB"}},
            "ts-vc15-x64":  {"zip": {"path": "php-7.4.33-Win32-vc15-x64.zip", "sha256": "CDBB85", "size": "25.02MB"}},
            "ts-vc15-x86":  {"zip": {"path": "php-7.4.33-Win32-vc15-x86.zip", "sha256": "BB", "size": "23.18MB"}},
            "source": {"path": "php-7.4.33-src.zip", "sha256": "CC"}
        },
        "8.4": {
            "version": "8.4.23",
            "ts-vs17-x64": {"zip": {"path": "php-8.4.23-Win32-vs17-x64.zip", "sha256": "dd17", "size": "31.99MB"}}
        },
        "10.0": {
            "version": "10.0.1",
            "ts-vs20-x64": {"zip": {"path": "php-10.0.1-Win32-vs20-x64.zip", "sha256": "ee", "size": "40MB"}}
        },
        "9.9": {
            "version": "9.9.9"
        }
    }"#;

    #[test]
    fn the_catalogue_takes_one_ts_x64_zip_per_branch() {
        let catalogue = parse_php_catalogue(FEED).unwrap();
        let names: Vec<&str> = catalogue.iter().map(|r| r.file_name.as_str()).collect();
        // 9.9 has no ts x64 build and is skipped, not an error. The x86, NTS,
        // and source entries never make it in.
        assert_eq!(
            names,
            [
                "php-7.4.33-Win32-vc15-x64.zip",
                "php-8.4.23-Win32-vs17-x64.zip",
                "php-10.0.1-Win32-vs20-x64.zip",
            ]
        );
        assert_eq!(catalogue[0].version, "7.4");
        assert_eq!(catalogue[0].release, "7.4.33");
        // The feed's hex is taken lowercase, whatever case it arrived in.
        assert_eq!(catalogue[0].sha256, "cdbb85");
    }

    /// The branch keys are strings; a two-digit major must not sort between
    /// 1.x and 7.x the way a lexical sort would put it.
    #[test]
    fn the_catalogue_is_ordered_by_version_not_by_string() {
        let catalogue = parse_php_catalogue(FEED).unwrap();
        let versions: Vec<&str> = catalogue.iter().map(|r| r.version.as_str()).collect();
        assert_eq!(versions, ["7.4", "8.4", "10.0"]);
    }

    #[test]
    fn a_feed_with_no_usable_builds_is_an_error_not_an_empty_list() {
        let err = parse_php_catalogue(r#"{"9.9": {"version": "9.9.9"}}"#).unwrap_err();
        assert!(err.to_string().contains("format has probably changed"), "{err}");
        assert!(parse_php_catalogue("not json").is_err());
    }

    #[test]
    fn the_archive_url_is_the_feed_path_on_the_releases_directory() {
        let release = PhpRelease {
            version: "8.4".into(),
            release: "8.4.23".into(),
            file_name: "php-8.4.23-Win32-vs17-x64.zip".into(),
            sha256: "dd".into(),
            size: "31.99MB".into(),
        };
        assert_eq!(
            release.url(),
            "https://windows.php.net/downloads/releases/php-8.4.23-Win32-vs17-x64.zip"
        );
    }

    #[test]
    fn sha256_of_hashes_what_is_on_disk() {
        let dir = std::env::temp_dir().join("devcrate-test-sha256");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("abc.txt");
        std::fs::write(&file, "abc").unwrap();
        // The FIPS-180 test vector for "abc".
        assert_eq!(
            sha256_of(&file).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
