//! The downloading half of `devcrate install`: the version catalogues, the
//! fetch, and whatever each vendor gives us to check the bytes against.
//!
//! **PHP** publishes `releases.json`, which lists the current release of every
//! branch -- EOL branches included, so everything this stack runs (7.4 up) is
//! on it. Each entry carries the release's sha256, which is what makes the
//! download verifiable: the hash and the bytes come from the same feed, so a
//! truncated or tampered transfer cannot install. Superseded patch releases
//! move to the vendor's `archives/` and are deliberately not offered -- an
//! older build can still be installed with `--from`.
//!
//! **nginx** publishes no machine-readable catalogue and no checksums at all --
//! its download page is HTML and the only integrity material beside each zip is
//! a PGP signature. So the page is parsed, and a fetch with nothing to hash
//! against falls back to what is left: TLS to the vendor's own host, and the
//! `Content-Length` the server declared. That is weaker than PHP's, and
//! [`Download::sha256`] being an `Option` is where the difference lives rather
//! than something the caller has to remember.
//!
//! Nothing here prints. The catalogues and the fetch return structured results
//! and report progress through a callback, same as [`super::from_archive`],
//! so the dashboard can drive them without touching stdout.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

/// Where the PHP catalogue and archives live. windows.php.net answers with a
/// redirect to downloads.php.net; ureq follows it.
const RELEASES_URL: &str = "https://windows.php.net/downloads/releases/";

/// nginx's download page, and the directory the zips it names actually sit in.
const NGINX_PAGE_URL: &str = "https://nginx.org/en/download.html";
const NGINX_DOWNLOAD_URL: &str = "https://nginx.org/download/";

/// One archive to fetch, and what there is to check it against.
///
/// The one thing [`fetch`] needs to know about a release, so that adding a
/// runtime is a matter of describing where its archive is rather than of
/// teaching the transfer anything new.
#[derive(Debug, Clone)]
pub struct Download {
    pub url: String,
    pub file_name: String,
    /// Lowercase hex sha256 the vendor published, where the vendor publishes
    /// one. `None` for nginx -- see the module docs.
    pub sha256: Option<String>,
}

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
    pub fn download(&self) -> Download {
        Download {
            url: format!("{RELEASES_URL}{}", self.file_name),
            file_name: self.file_name.clone(),
            sha256: Some(self.sha256.clone()),
        }
    }
}

/// Which line of nginx a release is on, as its download page groups them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Mainline,
    Stable,
    Legacy,
}

impl Channel {
    /// From the `<h4>` the page puts above each group.
    fn from_heading(text: &str) -> Channel {
        let text = text.to_ascii_lowercase();
        match () {
            _ if text.contains("mainline") => Channel::Mainline,
            _ if text.contains("stable") => Channel::Stable,
            _ => Channel::Legacy,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Channel::Mainline => "mainline",
            Channel::Stable => "stable",
            Channel::Legacy => "legacy",
        }
    }
}

/// One installable nginx release, as the download page lists it.
#[derive(Debug, Clone)]
pub struct NginxRelease {
    /// The full version, which is also the name the stack knows it by: `1.31.3`.
    /// nginx has no branch/release split the way PHP does -- a build *is* its
    /// version, which is why `nginx-1.31.3\` is the folder name.
    pub version: String,
    /// `nginx-1.31.3.zip`.
    pub file_name: String,
    pub channel: Channel,
}

impl NginxRelease {
    pub fn download(&self) -> Download {
        Download {
            url: format!("{NGINX_DOWNLOAD_URL}{}", self.file_name),
            file_name: self.file_name.clone(),
            // nginx signs its releases with PGP and publishes no hash. See the
            // module docs for what is checked instead.
            sha256: None,
        }
    }
}

/// An archive on disk, ready for [`super::from_archive`].
#[derive(Debug)]
pub struct Fetched {
    pub path: PathBuf,
    /// The file was already there and nothing was transferred.
    pub cached: bool,
    /// The sha256 of what is on disk, computed here. Verified against the
    /// vendor's when there was one to verify against; recorded either way, so
    /// the install receipt can carry it.
    pub sha256: String,
    /// Whether that hash was checked against one the vendor published.
    pub verified: bool,
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

/// The Windows builds nginx.org offers: mainline first, then stable, then the
/// legacy series newest first -- the page's own order, which is the order
/// someone choosing a version wants to read.
pub fn nginx_catalogue() -> Result<Vec<NginxRelease>> {
    let mut response = agent()
        .get(NGINX_PAGE_URL)
        .call()
        .with_context(|| format!("fetching the nginx download page from {NGINX_PAGE_URL}"))?;
    let html = response
        .body_mut()
        .read_to_string()
        .with_context(|| format!("reading the nginx download page from {NGINX_PAGE_URL}"))?;
    parse_nginx_catalogue(&html)
}

/// The `href` every Windows build on the page is behind.
const ZIP_HREF: &str = "href=\"/download/nginx-";

/// Pull the Windows zips out of nginx's download page.
///
/// Parsing HTML is the fragile way to read a catalogue, and it is done here
/// only because nginx publishes no other kind -- no JSON, no plain-text index
/// of releases. It is kept to the two things the page has always done: put a
/// `<h4>` above each group, and link the Windows build as
/// `/download/nginx-<version>.zip`. No tags are matched, no nesting is
/// tracked, and the source tarballs beside each zip are simply not that href.
///
/// The failure mode is the point: if the page changes shape, this finds nothing
/// and says so, rather than quietly offering a truncated list.
fn parse_nginx_catalogue(html: &str) -> Result<Vec<NginxRelease>> {
    let mut releases: Vec<NginxRelease> = Vec::new();
    // Everything before the first heading, if the page ever grew such a thing,
    // is no line in particular.
    let mut channel = Channel::Legacy;
    let mut rest = html;

    loop {
        let heading = rest.find("<h4");
        let link = rest.find(ZIP_HREF);

        let cut = match (heading, link) {
            // A heading first: it labels everything up to the next one.
            (Some(at), link) if link.is_none_or(|link| at < link) => {
                let body = &rest[at..];
                let open = body.find('>').map(|i| i + 1).unwrap_or(body.len());
                let close = body.find("</h4").unwrap_or(body.len());
                if open < close {
                    channel = Channel::from_heading(&body[open..close]);
                }
                at + open
            }
            (_, Some(at)) => {
                let body = &rest[at + ZIP_HREF.len()..];
                let quoted = body.find('"').map(|end| &body[..end]).unwrap_or("");
                if let Some(version) = quoted.strip_suffix(".zip")
                    && is_version(version)
                    && !releases.iter().any(|r| r.version == version)
                {
                    releases.push(NginxRelease {
                        version: version.to_string(),
                        file_name: format!("nginx-{version}.zip"),
                        channel,
                    });
                }
                at + ZIP_HREF.len()
            }
            _ => break,
        };
        rest = &rest[cut..];
    }

    if releases.is_empty() {
        bail!(
            "no Windows builds found on {NGINX_PAGE_URL}; \
             the page's format has probably changed"
        );
    }
    Ok(releases)
}

fn is_version(text: &str) -> bool {
    !text.is_empty()
        && text.chars().all(|c| c.is_ascii_digit() || c == '.')
        && text.split('.').all(|part| !part.is_empty())
}

/// Download an archive into `dir`, check it, and hand back the path.
///
/// The transfer is written to a `.part` file and hashed as it streams; only a
/// transfer that passes is renamed to the real name. So a file sitting in the
/// directory under its *final* name is one that completed and passed, which is
/// what makes the cache trustworthy enough to reuse -- an archive downloaded
/// once installs again offline.
///
/// What "passes" means depends on what the vendor published. With a sha256 it
/// is the sha256, and a cached file is re-hashed against it and thrown away if
/// it no longer matches. Without one it is the declared `Content-Length`, which
/// catches the failure that actually happens -- a transfer cut short -- and a
/// cached file is taken at its name, because there is nothing to re-check it
/// against. That difference is reported in [`Fetched::verified`] rather than
/// glossed over.
pub fn fetch(
    download: &Download,
    dir: &Path,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<Fetched> {
    let verified = download.sha256.is_some();
    let dest = dir.join(&download.file_name);

    if dest.is_file() {
        let got = sha256_of(&dest)?;
        match &download.sha256 {
            None => return Ok(Fetched { path: dest, cached: true, sha256: got, verified }),
            Some(want) if got.eq_ignore_ascii_case(want) => {
                return Ok(Fetched { path: dest, cached: true, sha256: got, verified });
            }
            Some(_) => std::fs::remove_file(&dest)
                .with_context(|| format!("removing the corrupt {}", dest.display()))?,
        }
    }

    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let part = dir.join(format!("{}.part", download.file_name));

    let result = stream_to(&part, download, progress);
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    let sha256 = result?;

    std::fs::rename(&part, &dest)
        .with_context(|| format!("moving the download to {}", dest.display()))?;
    Ok(Fetched { path: dest, cached: false, sha256, verified })
}

/// Stream the body to `part`, returning its sha256. Errors if what arrived is
/// not what was promised, by either measure available.
fn stream_to(
    part: &Path,
    download: &Download,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<String> {
    let url = &download.url;
    let mut response =
        agent().get(url).call().with_context(|| format!("downloading {url}"))?;
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

    // Checked first because it is the more legible failure of the two: a
    // truncated transfer fails the hash as well, but "23 of 31 MB arrived"
    // says what went wrong and "the hashes differ" does not.
    if let Some(total) = total
        && done != total
    {
        bail!(
            "{} arrived incomplete: {done} of {total} bytes.\n\
             The transfer was discarded; run the install again.",
            download.file_name
        );
    }

    let got = hex(&hasher.finalize());
    if let Some(want) = &download.sha256
        && !got.eq_ignore_ascii_case(want)
    {
        bail!(
            "checksum mismatch for {}:\n  expected {want}\n  got      {got}\n\
             The transfer was discarded; run the install again.",
            download.file_name
        );
    }
    Ok(got)
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
        let download = release.download();
        assert_eq!(
            download.url,
            "https://windows.php.net/downloads/releases/php-8.4.23-Win32-vs17-x64.zip"
        );
        assert_eq!(download.sha256.as_deref(), Some("dd"));
    }

    /// The shape nginx.org's download page has: an `<h4>` naming each line,
    /// then a table per series whose third cell links the Windows zip beside
    /// the source tarball and the two PGP signatures.
    const PAGE: &str = r#"
        <div id="content"><h2>nginx: download</h2>
        <center><h4>Mainline version</h4></center>
        <table><tr><td><a href="/en/CHANGES">CHANGES</a></td>
        <td><a href="/download/nginx-1.31.3.tar.gz">nginx-1.31.3</a> <a href="/download/nginx-1.31.3.tar.gz.asc">pgp</a></td>
        <td><a href="/download/nginx-1.31.3.zip">nginx/Windows-1.31.3</a> <a href="/download/nginx-1.31.3.zip.asc">pgp</a></td></tr></table>
        <center><h4>Stable version</h4></center>
        <table><tr><td><a href="/download/nginx-1.30.4.tar.gz">nginx-1.30.4</a></td>
        <td><a href="/download/nginx-1.30.4.zip">nginx/Windows-1.30.4</a></td></tr></table>
        <center><h4>Legacy versions</h4></center>
        <table><tr><td><a href="/download/nginx-1.28.3.zip">nginx/Windows-1.28.3</a></td></tr></table>
        <table><tr><td><a href="/download/nginx-0.8.55.zip">nginx/Windows-0.8.55</a></td></tr></table>
        <table><tr><td><a href="/download/nginx-0.6.39.tar.gz">nginx-0.6.39</a></td><td></td></tr></table>
        <center><h4>Pre-Built Packages</h4></center></div>
    "#;

    #[test]
    fn the_nginx_catalogue_takes_the_windows_zips_in_page_order() {
        let catalogue = parse_nginx_catalogue(PAGE).unwrap();
        let versions: Vec<&str> = catalogue.iter().map(|r| r.version.as_str()).collect();
        // The tarballs, the .asc signatures, and the oldest series -- which has
        // no Windows build at all -- are all absent.
        assert_eq!(versions, ["1.31.3", "1.30.4", "1.28.3", "0.8.55"]);
        assert_eq!(catalogue[0].file_name, "nginx-1.31.3.zip");
    }

    /// Which line a release is on is the reason the headings are read at all:
    /// it is the difference between the version to install and the one to
    /// install only if you mean it.
    #[test]
    fn each_release_carries_the_line_its_heading_named() {
        let catalogue = parse_nginx_catalogue(PAGE).unwrap();
        let channels: Vec<Channel> = catalogue.iter().map(|r| r.channel).collect();
        assert_eq!(
            channels,
            [Channel::Mainline, Channel::Stable, Channel::Legacy, Channel::Legacy]
        );
    }

    /// A page this finds nothing in has to be an error. Silently offering an
    /// empty list would read as "nginx has no releases".
    #[test]
    fn a_page_with_no_windows_builds_is_an_error_not_an_empty_list() {
        let err = parse_nginx_catalogue("<h4>Mainline version</h4>").unwrap_err();
        assert!(err.to_string().contains("format has probably changed"), "{err}");
        // ...including one where the only links are the source tarballs.
        assert!(
            parse_nginx_catalogue(r#"<a href="/download/nginx-1.31.3.tar.gz">x</a>"#).is_err()
        );
    }

    #[test]
    fn the_nginx_archive_url_is_the_vendors_download_directory() {
        let release = NginxRelease {
            version: "1.31.3".into(),
            file_name: "nginx-1.31.3.zip".into(),
            channel: Channel::Mainline,
        };
        let download = release.download();
        assert_eq!(download.url, "https://nginx.org/download/nginx-1.31.3.zip");
        // nginx publishes no hash, and that has to be visible rather than
        // faked -- it is what decides whether the transfer can be verified.
        assert_eq!(download.sha256, None);
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
