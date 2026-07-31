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

impl MariaDbRelease {
    pub fn download(&self) -> Download {
        Download {
            url: format!(
                "https://archive.mariadb.org/mariadb-{}/winx64-packages/{}",
                self.version, self.file_name
            ),
            file_name: self.file_name.clone(),
            sha256: self.sha256.clone(),
        }
    }
}

pub fn mariadb_catalogue() -> Result<Vec<MariaDbRelease>> {
    Ok(vec![
        MariaDbRelease {
            version: "11.4.5".to_string(),
            file_name: "mariadb-11.4.5-winx64.zip".to_string(),
            sha256: None,
        },
        MariaDbRelease {
            version: "10.11.11".to_string(),
            file_name: "mariadb-10.11.11-winx64.zip".to_string(),
            sha256: None,
        },
    ])
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

pub fn rabbitmq_catalogue() -> Result<Vec<RabbitMqRelease>> {
    Ok(vec![
        RabbitMqRelease {
            version: "4.0.5".to_string(),
            file_name: "rabbitmq-server-windows-4.0.5.zip".to_string(),
            sha256: None,
        },
        RabbitMqRelease {
            version: "4.0.0".to_string(),
            file_name: "rabbitmq-server-windows-4.0.0.zip".to_string(),
            sha256: None,
        },
    ])
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

pub fn erlang_catalogue() -> Result<Vec<ErlangRelease>> {
    Ok(vec![
        ErlangRelease {
            version: "27.2".to_string(),
            file_name: "otp_win64_27.2.exe".to_string(),
            sha256: None,
        },
        ErlangRelease {
            version: "26.2.5.5".to_string(),
            file_name: "otp_win64_26.2.5.5.exe".to_string(),
            sha256: None,
        },
    ])
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
    let mut response = agent()
        .get(COMPOSER_VERSIONS_URL)
        .call()
        .with_context(|| format!("fetching the Composer version list from {COMPOSER_VERSIONS_URL}"))?;
    let json = response
        .body_mut()
        .read_to_string()
        .with_context(|| format!("reading the Composer version list from {COMPOSER_VERSIONS_URL}"))?;
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
        let is_win_zip = files.map_or(false, |arr| {
            arr.iter().any(|f| f.as_str() == Some("win-x64-zip"))
        });

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
    let mut response = agent()
        .get(url)
        .call()
        .with_context(|| format!("fetching Node.js releases index from {url}"))?;
    let text = response
        .body_mut()
        .read_to_string()
        .with_context(|| format!("reading Node.js releases index from {url}"))?;
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
    let mut response = agent()
        .get(url)
        .call()
        .with_context(|| format!("fetching Bun releases from {url}"))?;
    let text = response
        .body_mut()
        .read_to_string()
        .with_context(|| format!("reading Bun releases from {url}"))?;
    parse_bun_catalogue(&text)
}
