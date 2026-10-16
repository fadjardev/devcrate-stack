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
//! **Composer** is the third shape. Its `/versions` index is JSON -- so the
//! catalogue is read, not scraped -- but carries no hash, unlike PHP's feed.
//! The hash lives next to each phar instead, in a `composer.phar.sha256sum`
//! sidecar fetched over the same TLS. So Composer's download *is* checksum-
//! verified, at full strength: the `Download::sha256` is `Some`, filled from
//! the sidecar rather than from the catalogue. The roadmap had guessed
//! `installer.sig` for this; that is the SHA-384 of the *setup script*, which
//! would need the PHP bootstrap to use, whereas the sidecar hashes the phar the
//! stack actually installs.
//!
//! **Adminer** and **Mailpit** are the fourth: GitHub releases, read from the
//! releases API, whose assets carry a `digest` -- the sha256 GitHub recorded
//! for the file the vendor uploaded. That is a real hash at full strength, the
//! same as PHP's, with GitHub rather than the vendor's own host as the place it
//! comes from; an API response without one falls back to the length check.
//!
//! **PECL** extensions (Xdebug) are the fifth: they come from the PHP Windows
//! team's build host, which publishes directory listings and no checksums --
//! so, like nginx, a download is length-checked over TLS and no more. What
//! makes a PECL DLL safe to install is matching the PHP build exactly, and PHP
//! itself refuses one that does not; see [`crate::php_ext`].
//!
//! **WordPress** is the sixth, and the one with the weakest hash: its
//! version-check API names the current release and the zip, and a `.sha1`
//! sidecar sits beside each zip -- SHA-1, not sha256, because that (and MD5)
//! is all WordPress publishes. The fetch itself is length-checked like
//! nginx's, then the file is hashed with SHA-1 against the sidecar; the
//! caller reports it as SHA-1, never as plain "verified". Not a runtime: it
//! is what `devcrate new wordpress` unpacks into a project.
//!
//! Nothing here prints. The catalogues and the fetch return structured results
//! and report progress through a callback, same as [`super::from_archive`],
//! so the dashboard can drive them without touching stdout.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sha1::Sha1;
use sha2::{Digest, Sha256};

/// Where the PHP catalogue and archives live. windows.php.net answers with a
/// redirect to downloads.php.net; ureq follows it.
const RELEASES_URL: &str = "https://windows.php.net/downloads/releases/";

/// nginx's download page, and the directory the zips it names actually sit in.
const NGINX_PAGE_URL: &str = "https://nginx.org/en/download.html";
const NGINX_DOWNLOAD_URL: &str = "https://nginx.org/download/";

/// Composer's machine-readable version index, and the directory each release's
/// phar and its checksum sidecar sit under (`<version>/composer.phar`).
const COMPOSER_VERSIONS_URL: &str = "https://getcomposer.org/versions";
const COMPOSER_DOWNLOAD_URL: &str = "https://getcomposer.org/download/";

/// Where EDB serves the Windows x64 *binaries* zips (the archive without the
/// installer), named `postgresql-<version>-<build>-windows-x64-binaries.zip`.
const POSTGRES_BINARIES_URL: &str = "https://get.enterprisedb.com/postgresql/";

/// python.org's release archive: `<release>/python-<release>-embed-amd64.zip`.
const PYTHON_FTP_URL: &str = "https://www.python.org/ftp/python/";

/// How far up a branch's patch numbers to look for the last one that still ships
/// an embeddable zip. Windows binaries stop partway up every branch (3.8 ends at
/// 3.8.10; later 3.8.x are source-only), so resolving a branch means finding the
/// highest patch that has a zip -- and no branch's binaries have ever reached
/// this many patches.
const PYTHON_PATCH_CEILING: u32 = 25;

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

/// One installable PostgreSQL release, resolved to the EDB zip that holds it.
///
/// A fourth vendor shape, and the weakest for verification. EDB publishes no
/// machine-readable index of these binaries zips and no checksums for them, so
/// there is nothing to *list* and nothing to hash against -- only a predictable
/// URL per release, checked (like nginx) against its declared length over TLS.
/// [`Download::sha256`] is `None`, and that is where the difference lives.
#[derive(Debug, Clone)]
pub struct PostgresRelease {
    /// `major.minor`, e.g. `13.23`. The name the stack knows it by.
    pub version: String,
    /// The zip EDB actually serves, incl. their packaging build number.
    pub file_name: String,
}

impl PostgresRelease {
    pub fn download(&self) -> Download {
        Download {
            url: format!("{POSTGRES_BINARIES_URL}{}", self.file_name),
            file_name: self.file_name.clone(),
            // EDB publishes no checksum; see the type docs.
            sha256: None,
        }
    }
}

/// Resolve a `major.minor` to the EDB binaries zip that exists for it.
///
/// The build number after the version (`-1`, occasionally `-2`) is EDB's own
/// packaging revision, not something the caller should have to know, so it is
/// discovered by asking the server which one is there rather than guessed. There
/// is no catalogue to enumerate -- EDB serves no index -- so this is the whole
/// of "which release": a named version either resolves to a URL that answers or
/// it does not.
pub fn postgres_release(version: &str) -> Result<PostgresRelease> {
    if !is_postgres_minor(version) {
        bail!(
            "{version:?} is not a PostgreSQL major.minor release (try 13.23).\n\
             EDB publishes no index to resolve a bare series, so an exact minor \
             is required."
        );
    }

    let mut tried = Vec::new();
    for build in 1..=3u32 {
        let file_name = format!("postgresql-{version}-{build}-windows-x64-binaries.zip");
        let url = format!("{POSTGRES_BINARIES_URL}{file_name}");
        if resource_exists(&url) {
            return Ok(PostgresRelease { version: version.to_string(), file_name });
        }
        tried.push(file_name);
    }

    bail!(
        "no EDB Windows x64 binaries zip found for PostgreSQL {version}.\n\
         Tried: {}\n\
         Name an exact minor that EDB still hosts, or download the zip yourself \
         and install it with --from (docs/installation.md).",
        tried.join(", ")
    )
}

/// Does a `HEAD` to this URL come back as present? Any error -- a 404, or a
/// connection that never opened -- reads as "no", which is exactly what the
/// caller wants: it is about to try the next candidate build, and the last one's
/// failure is not worth distinguishing from a missing file here.
fn resource_exists(url: &str) -> bool {
    agent().head(url).call().is_ok()
}

/// One installable Python release, resolved to the embeddable amd64 zip.
///
/// python.org's shape is nginx's, not PHP's: it publishes MD5 sums and GPG
/// signatures for these files but no sha256, so there is nothing that fits
/// [`Download::sha256`] to check against, and the transfer falls back to its
/// declared length over TLS to python.org. There is no machine-readable index of
/// which patch releases carry a Windows zip either, so a branch is resolved by
/// asking the archive which ones are there.
#[derive(Debug, Clone)]
pub struct PythonRelease {
    /// The branch, and the folder name the stack knows it by: `3.8`.
    pub version: String,
    /// The full release the zip holds: `3.8.10`.
    pub release: String,
    /// `python-3.8.10-embed-amd64.zip`.
    pub file_name: String,
}

impl PythonRelease {
    pub fn download(&self) -> Download {
        Download {
            url: format!("{PYTHON_FTP_URL}{}/{}", self.release, self.file_name),
            file_name: self.file_name.clone(),
            // python.org publishes MD5 and GPG, not sha256; see the type docs.
            sha256: None,
        }
    }
}

/// One installable MariaDB release.
#[derive(Debug, Clone)]
pub struct MariaDbRelease {
    pub version: String,
    pub file_name: String,
    pub sha256: Option<String>,
}

/// MariaDB's download CDN. `archive.mariadb.org` serves the same files, but
/// has been seen trickling at 13 KB/s -- two hours for one zip -- where this
/// answers at hundreds of KB/s. Each release directory carries a
/// `sha256sums.txt` beside the zip, on both hosts.
const MARIADB_DOWNLOAD_URL: &str = "https://dlm.mariadb.com/mariadb/";

impl MariaDbRelease {
    /// The zip, verified against the `sha256sums.txt` MariaDB publishes in the
    /// same release directory -- so a MariaDB download is checksum-verified,
    /// at full strength, the way Composer's is from its sidecar.
    pub fn download(&self) -> Result<Download> {
        let dir = format!("{MARIADB_DOWNLOAD_URL}mariadb-{}/winx64-packages/", self.version);
        let sums_url = format!("{dir}sha256sums.txt");
        let mut response = agent()
            .get(&sums_url)
            .call()
            .with_context(|| format!("fetching the MariaDB checksums from {sums_url}"))?;
        let sums = response
            .body_mut()
            .read_to_string()
            .with_context(|| format!("reading the MariaDB checksums from {sums_url}"))?;
        let sha256 = self.sha256.clone().or_else(|| sha256_for(&sums, &self.file_name)).ok_or_else(|| {
            anyhow::anyhow!("{sums_url} lists no sha256 for {}", self.file_name)
        })?;
        Ok(Download {
            url: format!("{dir}{}", self.file_name),
            file_name: self.file_name.clone(),
            sha256: Some(sha256),
        })
    }
}

/// The hash `sha256sums.txt` gives `file`: lines of `<64 hex>  ./<file>`.
pub fn sha256_for(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*').trim_start_matches("./");
        (name == file && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

/// MariaDB's REST API: the major series, then one series' releases.
const MARIADB_API_URL: &str = "https://downloads.mariadb.org/rest-api/mariadb/";

/// The newest release of every stable series that has not reached its end of
/// life, read from MariaDB's REST API -- the newest *long-term-support* series
/// first, because that is what `latest` installs: a rolling series (13.0)
/// is replaced within months, an LTS (12.3) is supported for years.
pub fn mariadb_catalogue() -> Result<Vec<MariaDbRelease>> {
    let majors = catalogue_text("mariadb", "the MariaDB release list", MARIADB_API_URL)?;
    let series = parse_mariadb_series(&majors, &today())?;
    let mut releases = Vec::new();
    for (id, _) in &series {
        let url = format!("{MARIADB_API_URL}{id}/");
        let Ok(text) = catalogue_text(&format!("mariadb-{id}"), &format!("the MariaDB {id} releases"), &url) else {
            continue;
        };
        if let Some(version) = parse_mariadb_newest(&text) {
            let file_name = format!("mariadb-{version}-winx64.zip");
            releases.push(MariaDbRelease { version, file_name, sha256: None });
        }
    }
    if releases.is_empty() {
        bail!("MariaDB's REST API listed no stable series with a Windows build");
    }
    Ok(releases)
}

/// The stable series not past their end of life, as `(id, lts)`: the newest
/// LTS first, then the rest newest first.
pub fn parse_mariadb_series(json_text: &str, today: &str) -> Result<Vec<(String, bool)>> {
    let json: serde_json::Value =
        serde_json::from_str(json_text).context("the MariaDB release list is not JSON")?;
    let majors = json
        .get("major_releases")
        .and_then(|m| m.as_array())
        .context("the MariaDB release list has no major_releases")?;
    let mut series: Vec<(String, bool)> = majors
        .iter()
        .filter(|m| m.get("release_status").and_then(|s| s.as_str()) == Some("Stable"))
        .filter(|m| m.get("release_eol_date").and_then(|d| d.as_str()).is_none_or(|eol| eol >= today))
        .filter_map(|m| {
            let id = m.get("release_id")?.as_str()?.to_string();
            let lts = m.get("release_support_type").and_then(|s| s.as_str()) == Some("Long Term Support");
            Some((id, lts))
        })
        .collect();
    series.sort_by_key(|(id, _)| std::cmp::Reverse(version_parts(id)));
    if let Some(at) = series.iter().position(|(_, lts)| *lts) {
        let lts = series.remove(at);
        series.insert(0, lts);
    }
    Ok(series)
}

/// The newest release of one series that has a Windows x64 zip.
pub fn parse_mariadb_newest(json_text: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(json_text).ok()?;
    json.get("releases")?
        .as_object()?
        .iter()
        .filter(|(version, release)| {
            let wanted = format!("mariadb-{version}-winx64.zip");
            release
                .get("files")
                .and_then(|f| f.as_array())
                .is_some_and(|files| files.iter().any(|f| f.get("file_name").and_then(|n| n.as_str()) == Some(&wanted)))
        })
        .map(|(version, _)| version.clone())
        .max_by(|a, b| version_parts(a).cmp(&version_parts(b)))
}

/// `12.3.3` -> [12, 3, 3], so 10.11 sorts above 10.6.
fn version_parts(version: &str) -> Vec<u32> {
    version.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Today's date, `YYYY-MM-DD`, in UTC: what an end-of-life date is compared to.
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    date_of(secs).0
}

/// Seconds since the epoch as (`YYYY-MM-DD`, `HH:MM`), UTC -- Howard Hinnant's
/// civil-from-days, so no date crate is needed for two strings.
fn date_of(secs: u64) -> (String, String) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (format!("{year:04}-{month:02}-{day:02}"), format!("{:02}:{:02}", rem / 3600, (rem % 3600) / 60))
}

/// One installable RabbitMQ release.
#[derive(Debug, Clone)]
pub struct RabbitMqRelease {
    pub version: String,
    pub file_name: String,
    pub sha256: Option<String>,
}

impl RabbitMqRelease {
    pub fn download(&self) -> Download {
        Download {
            url: format!(
                "https://github.com/rabbitmq/rabbitmq-server/releases/download/v{}/{}",
                self.version, self.file_name
            ),
            file_name: self.file_name.clone(),
            sha256: self.sha256.clone(),
        }
    }
}

/// RabbitMQ's GitHub releases, newest first, each with the sha256 GitHub
/// recorded for its Windows zip.
pub fn rabbitmq_catalogue() -> Result<Vec<RabbitMqRelease>> {
    let text = github_releases_n("rabbitmq/rabbitmq-server", 30)?;
    let mut found = parse_github_tagged(&text, "v", |v| format!("rabbitmq-server-windows-{v}.zip"))?;
    newest_first(&mut found);
    Ok(found
        .into_iter()
        .map(|r| RabbitMqRelease { version: r.version, file_name: r.file_name, sha256: r.sha256 })
        .collect())
}

/// Move the newest release of `series` to the front, where `latest` looks.
fn pinned_first(releases: &mut Vec<GithubRelease>, series: &str) {
    let prefix = format!("{series}.");
    if let Some(at) = releases.iter().position(|r| r.version.starts_with(&prefix)) {
        let pinned = releases.remove(at);
        releases.insert(0, pinned);
    }
}

fn newest_first(releases: &mut [GithubRelease]) {
    releases.sort_by_key(|r| std::cmp::Reverse(version_parts(&r.version)));
}

/// One installable Redis release, from `redis-windows/redis-windows` on
/// GitHub -- a community compile of the official Redis source for Windows.
/// There is no vendor-published checksum for these builds, so `sha256` is
/// `None`, the same shape as [`RabbitMqRelease`] and [`ErlangRelease`].
#[derive(Debug, Clone)]
pub struct RedisRelease {
    pub version: String,
    pub file_name: String,
    pub sha256: Option<String>,
}

impl RedisRelease {
    pub fn download(&self) -> Download {
        Download {
            url: format!(
                "https://github.com/redis-windows/redis-windows/releases/download/{}/{}",
                self.version, self.file_name
            ),
            file_name: self.file_name.clone(),
            sha256: self.sha256.clone(),
        }
    }
}

/// The redis-windows project's GitHub releases (Redis has no Windows build of
/// its own), newest first, with GitHub's sha256 of each zip.
pub fn redis_catalogue() -> Result<Vec<RedisRelease>> {
    let text = github_releases_n("redis-windows/redis-windows", 30)?;
    let mut found = parse_github_tagged(&text, "", |v| format!("Redis-{v}-Windows-x64-msys2.zip"))?;
    newest_first(&mut found);
    Ok(found
        .into_iter()
        .map(|r| RedisRelease { version: r.version, file_name: r.file_name, sha256: r.sha256 })
        .collect())
}

/// One installable Erlang/OTP release.
#[derive(Debug, Clone)]
pub struct ErlangRelease {
    pub version: String,
    pub file_name: String,
    pub sha256: Option<String>,
}

impl ErlangRelease {
    pub fn download(&self) -> Download {
        Download {
            url: format!(
                "https://github.com/erlang/otp/releases/download/OTP-{}/{}",
                self.version, self.file_name
            ),
            file_name: self.file_name.clone(),
            sha256: self.sha256.clone(),
        }
    }
}

fn embed_release(branch: &str, release: &str) -> PythonRelease {
    PythonRelease {
        version: branch.to_string(),
        release: release.to_string(),
        file_name: format!("python-{release}-embed-amd64.zip"),
    }
}

/// Resolve a Python branch (`3.8`) or exact release (`3.8.10`) to the embeddable
/// zip that exists for it.
///
/// A branch names no single file -- the stack switches by branch, but the URL
/// needs a patch level -- so the highest patch that still ships a Windows zip is
/// found by asking python.org's archive, newest first. An exact release skips
/// the search and is only checked to be there.
pub fn python_release(wanted: &str) -> Result<PythonRelease> {
    let parts: Vec<&str> = wanted.split('.').collect();
    let numeric =
        parts.iter().all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    if !numeric || parts.len() < 2 || parts.len() > 3 {
        bail!("{wanted:?} is not a Python version (try 3.8 or 3.8.10)");
    }
    let branch = format!("{}.{}", parts[0], parts[1]);

    // An exact release is one URL to confirm.
    if parts.len() == 3 {
        let release = embed_release(&branch, wanted);
        if resource_exists(&release.download().url) {
            return Ok(release);
        }
        bail!(
            "python.org has no embeddable amd64 zip for Python {wanted}.\n\
             Windows binaries stop partway up each branch; name one that has a \
             zip (3.8.10 is the last for 3.8), or install it with --from."
        );
    }

    // A branch: walk its patches down from the ceiling to the first that has a
    // zip, which is the newest Windows build of that branch.
    for patch in (0..=PYTHON_PATCH_CEILING).rev() {
        let release = embed_release(&branch, &format!("{branch}.{patch}"));
        if resource_exists(&release.download().url) {
            return Ok(release);
        }
    }
    bail!(
        "no embeddable amd64 zip found for Python {branch} on python.org \
         (tried .{PYTHON_PATCH_CEILING} down to .0).\n\
         Name an exact release, e.g. 3.8.10, or install it with --from."
    )
}

/// A PostgreSQL `major.minor`: two non-empty numeric parts, e.g. `13.23`.
fn is_postgres_minor(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 2
        && parts.iter().all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// The OTP series `latest` installs. RabbitMQ supports a range of OTP series
/// per release and publishes it only as prose, so the series the stack runs is
/// pinned here and raised on purpose; any other release is installed by naming
/// it.
pub const ERLANG_SERIES: &str = "27";

/// Erlang/OTP's GitHub releases (`OTP-27.3.4.19`), with GitHub's sha256 of each
/// Windows installer: the newest of [`ERLANG_SERIES`] first, then the rest
/// newest first.
pub fn erlang_catalogue() -> Result<Vec<ErlangRelease>> {
    let text = github_releases_n("erlang/otp", 50)?;
    let mut found = parse_github_tagged(&text, "OTP-", |v| format!("otp_win64_{v}.exe"))?;
    newest_first(&mut found);
    pinned_first(&mut found, ERLANG_SERIES);
    Ok(found
        .into_iter()
        .map(|r| ErlangRelease { version: r.version, file_name: r.file_name, sha256: r.sha256 })
        .collect())
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

/// For catalogues, checksums, and listings: small responses, so connecting
/// and waiting for the headers are both capped.
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .user_agent(concat!("devcrate/", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

/// For the archives themselves: only connecting is capped. ureq checks the
/// header timeout again while the body is read -- it counts from the start of
/// that phase -- so with [`agent`] any download taking longer than 30 seconds
/// failed with "timeout: receive response", however steadily it was arriving.
/// A whole-transfer cap long enough for a PHP zip on a slow line would be too
/// long to be worth anything, and the progress callback shows whether bytes
/// are moving.
fn download_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .user_agent(concat!("devcrate/", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

/// The releases windows.php.net offers, oldest branch first.
pub fn php_catalogue() -> Result<Vec<PhpRelease>> {
    let url = format!("{RELEASES_URL}releases.json");
    let json = catalogue_text("php", "the PHP release list", &url)?;
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
    let html = catalogue_text("nginx", "the nginx download page", NGINX_PAGE_URL)?;
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

/// Which line of Composer a release is on, as `/versions` groups them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerLine {
    /// The current release everyone should install.
    Stable,
    /// The long-term-support line (2.2.x), for hosts stuck on old PHP.
    Lts,
    /// Pre-release and development builds -- offered by name, never by default.
    Preview,
    Snapshot,
}

impl ComposerLine {
    pub fn label(self) -> &'static str {
        match self {
            ComposerLine::Stable => "stable",
            ComposerLine::Lts => "LTS",
            ComposerLine::Preview => "preview",
            ComposerLine::Snapshot => "snapshot",
        }
    }
}

/// One installable Composer release, as the version index lists it.
#[derive(Debug, Clone)]
pub struct ComposerRelease {
    /// The full version, and the name the download is cached under: `2.10.2`.
    pub version: String,
    /// The index's own path to the phar: `/download/2.10.2/composer.phar`.
    pub path: String,
    /// The lowest PHP the release runs on, as `min-php` encodes it (`70205`).
    /// Informational -- shown so a stack on old PHP is not surprised.
    pub min_php: Option<u32>,
    pub line: ComposerLine,
}

/// The Composer lines worth offering, each already newest-first.
///
/// Like PHP's and nginx's catalogues, this is only what the vendor currently
/// serves, not a history: `/versions` lists the *current* release of each
/// maintained line under `stable` (the 2.x stable and the 2.2 LTS today), so an
/// exact version resolves only while it is still one of those. An older release
/// installs from a downloaded phar with `--from`, the same offline path the
/// other runtimes fall back to.
#[derive(Debug, Clone)]
pub struct ComposerCatalogue {
    /// The `stable` array as the index gives it, newest first -- the current
    /// stable of each maintained line, which an exact version resolves against.
    pub stable: Vec<ComposerRelease>,
    pub lts: Option<ComposerRelease>,
    pub preview: Option<ComposerRelease>,
    pub snapshot: Option<ComposerRelease>,
}

impl ComposerCatalogue {
    /// The release `install composer` with no version installs.
    pub fn latest_stable(&self) -> Option<&ComposerRelease> {
        self.stable.first()
    }
}

/// The releases getcomposer.org offers.
pub fn composer_catalogue() -> Result<ComposerCatalogue> {
    let json = catalogue_text("composer", "the Composer version list", COMPOSER_VERSIONS_URL)?;
    parse_composer_catalogue(&json)
}

/// Read the lines out of `/versions`.
///
/// The index keys each line by name (`stable`, `2.2` for LTS, `preview`,
/// `snapshot`) to an array newest-first. `stable` is the one that must be
/// there; a document without it is not the index this understands. The rest are
/// taken if present and skipped if not -- a missing `preview` is not an error,
/// just a line nobody can ask for today.
fn parse_composer_catalogue(json: &str) -> Result<ComposerCatalogue> {
    let root: serde_json::Value =
        serde_json::from_str(json).context("the Composer version index is not valid JSON")?;

    let stable = read_line(&root, "stable", ComposerLine::Stable);
    if stable.is_empty() {
        bail!(
            "the Composer version index at {COMPOSER_VERSIONS_URL} lists no stable \
             releases; its format has probably changed"
        );
    }

    Ok(ComposerCatalogue {
        stable,
        lts: read_line(&root, "2.2", ComposerLine::Lts).into_iter().next(),
        preview: read_line(&root, "preview", ComposerLine::Preview).into_iter().next(),
        snapshot: read_line(&root, "snapshot", ComposerLine::Snapshot).into_iter().next(),
    })
}

/// Pull one keyed array of releases out of the index, in its own order.
fn read_line(root: &serde_json::Value, key: &str, line: ComposerLine) -> Vec<ComposerRelease> {
    root.get(key)
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let version = entry.get("version").and_then(|v| v.as_str())?;
            let path = entry.get("path").and_then(|v| v.as_str())?;
            Some(ComposerRelease {
                version: version.to_string(),
                path: path.to_string(),
                min_php: entry.get("min-php").and_then(|v| v.as_u64()).map(|n| n as u32),
                line,
            })
        })
        .collect()
}

impl ComposerRelease {
    /// The archive to fetch, with the sha256 filled from the release's sidecar.
    ///
    /// Unlike PHP's [`PhpRelease::download`] and nginx's, this makes a network
    /// call: the hash is not in the catalogue, so it is fetched from
    /// `<version>/composer.phar.sha256sum` here. The cached file carries the
    /// version (`composer-2.10.2.phar`) so two versions do not collide in
    /// `_downloads\`, even though every release's phar is named `composer.phar`
    /// at the vendor.
    pub fn download(&self) -> Result<Download> {
        let sha256 = fetch_composer_sha256(&self.version)?;
        Ok(Download {
            url: format!("https://getcomposer.org{}", self.path),
            file_name: format!("composer-{}.phar", self.version),
            sha256: Some(sha256),
        })
    }
}

/// Fetch and read the sha256 sidecar for one Composer version.
fn fetch_composer_sha256(version: &str) -> Result<String> {
    let url = format!("{COMPOSER_DOWNLOAD_URL}{version}/composer.phar.sha256sum");
    let mut response = agent()
        .get(&url)
        .call()
        .with_context(|| format!("fetching the Composer checksum from {url}"))?;
    let text = response
        .body_mut()
        .read_to_string()
        .with_context(|| format!("reading the Composer checksum from {url}"))?;
    parse_sha256sum(&text)
        .with_context(|| format!("the checksum sidecar at {url} was not in the expected form"))
}

/// The one field of a `sha256sum` line: `<64 hex>  composer.phar`.
fn parse_sha256sum(text: &str) -> Result<String> {
    let hash = text.split_whitespace().next().unwrap_or("");
    if hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(hash.to_ascii_lowercase())
    } else {
        bail!("expected a sha256 and a file name, got {text:?}")
    }
}

/// Where fetched catalogues are kept: `_downloads\catalogues\` of the stack the
/// command runs on. Unset (in tests), nothing is cached.
static CATALOGUE_CACHE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Catalogues that came from the cache because the vendor could not be
/// reached, as lines to show -- collected here so every catalogue need not
/// carry where it came from through its callers.
static OFFLINE_NOTES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Keep fetched catalogues under `dir`. Called once, by `main`, with the stack.
pub fn set_catalogue_cache(dir: PathBuf) {
    let _ = CATALOGUE_CACHE.set(dir);
}

/// The offline notes collected since the last call.
pub fn take_offline_notes() -> Vec<String> {
    OFFLINE_NOTES.lock().map(|mut notes| std::mem::take(&mut *notes)).unwrap_or_default()
}

/// A catalogue's text: fetched, and saved under `key` for next time; or, when
/// the vendor cannot be reached, the copy saved last time, with a note that
/// says so and how old it is. No copy and no network is the fetch's own error.
///
/// Only the lists are cached. What is downloaded from them is checked against
/// the same hashes as ever, so a stale list can name an older release but never
/// let an unchecked file through.
fn catalogue_text(key: &str, what: &str, url: &str) -> Result<String> {
    let fetched = agent()
        .get(url)
        .call()
        .with_context(|| format!("fetching {what} from {url}"))
        .and_then(|mut response| {
            response.body_mut().read_to_string().with_context(|| format!("reading {what} from {url}"))
        });
    let cached = CATALOGUE_CACHE.get().map(|dir| dir.join(format!("{key}.txt")));
    match (fetched, cached) {
        (Ok(text), Some(path)) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let part = path.with_extension("part");
            if std::fs::write(&part, &text).is_ok() {
                let _ = std::fs::rename(&part, &path);
            }
            Ok(text)
        }
        (Ok(text), None) => Ok(text),
        (Err(err), Some(path)) if path.is_file() => {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading the saved copy of {what}"))?;
            let saved = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| {
                    let (day, time) = date_of(d.as_secs());
                    format!("{day} {time} UTC")
                })
                .unwrap_or_else(|| "an unknown date".into());
            let reason = err.root_cause().to_string();
            if let Ok(mut notes) = OFFLINE_NOTES.lock() {
                notes.push(format!("offline: {what} could not be fetched ({reason}); using the copy saved {saved}"));
            }
            Ok(text)
        }
        (Err(err), _) => Err(err),
    }
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
        download_agent().get(url).call().with_context(|| format!("downloading {url}"))?;
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

/// WordPress's version-check API: the current release, its zip, and the
/// oldest PHP it supports.
const WORDPRESS_VERSION_CHECK_URL: &str = "https://api.wordpress.org/core/version-check/1.7/";

/// The current WordPress release, as the version-check API describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordPressRelease {
    pub version: String,
    /// The full zip: `https://downloads.wordpress.org/release/wordpress-7.1.3.zip`.
    pub url: String,
    /// The oldest PHP the release runs on: `7.4`.
    pub min_php: String,
}

impl WordPressRelease {
    /// No sha256 -- WordPress publishes none. [`wordpress_sha1`] is the check.
    pub fn download(&self) -> Download {
        Download {
            url: self.url.clone(),
            file_name: format!("wordpress-{}.zip", self.version),
            sha256: None,
        }
    }
}

/// Read the first offer of a version-check response, which is the current
/// release in the default locale.
pub fn parse_wordpress_release(json_text: &str) -> Result<WordPressRelease> {
    let json: serde_json::Value =
        serde_json::from_str(json_text).context("the WordPress version-check response is not JSON")?;
    let offer = json
        .get("offers")
        .and_then(|offers| offers.get(0))
        .context("the WordPress version-check response has no offers")?;
    let field = |name: &str| offer.get(name).and_then(|v| v.as_str()).map(str::to_string);
    let version = field("version").context("the WordPress offer names no version")?;
    let url = offer
        .get("packages")
        .and_then(|p| p.get("full"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| field("download"))
        .context("the WordPress offer names no download")?;
    if !url.starts_with("https://") || !url.ends_with(".zip") {
        bail!("the WordPress offer's download is not an https zip: {url}");
    }
    Ok(WordPressRelease { version, url, min_php: field("php_version").unwrap_or_default() })
}

pub fn wordpress_release() -> Result<WordPressRelease> {
    let url = WORDPRESS_VERSION_CHECK_URL;
    let mut response =
        agent().get(url).call().with_context(|| format!("fetching the WordPress release from {url}"))?;
    let text = response
        .body_mut()
        .read_to_string()
        .with_context(|| format!("reading the WordPress release from {url}"))?;
    parse_wordpress_release(&text)
}

/// The SHA-1 WordPress publishes beside a zip, in `<zip url>.sha1`.
pub fn wordpress_sha1(release: &WordPressRelease) -> Result<String> {
    let url = format!("{}.sha1", release.url);
    let mut response =
        agent().get(&url).call().with_context(|| format!("fetching the WordPress checksum from {url}"))?;
    let text = response
        .body_mut()
        .read_to_string()
        .with_context(|| format!("reading the WordPress checksum from {url}"))?;
    let hash = text.split_whitespace().next().unwrap_or("");
    if hash.len() == 40 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(hash.to_ascii_lowercase())
    } else {
        bail!("the checksum sidecar at {url} is not a SHA-1: {text:?}")
    }
}

/// Streaming SHA-1 of a file on disk, as lowercase hex. Only for what a vendor
/// publishes nothing stronger for -- WordPress.
pub fn sha1_of(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let mut hasher = Sha1::new();
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

/// One release asset off GitHub's releases API.
#[derive(Debug, Clone)]
pub struct GithubRelease {
    /// The tag without its `v`: `6.1.2`.
    pub version: String,
    pub file_name: String,
    pub url: String,
    /// Lowercase hex sha256 from the asset's `digest`, when the API has one.
    pub sha256: Option<String>,
}

impl GithubRelease {
    pub fn download(&self) -> Download {
        Download { url: self.url.clone(), file_name: self.file_name.clone(), sha256: self.sha256.clone() }
    }
}

/// The non-draft, non-prerelease releases in a releases-API response, newest
/// first as GitHub lists them, each with the one asset `asset` names for its
/// version. A release without that asset is skipped rather than guessed at.
pub fn parse_github_releases(
    json_text: &str,
    asset: impl Fn(&str) -> String,
) -> Result<Vec<GithubRelease>> {
    parse_github_tagged(json_text, "v", asset)
}

/// [`parse_github_releases`] for a project whose tags carry another prefix
/// than `v` (`OTP-27.3`) or none (`8.10.2`).
pub fn parse_github_tagged(
    json_text: &str,
    prefix: &str,
    asset: impl Fn(&str) -> String,
) -> Result<Vec<GithubRelease>> {
    let val: serde_json::Value = serde_json::from_str(json_text).context("parsing the releases json")?;
    let Some(items) = val.as_array() else {
        bail!("the releases json is not an array");
    };

    let mut releases = Vec::new();
    for item in items {
        let flag = |key: &str| item.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
        if flag("draft") || flag("prerelease") {
            continue;
        }
        let Some(tag) = item.get("tag_name").and_then(|v| v.as_str()) else { continue };
        let version = tag.strip_prefix(prefix).unwrap_or(tag).to_string();
        let wanted = asset(&version);
        let found = item
            .get("assets")
            .and_then(|a| a.as_array())
            .and_then(|assets| assets.iter().find(|a| a.get("name").and_then(|n| n.as_str()) == Some(&wanted)));
        let Some(found) = found else { continue };
        let Some(url) = found.get("browser_download_url").and_then(|u| u.as_str()) else { continue };
        let sha256 = found
            .get("digest")
            .and_then(|d| d.as_str())
            .and_then(|d| d.strip_prefix("sha256:"))
            .filter(|hex| hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()))
            .map(str::to_ascii_lowercase);
        releases.push(GithubRelease { version, file_name: wanted, url: url.to_string(), sha256 });
    }

    if releases.is_empty() {
        bail!("the releases json carried no release with the expected download");
    }
    Ok(releases)
}

fn github_releases(repo: &str) -> Result<String> {
    github_releases_n(repo, 10)
}

/// The newest `count` releases of a GitHub project -- more than ten for
/// projects that publish several series side by side (Erlang's 29, 28, 27).
fn github_releases_n(repo: &str, count: u32) -> Result<String> {
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page={count}");
    catalogue_text(&format!("github-{}", repo.replace('/', "-")), &format!("the {repo} releases"), &url)
}

/// mkcert's releases: the Windows x64 executable, `mkcert-v<version>-windows-amd64.exe`.
pub fn mkcert_catalogue() -> Result<Vec<GithubRelease>> {
    parse_github_releases(&github_releases("FiloSottile/mkcert")?, |v| format!("mkcert-v{v}-windows-amd64.exe"))
}

/// Adminer's releases: the full single-file build (every driver, every
/// language), `adminer-<version>.php`.
pub fn adminer_catalogue() -> Result<Vec<GithubRelease>> {
    parse_github_releases(&github_releases("vrana/adminer")?, |v| format!("adminer-{v}.php"))
}

/// PECL's Windows builds, one directory per extension and release:
/// `xdebug/3.5.3/php_xdebug-3.5.3-8.5-ts-vs17-x64.zip`. The PHP Windows team
/// builds and hosts them; xdebug.org's own downloads are the same DLLs.
const PECL_URL: &str = "https://downloads.php.net/~windows/pecl/releases/";

/// One PECL build that matches a PHP build exactly.
#[derive(Debug, Clone)]
pub struct PeclBuild {
    pub version: String,
    pub file_name: String,
    pub url: String,
}

impl PeclBuild {
    /// No hash: the PECL host publishes none, so -- like nginx -- the fetch is
    /// checked against the declared length over TLS, and says so.
    pub fn download(&self) -> Download {
        Download { url: self.url.clone(), file_name: self.file_name.clone(), sha256: None }
    }
}

fn pecl_listing(url: &str) -> Result<String> {
    let mut response = agent().get(url).call().with_context(|| format!("fetching {url}"))?;
    response.body_mut().read_to_string().with_context(|| format!("reading {url}"))
}

/// The stable releases in a PECL directory listing, newest first. Alphas,
/// betas, and release candidates are left out.
pub fn parse_pecl_versions(html: &str) -> Vec<String> {
    let mut versions: Vec<Vec<u32>> = html
        .split("href=\"")
        .skip(1)
        .filter_map(|rest| {
            let dir = rest.split('"').next()?.strip_suffix('/')?;
            dir.split('.').map(|part| part.parse::<u32>().ok()).collect::<Option<Vec<u32>>>()
        })
        .filter(|parts| parts.len() >= 2)
        .collect();
    versions.sort_by(|a, b| b.cmp(a));
    versions.dedup();
    versions
        .into_iter()
        .map(|parts| parts.iter().map(u32::to_string).collect::<Vec<_>>().join("."))
        .collect()
}

/// The zip in one release's listing built for `build` (`8.5-ts-vs17-x64`).
pub fn parse_pecl_build(html: &str, extension: &str, version: &str, build: &str) -> Option<String> {
    let wanted = format!("php_{extension}-{version}-{build}.zip").to_ascii_lowercase();
    html.split("href=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .find(|name| name.to_ascii_lowercase() == wanted)
        .map(str::to_string)
}

/// The newest stable release of `extension` with a build for `build`, or the
/// release named. Walks the releases newest first, one listing each, so an
/// old PHP costs a few requests more than a current one.
pub fn pecl_build(extension: &str, version: Option<&str>, build: &str) -> Result<Option<PeclBuild>> {
    let base = format!("{PECL_URL}{}/", extension.to_ascii_lowercase());
    let versions = match version {
        Some(version) => vec![version.to_string()],
        None => {
            let versions = parse_pecl_versions(&pecl_listing(&base)?);
            if versions.is_empty() {
                bail!("{base} lists no releases of {extension}");
            }
            versions
        }
    };
    for version in versions {
        let dir = format!("{base}{version}/");
        let Ok(listing) = pecl_listing(&dir) else { continue };
        if let Some(file_name) = parse_pecl_build(&listing, extension, &version, build) {
            let url = format!("{dir}{file_name}");
            return Ok(Some(PeclBuild { version, file_name, url }));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {

    #[test]
    fn mariadb_latest_is_the_newest_lts_and_eol_series_are_dropped() {
        let json = r#"{"major_releases": [
            {"release_id": "13.2", "release_status": "Preview", "release_support_type": null, "release_eol_date": null},
            {"release_id": "13.0", "release_status": "Stable", "release_support_type": "Rolling", "release_eol_date": null},
            {"release_id": "12.3", "release_status": "Stable", "release_support_type": "Long Term Support", "release_eol_date": "2029-06-12"},
            {"release_id": "11.8", "release_status": "Stable", "release_support_type": "Long Term Support", "release_eol_date": "2028-06-04"},
            {"release_id": "10.11", "release_status": "Stable", "release_support_type": "Long Term Support", "release_eol_date": "2028-02-16"},
            {"release_id": "10.6", "release_status": "Stable", "release_support_type": "Long Term Support", "release_eol_date": "2026-07-06"}
        ]}"#;
        let series = parse_mariadb_series(json, "2026-10-16").unwrap();
        let ids: Vec<&str> = series.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["12.3", "13.0", "11.8", "10.11"], "LTS first, preview and past-EOL gone");
    }

    #[test]
    fn a_mariadb_series_yields_its_newest_windows_zip() {
        let json = r#"{"releases": {
            "12.3.2": {"files": [{"file_name": "mariadb-12.3.2-winx64.zip"}]},
            "12.3.10": {"files": [{"file_name": "mariadb-12.3.10-winx64.zip"}]},
            "12.3.11": {"files": [{"file_name": "mariadb-12.3.11.tar.gz"}]}
        }}"#;
        assert_eq!(parse_mariadb_newest(json).as_deref(), Some("12.3.10"), "numeric order, Windows zip required");
    }

    fn release(tag: &str, asset: &str) -> String {
        format!(
            r#"{{"tag_name": "{tag}", "draft": false, "prerelease": false, "assets": [{{"name": "{asset}", "browser_download_url": "https://example.test/{asset}", "digest": "sha256:{}"}}]}}"#,
            "a".repeat(64)
        )
    }

    #[test]
    fn erlang_tags_lose_their_prefix_and_the_pinned_series_leads() {
        let json = format!(
            "[{}, {}, {}]",
            release("OTP-29.1.2", "otp_win64_29.1.2.exe"),
            release("OTP-27.3.4.19", "otp_win64_27.3.4.19.exe"),
            release("OTP-28.5.0.8", "otp_win64_28.5.0.8.exe")
        );
        let mut found = parse_github_tagged(&json, "OTP-", |v| format!("otp_win64_{v}.exe")).unwrap();
        newest_first(&mut found);
        pinned_first(&mut found, "27");
        let versions: Vec<&str> = found.iter().map(|r| r.version.as_str()).collect();
        assert_eq!(versions, ["27.3.4.19", "29.1.2", "28.5.0.8"]);
        assert!(found.iter().all(|r| r.sha256.is_some()), "GitHub's digest is kept");

        let redis = format!("[{}]", release("8.10.2", "Redis-8.10.2-Windows-x64-msys2.zip"));
        let found = parse_github_tagged(&redis, "", |v| format!("Redis-{v}-Windows-x64-msys2.zip")).unwrap();
        assert_eq!(found[0].version, "8.10.2");
    }

    #[test]
    fn dates_are_civil() {
        assert_eq!(date_of(0), ("1970-01-01".to_string(), "00:00".to_string()));
        assert_eq!(date_of(1_792_134_199), ("2026-10-16".to_string(), "07:03".to_string()));
        assert_eq!(date_of(951_782_400).0, "2000-02-29");
    }

    /// With the vendor unreachable, a list comes from the copy saved when it
    /// was last fetched, and a note says so; with no copy, the error stands.
    #[test]
    fn an_unreachable_catalogue_falls_back_to_its_saved_copy() {
        let dir = std::env::temp_dir().join(format!("devcrate-catalogues-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        set_catalogue_cache(dir.clone());
        let Some(cache) = CATALOGUE_CACHE.get() else { unreachable!() };
        let key = format!("test-{}", std::process::id());
        let path = cache.join(format!("{key}.txt"));
        // Port 9 (discard) is closed on a normal machine: refused at once.
        let url = "http://127.0.0.1:9/catalogue.json";

        assert!(catalogue_text(&key, "the test list", url).is_err(), "no copy yet");
        std::fs::write(&path, "[\"saved\"]").unwrap();
        let _ = take_offline_notes();
        assert_eq!(catalogue_text(&key, "the test list", url).unwrap(), "[\"saved\"]");
        let notes = take_offline_notes();
        assert!(notes.iter().any(|n| n.starts_with("offline: the test list could not be fetched") && n.contains("UTC")), "{notes:?}");
        let _ = std::fs::remove_file(&path);
    }
    use super::*;

    #[test]
    fn mariadb_checksums_are_found_by_file_name() {
        let sums = "5fd2ef7c2df182d86549c47c992e870f1b820137da700518e2a81b21e1b8870e  ./mariadb-11.4.5-winx64-debugsymbols.zip\n\
                    b7c11d38657f16b837e68199d73670510aadb78f42dfa5d5fdea31a7aab342e3  ./mariadb-11.4.5-winx64.zip\n";
        assert_eq!(
            sha256_for(sums, "mariadb-11.4.5-winx64.zip").as_deref(),
            Some("b7c11d38657f16b837e68199d73670510aadb78f42dfa5d5fdea31a7aab342e3")
        );
        assert_eq!(sha256_for(sums, "mariadb-11.4.5-winx64.msi"), None);
    }

    #[test]
    fn pecl_listings_give_stable_releases_newest_first() {
        let html = r#"<a href="?C=N;O=D">x</a><a href="/~windows/pecl/releases/">up</a>
            <a href="2.5.5/">2.5.5/</a><a href="3.4.0beta1/">b</a><a href="3.10.0/">c</a>
            <a href="3.5.3/">d</a><a href="3.3.0alpha3/">e</a>"#;
        assert_eq!(parse_pecl_versions(html), ["3.10.0", "3.5.3", "2.5.5"]);
    }

    #[test]
    fn a_pecl_build_must_match_exactly() {
        let html = r#"<a href="php_xdebug-3.5.3-8.5-nts-vs17-x64.zip">n</a>
            <a href="php_xdebug-3.5.3-8.5-ts-vs17-x86.zip">x</a>
            <a href="php_xdebug-3.5.3-8.5-ts-vs17-x64.zip">t</a>"#;
        assert_eq!(
            parse_pecl_build(html, "xdebug", "3.5.3", "8.5-ts-vs17-x64").as_deref(),
            Some("php_xdebug-3.5.3-8.5-ts-vs17-x64.zip")
        );
        assert_eq!(parse_pecl_build(html, "xdebug", "3.5.3", "8.4-ts-vs17-x64"), None);
    }

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

    /// Shaped like `/versions`: each line keyed to a newest-first array, with
    /// the LTS line under `2.2` and lines the stack does not offer (`2`, `1`)
    /// alongside.
    const VERSIONS: &str = r#"{
        "stable": [
            {"path": "/download/2.10.2/composer.phar", "version": "2.10.2", "min-php": 70205, "lts": false},
            {"path": "/download/2.10.1/composer.phar", "version": "2.10.1", "min-php": 70205},
            {"path": "/download/2.7.1/composer.phar",  "version": "2.7.1",  "min-php": 70205}
        ],
        "preview": [
            {"path": "/download/2.11.0-RC1/composer.phar", "version": "2.11.0-RC1", "min-php": 70205}
        ],
        "snapshot": [
            {"path": "/download/snapshot/composer.phar", "version": "snapshot"}
        ],
        "2.2": [
            {"path": "/download/2.2.29/composer.phar", "version": "2.2.29", "min-php": 50309, "lts": true}
        ],
        "2": [{"path": "/download/2.10.2/composer.phar", "version": "2.10.2", "min-php": 70205}],
        "1": [{"path": "/download/1.10.27/composer.phar", "version": "1.10.27", "min-php": 50302}]
    }"#;

    #[test]
    fn the_composer_catalogue_keeps_the_lines_it_offers() {
        let catalogue = parse_composer_catalogue(VERSIONS).unwrap();
        // Every stable entry the index carries, kept in its order.
        let stable: Vec<&str> = catalogue.stable.iter().map(|r| r.version.as_str()).collect();
        assert_eq!(stable, ["2.10.2", "2.10.1", "2.7.1"]);
        assert_eq!(catalogue.latest_stable().unwrap().version, "2.10.2");
        assert_eq!(catalogue.latest_stable().unwrap().min_php, Some(70205));

        // The LTS line comes from the `2.2` key, the dev lines from theirs.
        assert_eq!(catalogue.lts.as_ref().unwrap().version, "2.2.29");
        assert_eq!(catalogue.lts.as_ref().unwrap().line, ComposerLine::Lts);
        assert_eq!(catalogue.preview.as_ref().unwrap().version, "2.11.0-RC1");
        assert_eq!(catalogue.snapshot.as_ref().unwrap().version, "snapshot");
    }

    #[test]
    fn a_versions_document_without_a_stable_line_is_an_error() {
        let err = parse_composer_catalogue(r#"{"preview": []}"#).unwrap_err();
        assert!(err.to_string().contains("format has probably changed"), "{err}");
        assert!(parse_composer_catalogue("not json").is_err());
    }

    /// A line the index does not carry is absent, not an error -- there is just
    /// nothing to install by that name.
    #[test]
    fn a_missing_line_is_simply_none() {
        let catalogue =
            parse_composer_catalogue(r#"{"stable": [{"path": "/download/2.10.2/composer.phar", "version": "2.10.2"}]}"#)
                .unwrap();
        assert!(catalogue.lts.is_none());
        assert!(catalogue.preview.is_none());
        assert_eq!(catalogue.stable[0].min_php, None);
    }

    #[test]
    fn the_sha256_is_the_first_field_of_the_sidecar_line() {
        let hash = "5ee7125f8a30a34d246cefdc0bc85b8a783b28f2aec968994118512350d28027";
        assert_eq!(parse_sha256sum(&format!("{hash}  composer.phar")).unwrap(), hash);
        assert_eq!(parse_sha256sum(&format!("{hash}  composer.phar\n")).unwrap(), hash);
        // Whatever case it arrives in, kept lowercase like PHP's.
        assert_eq!(parse_sha256sum(&format!("{}  composer.phar", hash.to_uppercase())).unwrap(), hash);

        // Not a 404 page, an empty file, or a truncated hash.
        assert!(parse_sha256sum("<html>404</html>").is_err());
        assert!(parse_sha256sum("").is_err());
        assert!(parse_sha256sum("abc123  composer.phar").is_err());
    }

    #[test]
    fn a_postgres_minor_is_two_numeric_parts() {
        assert!(is_postgres_minor("13.23"));
        assert!(is_postgres_minor("13.0"));
        // A bare series has no minor to resolve, and EDB serves no index to
        // resolve it from -- so it is not accepted as a version.
        assert!(!is_postgres_minor("13"));
        assert!(!is_postgres_minor("13.23.1"));
        assert!(!is_postgres_minor("13.x"));
        assert!(!is_postgres_minor(""));
    }

    #[test]
    fn the_postgres_url_is_the_edb_binaries_zip() {
        let release = PostgresRelease {
            version: "13.23".into(),
            file_name: "postgresql-13.23-1-windows-x64-binaries.zip".into(),
        };
        let download = release.download();
        assert_eq!(
            download.url,
            "https://get.enterprisedb.com/postgresql/postgresql-13.23-1-windows-x64-binaries.zip"
        );
        // EDB publishes no hash, and that must be visible rather than faked.
        assert_eq!(download.sha256, None);
    }

    #[test]
    fn the_redis_url_is_the_github_release_asset() {
        let release = RedisRelease {
            version: "8.10.2".into(),
            file_name: "Redis-8.10.2-Windows-x64-msys2.zip".into(),
            sha256: None,
        };
        let download = release.download();
        assert_eq!(
            download.url,
            "https://github.com/redis-windows/redis-windows/releases/download/8.10.2/Redis-8.10.2-Windows-x64-msys2.zip"
        );
        // redis-windows publishes no hash, and that must be visible rather
        // than faked.
        assert_eq!(download.sha256, None);
    }

    #[test]
    fn the_python_url_is_the_embeddable_zip_under_its_release() {
        let release = embed_release("3.8", "3.8.10");
        assert_eq!(release.version, "3.8");
        assert_eq!(release.file_name, "python-3.8.10-embed-amd64.zip");
        let download = release.download();
        assert_eq!(
            download.url,
            "https://www.python.org/ftp/python/3.8.10/python-3.8.10-embed-amd64.zip"
        );
        // python.org publishes MD5 and GPG, not sha256 -- so this must be None,
        // not a hash faked to look verified.
        assert_eq!(download.sha256, None);
    }

    #[test]
    fn a_python_version_must_be_two_or_three_numeric_parts() {
        // These parse; whether the file exists is python.org's to answer.
        assert!(python_release("3.8.x").is_err());
        assert!(python_release("3").is_err());
        assert!(python_release("3.8.10.1").is_err());
        assert!(python_release("").is_err());
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

    #[test]
    fn test_parse_node_catalogue() {
        let json = r#"[
            {"version": "v22.11.0", "lts": "Jod", "files": ["win-x64-zip", "tar.gz"]},
            {"version": "v20.18.0", "lts": true, "files": ["win-x64-zip"]},
            {"version": "v0.1.0", "lts": false, "files": ["src"]}
        ]"#;
        let cat = parse_node_catalogue(json).unwrap();
        assert_eq!(cat.releases.len(), 2);
        assert_eq!(cat.releases[0].version, "22.11.0");
        assert_eq!(cat.releases[0].lts.as_deref(), Some("Jod"));
        assert_eq!(cat.releases[0].url, "https://nodejs.org/dist/v22.11.0/node-v22.11.0-win-x64.zip");
    }

    #[test]
    fn test_parse_bun_catalogue() {
        let json = r#"[
            {"tag_name": "bun-v1.2.2", "draft": false, "prerelease": false},
            {"tag_name": "bun-v1.2.0-canary.1", "draft": false, "prerelease": true}
        ]"#;
        let cat = parse_bun_catalogue(json).unwrap();
        assert_eq!(cat.releases.len(), 1);
        assert_eq!(cat.releases[0].version, "1.2.2");
        assert_eq!(cat.releases[0].url, "https://github.com/oven-sh/bun/releases/download/bun-v1.2.2/bun-windows-x64.zip");
    }
}

#[derive(Debug, Clone)]
pub struct NodeCatalogue {
    pub releases: Vec<NodeRelease>,
}

#[derive(Debug, Clone)]
pub struct NodeRelease {
    pub version: String,
    pub lts: Option<String>,
    pub url: String,
    pub archive_name: String,
}

pub fn parse_node_catalogue(json_text: &str) -> Result<NodeCatalogue> {
    let val: serde_json::Value = serde_json::from_str(json_text)
        .context("parsing nodejs index.json")?;
    let Some(arr) = val.as_array() else {
        bail!("nodejs index.json is not an array");
    };

    let mut releases = Vec::new();
    for item in arr {
        let version = match item.get("version").and_then(|v| v.as_str()) {
            Some(v) => v.trim_start_matches('v').to_string(),
            None => continue,
        };
        let files = item.get("files").and_then(|f| f.as_array());
        let is_win_zip =
            files.is_some_and(|arr| arr.iter().any(|f| f.as_str() == Some("win-x64-zip")));

        if !is_win_zip {
            continue;
        }

        let lts = item.get("lts").and_then(|l| {
            if l.is_string() {
                l.as_str().map(|s| s.to_string())
            } else if l.as_bool() == Some(true) {
                Some("LTS".to_string())
            } else {
                None
            }
        });

        let archive_name = format!("node-v{version}-win-x64.zip");
        let url = format!("https://nodejs.org/dist/v{version}/{archive_name}");

        releases.push(NodeRelease {
            version,
            lts,
            url,
            archive_name,
        });
    }

    if releases.is_empty() {
        bail!("nodejs index.json carried no win-x64-zip releases");
    }

    Ok(NodeCatalogue { releases })
}

pub fn fetch_node_catalogue() -> Result<NodeCatalogue> {
    let url = "https://nodejs.org/dist/index.json";
    let text = catalogue_text("node", "the Node.js release index", url)?;
    parse_node_catalogue(&text)
}

#[derive(Debug, Clone)]
pub struct BunCatalogue {
    pub releases: Vec<BunRelease>,
}

#[derive(Debug, Clone)]
pub struct BunRelease {
    pub version: String,
    pub url: String,
    pub archive_name: String,
}

pub fn parse_bun_catalogue(json_text: &str) -> Result<BunCatalogue> {
    let val: serde_json::Value = serde_json::from_str(json_text)
        .context("parsing bun releases json")?;
    let Some(arr) = val.as_array() else {
        bail!("bun releases json is not an array");
    };

    let mut releases = Vec::new();
    for item in arr {
        let tag = match item.get("tag_name").and_then(|v| v.as_str()) {
            Some(v) => v,
            None => continue,
        };
        if item.get("draft").and_then(|d| d.as_bool()).unwrap_or(false)
            || item.get("prerelease").and_then(|p| p.as_bool()).unwrap_or(false)
        {
            continue;
        }

        let version = tag.trim_start_matches("bun-v").trim_start_matches('v').to_string();
        let archive_name = "bun-windows-x64.zip".to_string();
        let url = format!("https://github.com/oven-sh/bun/releases/download/{tag}/bun-windows-x64.zip");

        releases.push(BunRelease {
            version,
            url,
            archive_name,
        });
    }

    if releases.is_empty() {
        bail!("bun releases json carried no releases");
    }

    Ok(BunCatalogue { releases })
}

pub fn fetch_bun_catalogue() -> Result<BunCatalogue> {
    let url = "https://api.github.com/repos/oven-sh/bun/releases";
    let text = catalogue_text("github-oven-sh-bun", "the Bun releases", url)?;
    parse_bun_catalogue(&text)
}

/// Mailpit's releases: the Windows x64 zip. The asset's name carries no
/// version, so it is cached under one that does -- two releases must not share
/// a file in `_downloads\`.
pub fn mailpit_catalogue() -> Result<Vec<GithubRelease>> {
    let releases =
        parse_github_releases(&github_releases("axllent/mailpit")?, |_| "mailpit-windows-amd64.zip".to_string())?;
    Ok(releases
        .into_iter()
        .map(|r| GithubRelease { file_name: format!("mailpit-{}-windows-amd64.zip", r.version), ..r })
        .collect())
}

#[cfg(test)]
mod github_tests {
    use super::*;

    /// Drafts and prereleases are skipped, a release without the wanted asset
    /// is skipped, and the asset's `digest` becomes the sha256 to verify with.
    #[test]
    fn github_releases_carry_the_asset_and_its_digest() {
        let json = r#"[
            {"tag_name": "v6.2.0-rc1", "prerelease": true, "draft": false, "assets": []},
            {"tag_name": "v6.1.2", "prerelease": false, "draft": false, "assets": [
                {"name": "adminer-6.1.2-en.php", "browser_download_url": "https://x/en.php", "digest": "sha256:00"},
                {"name": "adminer-6.1.2.php", "browser_download_url": "https://x/adminer-6.1.2.php",
                 "digest": "sha256:B32316156BD18FFA99C1A27617142C2E5AD72260E37F15DA4E9937A3DBB15F57"}
            ]},
            {"tag_name": "v6.1.1", "prerelease": false, "draft": false, "assets": [
                {"name": "adminer-6.1.1.php", "browser_download_url": "https://x/adminer-6.1.1.php"}
            ]},
            {"tag_name": "v6.0.0", "prerelease": false, "draft": false, "assets": []}
        ]"#;
        let releases = parse_github_releases(json, |v| format!("adminer-{v}.php")).unwrap();

        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].version, "6.1.2");
        assert_eq!(releases[0].url, "https://x/adminer-6.1.2.php");
        assert_eq!(
            releases[0].sha256.as_deref(),
            Some("b32316156bd18ffa99c1a27617142c2e5ad72260e37f15da4e9937a3dbb15f57")
        );
        assert_eq!(releases[1].sha256, None, "no digest is no hash, not a guess");

        assert!(parse_github_releases("[]", |v| format!("adminer-{v}.php")).is_err());
    }
}
