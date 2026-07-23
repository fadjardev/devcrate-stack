//! `devcrate install` -- putting a runtime into the stack root.
//!
//! The pipeline starts from a *local archive*. Downloading ([`download`]) is a
//! separate step that ends by handing this code a file on disk, so everything
//! genuinely hard about installing -- proving the archive holds the build the
//! stack needs, keeping the extraction inside the stack root, generating the
//! first-run config, and never leaving a half-written version behind -- happens
//! after the bytes have landed, and can be tested without a network.
//!
//! Two runtimes are installable, and they need opposite things once unpacked.
//! PHP is a self-contained version that has to be *configured* ([`php`]); nginx
//! is a bare build that needs the *prefix around it* to be startable
//! ([`nginx`]). What they share is everything between the archive and the
//! folder: the extraction guard, the staging directory, and the swap that keeps
//! a half-written version invisible. That shared middle is what lives here.
//!
//! Printing lives in [`install`]. [`from_archive`] returns a structured result
//! and reports progress through a callback, because the dashboard drives the
//! same function and must never write to the screen it just drew.

mod archive;
mod download;
mod nginx;
mod php;

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;

use crate::config::{self, Stack};
use crate::{exit, junction};

/// Runtimes `install` knows how to name.
///
/// The ones that cannot be installed yet are still listed, so that naming one
/// gets "not built yet, here is what does it" rather than "unknown runtime" --
/// the difference between a missing feature and a typo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    Php,
    Nginx,
}

/// Named in `devcrate install --help`, installable in the order issue #2 sets.
const PLANNED: [&str; 4] = ["mariadb", "rabbitmq", "erlang", "composer"];

impl Runtime {
    pub fn parse(name: &str) -> Result<Runtime> {
        let name = name.trim().to_ascii_lowercase();
        match name.as_str() {
            "php" => Ok(Runtime::Php),
            "nginx" => Ok(Runtime::Nginx),
            other if PLANNED.contains(&other) => bail!(
                "installing {other} is not built yet -- PHP and nginx are the \
                 runtimes that are (docs/roadmap.md item 2).\n\
                 Unpack it by hand for now: docs/installation.md"
            ),
            other => bail!(
                "{other:?} is not a runtime devcrate manages.\nKnown: php, nginx, {}",
                PLANNED.join(", ")
            ),
        }
    }

    /// The name in a path or a command: `php`, as in `php\php-8.4`.
    pub fn as_str(self) -> &'static str {
        match self {
            Runtime::Php => "php",
            Runtime::Nginx => "nginx",
        }
    }

    /// The name in a sentence: `PHP`.
    pub fn label(self) -> &'static str {
        match self {
            Runtime::Php => "PHP",
            Runtime::Nginx => "nginx",
        }
    }
}

#[derive(Debug, Default)]
pub struct Options {
    /// Overrides the version read from the archive's file name.
    pub version: Option<String>,
    /// Replace an existing installation of the same version.
    pub force: bool,
}

/// Where an install has got to. Reported through a callback so the dashboard
/// can render it and the CLI can print it, without either owning the pipeline.
#[derive(Debug, Clone, Copy)]
pub enum Progress {
    /// Bytes, not entries; `total` is the Content-Length when the server sent one.
    Downloading { done: u64, total: Option<u64> },
    Extracting { done: usize, total: usize },
    Validating,
    Configuring,
    Installing,
}

/// A runtime that is now in place.
#[derive(Debug)]
pub struct Installed {
    pub runtime: Runtime,
    /// The name the stack knows the version by, and so the folder name after
    /// the runtime's prefix: `8.4` for `php\php-8.4`, `1.31.3` for
    /// `nginx\nginx-1.31.3`.
    pub version: String,
    /// The full release the archive held, e.g. `8.4.3`. Equal to `version` for
    /// nginx, which has no branch a build could be filed under.
    pub release: String,
    /// Root-relative install directory.
    pub dir: String,
    pub files: usize,
    /// An existing installation of the same version was replaced.
    pub replaced: bool,
    pub details: Details,
}

/// What is worth saying about an install that is true of one runtime only.
///
/// The alternative -- one struct with every runtime's fields on it and most of
/// them empty -- makes the caller guess which apply. An enum makes the printer
/// match, and the compiler asks the same question of the next runtime.
#[derive(Debug)]
pub enum Details {
    Php(PhpInstalled),
    Nginx(NginxInstalled),
}

#[derive(Debug)]
pub struct PhpInstalled {
    pub fastcgi_port: Option<u16>,
    pub enabled_extensions: Vec<String>,
    pub missing_extensions: Vec<String>,
    /// Whether the Visual C++ runtime was found. `None` when the check could
    /// not run at all.
    pub vcredist: Option<bool>,
}

#[derive(Debug)]
pub struct NginxInstalled {
    /// Root-relative prefix the build landed inside.
    pub prefix: String,
    /// `nginx\current` now names this build.
    pub activated: bool,
    /// ...or it still names this one, which the install left alone.
    pub active_instead: Option<String>,
    /// What the prefix was missing and now is not.
    pub prepared: nginx::Prepared,
    /// What the new binary made of the stack's own configuration. Advisory.
    pub config_test: Option<nginx::ConfigTest>,
    /// nginx is running, so nothing here takes effect until it restarts.
    pub restart_needed: bool,
}

/// Install a runtime from an archive already on disk.
pub fn from_archive(
    stack: &Stack,
    runtime: Runtime,
    archive_path: &Path,
    opts: &Options,
    progress: &mut dyn FnMut(Progress),
) -> Result<Installed> {
    match runtime {
        Runtime::Php => php_from_archive(stack, archive_path, opts, progress),
        Runtime::Nginx => nginx_from_archive(stack, archive_path, opts, progress),
    }
}

/// Where an install is assembled before it is put in place.
///
/// The leading dot is load-bearing, and for both runtimes. [`config`] discovers
/// PHP versions by scanning `php\` for directories whose name starts with
/// `php`, and [`crate::root::nginx_versions`] scans the prefix for ones
/// starting with `nginx` -- so a directory called `php-8.4` or `nginx-1.31.3`
/// shows up in `devcrate status`, the version lists, and the dashboard the
/// instant it exists. Assembling under a name that cannot match either scan is
/// what keeps a half-extracted version from ever being offered as an installed
/// one.
fn staging_name(tag: &str) -> String {
    format!(".devcrate-staging-{tag}")
}

/// Where the previous installation waits while the new one is moved in, so a
/// failed rename can put it back.
fn retired_name(tag: &str) -> String {
    format!(".devcrate-replaced-{tag}")
}

fn php_from_archive(
    stack: &Stack,
    archive_path: &Path,
    opts: &Options,
    progress: &mut dyn FnMut(Progress),
) -> Result<Installed> {
    if !archive_path.is_file() {
        bail!("{} is not a file", archive_path.display());
    }
    let file_name =
        archive_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    // For the receipt. Hashed before any work starts, so a failure here costs
    // nothing -- and when the archive came through `fetch_php`, this is the
    // same hash that was just checked against the vendor's feed.
    let source_sha256 = download::sha256_of(archive_path)
        .with_context(|| format!("hashing {}", archive_path.display()))?;

    let (version, release) = resolve_version(&file_name, opts.version.as_deref())?;
    let tag = format!("php-{version}");
    let dest = stack.php_dir.join(&tag);

    if dest.exists() && !opts.force {
        bail!(
            "{} already exists; pass --force to replace it",
            stack.rel(&dest)
        );
    }

    std::fs::create_dir_all(&stack.php_dir)
        .with_context(|| format!("creating {}", stack.php_dir.display()))?;

    let staging = stack.php_dir.join(staging_name(&tag));
    // A previous run that died mid-extract leaves one of these behind.
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .with_context(|| format!("clearing {}", stack.rel(&staging)))?;
    }
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("creating {}", stack.rel(&staging)))?;

    // Everything from here until the swap is provisional: any failure clears
    // the staging directory rather than leaving it to be found later.
    let assembled = match assemble_php(&staging, archive_path, progress) {
        Ok(assembled) => assembled,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(err);
        }
    };

    progress(Progress::Installing);
    let replaced = swap_into_place(stack, &stack.php_dir, &staging, &dest, &tag)?;

    let receipt = Receipt {
        runtime: Runtime::Php.as_str().to_string(),
        version: version.clone(),
        release: release.clone(),
        thread_safe: Some(true),
        source_archive: file_name,
        source_bytes: std::fs::metadata(archive_path).map(|m| m.len()).unwrap_or(0),
        source_sha256,
        files: assembled.files,
    };
    write_receipt(&dest, &receipt)
        .with_context(|| format!("writing the install receipt in {}", stack.rel(&dest)))?;

    Ok(Installed {
        runtime: Runtime::Php,
        version,
        release,
        dir: stack.rel(&dest),
        files: assembled.files,
        replaced,
        details: Details::Php(PhpInstalled {
            fastcgi_port: config::port_from_tag(&tag),
            enabled_extensions: assembled.ini.enabled,
            missing_extensions: assembled.ini.missing,
            vcredist: vcredist_present(),
        }),
    })
}

/// Install nginx into the prefix, as one more versioned build beside any
/// others.
///
/// The shape is the PHP one -- resolve a version, stage, check, swap -- with
/// the configuration step pointed the other way: nothing about the *build* is
/// configured, and the prefix it lands in is made startable instead. Everything
/// after the swap is the part PHP has no equivalent of, because PHP versions
/// all run at once and exactly one nginx does.
fn nginx_from_archive(
    stack: &Stack,
    archive_path: &Path,
    opts: &Options,
    progress: &mut dyn FnMut(Progress),
) -> Result<Installed> {
    if !archive_path.is_file() {
        bail!("{} is not a file", archive_path.display());
    }
    let file_name =
        archive_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let source_sha256 = download::sha256_of(archive_path)
        .with_context(|| format!("hashing {}", archive_path.display()))?;

    let version = resolve_nginx_version(&file_name, opts.version.as_deref())?;
    let tag = format!("nginx-{version}");
    let prefix = nginx::install_prefix(&stack.nginx_prefix)?.to_path_buf();
    let dest = prefix.join(&tag);

    if dest.exists() && !opts.force {
        bail!("{} already exists; pass --force to replace it", stack.rel(&dest));
    }
    // Replacing the build a running nginx is executing from would fail at the
    // rename anyway; saying which command fixes it beats an access-denied.
    if let Some(pid) = crate::nginx::running(&dest).first() {
        bail!(
            "nginx {version} is running (pid {pid}) out of {}.\n\
             Stop it before replacing it: `devcrate stop nginx`.",
            stack.rel(&dest)
        );
    }

    std::fs::create_dir_all(&prefix)
        .with_context(|| format!("creating {}", stack.rel(&prefix)))?;

    let staging = prefix.join(staging_name(&tag));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .with_context(|| format!("clearing {}", stack.rel(&staging)))?;
    }
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("creating {}", stack.rel(&staging)))?;

    let assembled = match assemble_nginx(stack, &prefix, &staging, archive_path, progress) {
        Ok(assembled) => assembled,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(err);
        }
    };

    progress(Progress::Installing);
    let replaced = swap_into_place(stack, &prefix, &staging, &dest, &tag)?;

    let receipt = Receipt {
        runtime: Runtime::Nginx.as_str().to_string(),
        version: version.clone(),
        release: version.clone(),
        thread_safe: None,
        source_archive: file_name,
        source_bytes: std::fs::metadata(archive_path).map(|m| m.len()).unwrap_or(0),
        source_sha256,
        files: assembled.files,
    };
    write_receipt(&dest, &receipt)
        .with_context(|| format!("writing the install receipt in {}", stack.rel(&dest)))?;

    let (activated, active_instead) = activate_nginx(stack, &prefix, &dest)?;

    Ok(Installed {
        runtime: Runtime::Nginx,
        version: version.clone(),
        release: version,
        dir: stack.rel(&dest),
        files: assembled.files,
        replaced,
        details: Details::Nginx(NginxInstalled {
            prefix: stack.rel(&prefix),
            activated,
            active_instead,
            prepared: assembled.prepared,
            config_test: nginx::test_config(&dest.join("nginx.exe"), &prefix),
            restart_needed: !crate::nginx::running(&prefix).is_empty(),
        }),
    })
}

struct AssembledNginx {
    files: usize,
    prepared: nginx::Prepared,
}

fn assemble_nginx(
    stack: &Stack,
    prefix: &Path,
    staging: &Path,
    archive_path: &Path,
    progress: &mut dyn FnMut(Progress),
) -> Result<AssembledNginx> {
    // The zip wraps everything in `nginx-<version>\`; the extractor strips it,
    // which is why the build's own conf\ lands beside nginx.exe rather than a
    // level down.
    let extracted = archive::extract(archive_path, staging, &mut |done, total| {
        progress(Progress::Extracting { done, total })
    })?;

    progress(Progress::Validating);
    nginx::check(staging, archive_path)?;

    progress(Progress::Configuring);
    let prepared =
        nginx::prepare_prefix(prefix, &stack.root.join("projects"), |path| stack.rel(path))?;

    Ok(AssembledNginx { files: extracted.files, prepared })
}

/// Point `nginx\current` at the build just installed -- unless another version
/// already holds it.
///
/// Deliberately not the unconditional switch it might look like it should be.
/// Installing 1.30.4 on a stack that has been running 1.31.3 is a perfectly
/// ordinary thing to do (keeping the older stable one to hand), and silently
/// making it the version that starts next would be a downgrade nobody asked
/// for. So a fresh prefix activates -- there is nothing to disturb, and an
/// inactive lone build would do nothing at all -- and an occupied one is left
/// alone and reported, with `nginx use` there to take it.
///
/// Returns whether it activated, and the version holding `current` if it did
/// not.
fn activate_nginx(stack: &Stack, prefix: &Path, dest: &Path) -> Result<(bool, Option<String>)> {
    // Compared by folder name: both sides are versions inside this one prefix,
    // and a junction's target comes back canonicalized while `dest` is built
    // from the prefix as configured.
    let active = stack.current_nginx();
    let holder = active.as_deref().and_then(|dir| dir.file_name()).map(|n| n.to_owned());
    if let Some(holder) = holder
        && Some(holder.as_os_str()) != dest.file_name()
    {
        return Ok((false, Some(holder.to_string_lossy().into_owned())));
    }

    let current = prefix.join("current");
    junction::create(&current, dest).with_context(|| {
        format!("pointing {} at {}", stack.rel(&current), stack.rel(dest))
    })?;
    Ok((true, None))
}

/// Settle on the nginx version to install.
fn resolve_nginx_version(file_name: &str, wanted: Option<&str>) -> Result<String> {
    match wanted {
        Some(wanted) => {
            let wanted = crate::nginx::spelled(wanted);
            match nginx::version_from_file_name(&format!("nginx-{wanted}.zip")) {
                Some(version) => Ok(version),
                None => bail!("{wanted:?} does not name an nginx version (try 1.31.3)"),
            }
        }
        None => nginx::version_from_file_name(file_name).ok_or_else(|| {
            anyhow!(
                "cannot tell which nginx version {file_name:?} holds.\n\
                 The vendor's own name carries it (nginx-1.31.3.zip); \
                 pass --version 1.31.3 if the file has been renamed."
            )
        }),
    }
}

struct Assembled {
    files: usize,
    ini: php::Ini,
}

/// Unpack, check, and configure -- everything that happens inside the staging
/// directory, before anything of it is visible as an installed version.
fn assemble_php(
    staging: &Path,
    archive_path: &Path,
    progress: &mut dyn FnMut(Progress),
) -> Result<Assembled> {
    let extracted = archive::extract(archive_path, staging, &mut |done, total| {
        progress(Progress::Extracting { done, total })
    })?;

    progress(Progress::Validating);

    // Both, and for different reasons: php-cgi.exe is what the FastCGI workers
    // run, php.exe is what `devcrate php use` puts on PATH. An archive missing
    // either is not one this stack can use.
    for required in ["php.exe", "php-cgi.exe"] {
        if !staging.join(required).is_file() {
            bail!(
                "{} does not contain {required}; it does not look like a PHP \
                 distribution for Windows",
                archive_path.display()
            );
        }
    }

    if php::detect_build(staging) == php::Build::NonThreadSafe {
        bail!(
            "{} is a non-thread-safe (NTS) build.\n\
             The stack runs php-cgi.exe as a long-lived FastCGI listener with \
             PHP_FCGI_CHILDREN, which needs the thread-safe (TS) build -- see \
             docs/php-versions.md.\n\
             On windows.php.net the TS download is the one *without* `nts` in its name.",
            archive_path.display()
        );
    }

    progress(Progress::Configuring);
    let ini = php::write_ini(staging)?;

    Ok(Assembled { files: extracted.files, ini })
}

/// Move the staged installation into place, putting the old one back if the
/// move fails. Returns whether an existing installation was replaced.
///
/// Two renames rather than a delete-then-rename: a failed install must not be
/// able to leave the version directory missing altogether. Renames within one
/// directory are as close to atomic as this gets on Windows -- which is why
/// `dir` is passed rather than assumed: staging, retiring, and the destination
/// all have to be siblings for that to hold, whether the directory is `php\` or
/// the nginx prefix.
fn swap_into_place(
    stack: &Stack,
    dir: &Path,
    staging: &Path,
    dest: &Path,
    tag: &str,
) -> Result<bool> {
    let replaced = dest.exists();
    let retired = dir.join(retired_name(tag));

    if replaced {
        if retired.exists() {
            std::fs::remove_dir_all(&retired)
                .with_context(|| format!("clearing {}", stack.rel(&retired)))?;
        }
        std::fs::rename(dest, &retired).with_context(|| {
            format!(
                "moving the existing {} aside (is something from it still \
                 running? `devcrate stop {tag}`)",
                stack.rel(dest)
            )
        })?;
    }

    if let Err(err) = std::fs::rename(staging, dest) {
        if replaced {
            let _ = std::fs::rename(&retired, dest);
        }
        let _ = std::fs::remove_dir_all(staging);
        return Err(anyhow::Error::new(err)
            .context(format!("installing into {}", stack.rel(dest))));
    }

    if replaced {
        let _ = std::fs::remove_dir_all(&retired);
    }
    Ok(replaced)
}

/// What was installed, left in the version directory.
///
/// Informational only, and deliberately so: the stack discovers versions from
/// the folder name, never from this file, which is what keeps `devcrate.toml`
/// from having to be written and keeps unpacking a folder by hand a complete
/// way to install a version.
#[derive(Debug, Serialize)]
struct Receipt {
    runtime: String,
    version: String,
    release: String,
    source_archive: String,
    source_bytes: u64,
    source_sha256: String,
    files: usize,
    /// PHP only: nginx ships one Windows build and has no such distinction, so
    /// the key is absent rather than answered with a meaningless `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_safe: Option<bool>,
}

const RECEIPT_FILE: &str = ".devcrate-install.toml";

fn write_receipt(dir: &Path, receipt: &Receipt) -> Result<()> {
    let header = "# Written by `devcrate install`. Informational only -- the stack\n\
                  # discovers versions from the folder name, not from this file.\n\n";
    let text = header.to_string() + &toml::to_string_pretty(receipt)?;
    std::fs::write(dir.join(RECEIPT_FILE), text)?;
    Ok(())
}

/// Settle on the version to install, and the release it came from.
fn resolve_version(file_name: &str, wanted: Option<&str>) -> Result<(String, String)> {
    let from_name = php::version_from_file_name(file_name);

    match wanted {
        Some(wanted) => {
            let digits = crate::php::digits(wanted);
            if digits.len() < 2 {
                bail!("{wanted:?} does not name a PHP version (try 8.4)");
            }
            let version = config::version_from_tag(&digits);
            // The archive still knows its own patch level even when the caller
            // has overridden the major.minor.
            let release = from_name.map(|(_, release)| release).unwrap_or_else(|| version.clone());
            Ok((version, release))
        }
        None => from_name.ok_or_else(|| {
            anyhow!(
                "cannot tell which PHP version {file_name:?} holds.\n\
                 The vendor's own name carries it (php-8.4.3-Win32-vs17-x64.zip); \
                 pass --version 8.4 if the file has been renamed."
            )
        }),
    }
}

/// Is the Visual C++ runtime that PHP's Windows builds link against present?
///
/// A missing one is the single most common cause of `php-cgi.exe` exiting with
/// no output whatsoever, which is worth saying at install time rather than
/// leaving to be discovered when the stack will not start.
///
/// This looks for the DLL in the system directory, which is where the
/// redistributable puts it. A good-enough signal rather than an inventory: it
/// cannot report *which* version is installed, so it catches "nothing at all",
/// which is the case that actually happens.
#[cfg(windows)]
fn vcredist_present() -> Option<bool> {
    let system_root = std::env::var_os("SystemRoot")?;
    Some(Path::new(&system_root).join("System32").join("vcruntime140.dll").is_file())
}

#[cfg(not(windows))]
fn vcredist_present() -> Option<bool> {
    None
}

// ---------------------------------------------------------------------------
// The subcommand
// ---------------------------------------------------------------------------

pub fn install(
    stack: &Stack,
    runtime: &str,
    version: Option<&str>,
    from: Option<&Path>,
    force: bool,
) -> Result<u8> {
    let runtime = Runtime::parse(runtime)?;

    let Some(archive_path) = from else {
        return install_by_download(stack, runtime, version, force);
    };

    let opts = Options { version: version.map(str::to_string), force };
    let mut reporter = Reporter::new(runtime);
    let done = from_archive(stack, runtime, archive_path, &opts, &mut |progress| {
        reporter.report(progress)
    })?;
    reporter.finish();

    print_installed(&done);
    Ok(exit::OK)
}

/// Fetch the vendor's release list, download the wanted version into
/// `_downloads\`, and install it -- or, with no version named, print what is
/// available and stop.
fn install_by_download(
    stack: &Stack,
    runtime: Runtime,
    version: Option<&str>,
    force: bool,
) -> Result<u8> {
    // Each runtime answers the same two questions -- what can be had, and where
    // would this one land -- and everything after that is common. Exhaustive on
    // purpose: the next downloadable runtime has to answer them here too.
    let chosen = match runtime {
        Runtime::Php => choose_php(stack, version)?,
        Runtime::Nginx => choose_nginx(stack, version)?,
    };
    // Nothing chosen means the catalogue was printed instead.
    let Some(Chosen { download: source, dest }) = chosen else {
        return Ok(exit::OK);
    };

    // The refusal --force overrides comes before the transfer, not after
    // thirty megabytes of it.
    if dest.exists() && !force {
        bail!("{} already exists; pass --force to replace it", stack.rel(&dest));
    }

    let downloads = stack.root.join(DOWNLOADS_DIR);
    let mut reporter = Reporter::new(runtime);
    let fetched = download::fetch(&source, &downloads, &mut |done, total| {
        reporter.report(Progress::Downloading { done, total })
    })?;
    reporter.finish();
    report_transfer(stack, &downloads, &fetched);

    let opts = Options { version: None, force };
    let mut reporter = Reporter::new(runtime);
    let done = from_archive(stack, runtime, &fetched.path, &opts, &mut |progress| {
        reporter.report(progress)
    })?;
    reporter.finish();

    print_installed(&done);
    Ok(exit::OK)
}

/// A release picked out of a catalogue: where to get it, and where it goes.
struct Chosen {
    download: download::Download,
    dest: PathBuf,
}

fn choose_php(stack: &Stack, version: Option<&str>) -> Result<Option<Chosen>> {
    println!("  fetching the release list from windows.php.net");
    let catalogue = download::php_catalogue()?;

    let Some(wanted) = version else {
        print_php_catalogue(stack, &catalogue);
        return Ok(None);
    };

    let digits = crate::php::digits(wanted);
    if digits.len() < 2 {
        bail!("{wanted:?} does not name a PHP version (try 8.4)");
    }
    let version = config::version_from_tag(&digits);
    let Some(release) = catalogue.iter().find(|r| r.version == version) else {
        bail!(
            "PHP {version} is not on windows.php.net's release list.\n\
             Available: {}\n\
             An older build can still be installed with --from (docs/installation.md).",
            catalogue.iter().map(|r| r.version.as_str()).collect::<Vec<_>>().join(", ")
        );
    };

    Ok(Some(Chosen {
        download: release.download(),
        dest: stack.php_dir.join(format!("php-{version}")),
    }))
}

fn choose_nginx(stack: &Stack, version: Option<&str>) -> Result<Option<Chosen>> {
    // Refused before the network call rather than after it: a stack in the old
    // layout has nowhere to put a second build, and finding that out is not
    // worth a round trip.
    let prefix = nginx::install_prefix(&stack.nginx_prefix)?.to_path_buf();

    println!("  fetching the download page from nginx.org");
    let catalogue = download::nginx_catalogue()?;

    let Some(wanted) = version else {
        print_nginx_catalogue(stack, &prefix, &catalogue);
        return Ok(None);
    };

    let release = pick_nginx(&catalogue, wanted)?;
    Ok(Some(Chosen {
        download: release.download(),
        dest: prefix.join(format!("nginx-{}", release.version)),
    }))
}

/// The release a spelling names, by the same rule `devcrate nginx use` uses on
/// the versions already installed -- exact, or an unambiguous dotted prefix, so
/// `1.30` reaches the current stable release without naming its patch level.
fn pick_nginx<'a>(
    catalogue: &'a [download::NginxRelease],
    wanted: &str,
) -> Result<&'a download::NginxRelease> {
    let wanted = crate::nginx::spelled(wanted);
    if wanted.is_empty() {
        bail!("no nginx version named");
    }

    if let Some(exact) = catalogue.iter().find(|r| r.version == wanted) {
        return Ok(exact);
    }

    let matches: Vec<&download::NginxRelease> =
        catalogue.iter().filter(|r| crate::nginx::answers_to(&r.version, wanted)).collect();
    match matches.as_slice() {
        [one] => Ok(one),
        // The page lists one release per series, so this is a series that is
        // not on it rather than a patch level that has moved.
        [] => bail!(
            "nginx {wanted} is not on nginx.org's download page.\n\
             Available: {}\n\
             An older build can still be installed with --from (docs/installation.md).",
            catalogue.iter().map(|r| r.version.as_str()).collect::<Vec<_>>().join(", ")
        ),
        many => bail!(
            "{wanted:?} is ambiguous: {}",
            many.iter().map(|r| r.version.as_str()).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// Where downloads land, and stay: the archive doubles as the offline
/// fallback for `--from`, and the whole directory is gitignored.
const DOWNLOADS_DIR: &str = "_downloads";

/// What the transfer came to. Said out loud because the two vendors give
/// different amounts to go on, and which one you got is worth knowing.
fn report_transfer(stack: &Stack, downloads: &Path, fetched: &download::Fetched) {
    match (fetched.cached, fetched.verified) {
        (true, true) => println!(
            "  already in {}, checksum still good -- nothing downloaded",
            stack.rel(downloads)
        ),
        (true, false) => {
            println!("  already in {} -- nothing downloaded", stack.rel(downloads))
        }
        (false, true) => println!("  sha256 verified against the release list"),
        (false, false) => {
            println!("  sha256 {}", fetched.sha256);
            println!("  (nginx publishes no checksum; the transfer was checked");
            println!("   against its declared length over TLS to nginx.org)");
        }
    }
}

fn print_php_catalogue(stack: &Stack, catalogue: &[download::PhpRelease]) {
    println!();
    println!("PHP releases on windows.php.net (thread-safe x64):");
    println!();
    let width = catalogue.iter().map(|r| r.release.len()).max().unwrap_or(0);
    for release in catalogue {
        let tag = format!("php-{}", release.version);
        let installed =
            if stack.php_dir.join(&tag).is_dir() { "   installed" } else { "" };
        println!(
            "  {:<5}  {:<width$}  {:>8}{installed}",
            release.version, release.release, release.size
        );
    }
    println!();
    println!("Install one with   devcrate install php 8.4");
    println!("Only each branch's current release is offered; an older build");
    println!("installs from a downloaded archive with --from.");
}

fn print_nginx_catalogue(
    stack: &Stack,
    prefix: &Path,
    catalogue: &[download::NginxRelease],
) {
    println!();
    println!("nginx for Windows on nginx.org:");
    println!();
    let width = catalogue.iter().map(|r| r.version.len()).max().unwrap_or(0);
    for release in catalogue {
        let dir = prefix.join(format!("nginx-{}", release.version));
        let installed = if dir.is_dir() { "   installed" } else { "" };
        let row = format!(
            "  {:<width$}  {:<8}{installed}",
            release.version,
            release.channel.label()
        );
        println!("{}", row.trim_end());
    }
    println!();
    println!("Install one with   devcrate install nginx 1.31.3");
    println!("...or name a series: `1.30` takes the current stable release.");
    println!("Each build gets its own folder in {}.", stack.rel(prefix));
}

/// Progress on the way to the terminal.
///
/// A download is millions of bytes and an extraction thousands of entries, so
/// both are redrawn in place on a terminal and suppressed entirely otherwise
/// -- an install log redirected to a file does not want three thousand
/// progress lines, and `\r` is meaningless in one.
struct Reporter {
    runtime: Runtime,
    interactive: bool,
    last: String,
    drawing: bool,
}

impl Reporter {
    fn new(runtime: Runtime) -> Reporter {
        Reporter {
            runtime,
            interactive: std::io::stdout().is_terminal(),
            last: String::new(),
            drawing: false,
        }
    }

    fn report(&mut self, progress: Progress) {
        match progress {
            Progress::Downloading { done, total } => match total {
                Some(total) => self.redraw(format!(
                    "  downloading  {:>3}%  {} / {}",
                    done * 100 / total.max(1),
                    megabytes(done),
                    megabytes(total)
                )),
                None => self.redraw(format!("  downloading  {}", megabytes(done))),
            },
            Progress::Extracting { done, total } => {
                let percent = done * 100 / total.max(1);
                self.redraw(format!("  extracting   {percent:>3}%"));
            }
            Progress::Validating => self.line("checking the build"),
            // The one step that means genuinely different things: PHP's
            // configuration is the version's own, nginx's belongs to the
            // prefix it is about to sit in.
            Progress::Configuring => match self.runtime {
                Runtime::Php => self.line("generating php.ini"),
                Runtime::Nginx => self.line("preparing the prefix"),
            },
            Progress::Installing => self.line("moving it into place"),
        }
    }

    /// Repaint the in-place line, but only when its text has changed -- the
    /// download callback fires on every 64 KiB read.
    fn redraw(&mut self, text: String) {
        if !self.interactive || text == self.last {
            return;
        }
        self.last = text;
        self.drawing = true;
        print!("\r{}", self.last);
        let _ = std::io::stdout().flush();
    }

    fn line(&mut self, text: &str) {
        self.finish();
        println!("  {text}");
    }

    /// End the redrawn line, so whatever prints next starts on its own.
    fn finish(&mut self) {
        if self.drawing {
            println!();
            self.drawing = false;
        }
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

fn print_installed(done: &Installed) {
    println!();
    println!("  {} files into {}", done.files, done.dir);
    match &done.details {
        Details::Php(php) => print_php_installed(done, php),
        Details::Nginx(nginx) => print_nginx_installed(done, nginx),
    }
}

fn print_php_installed(done: &Installed, php: &PhpInstalled) {
    println!(
        "  php.ini generated from php.ini-development, {} extensions enabled",
        php.enabled_extensions.len()
    );
    if done.replaced {
        println!("  replaced the previous {} installation", done.version);
    }

    if !php.missing_extensions.is_empty() {
        println!();
        println!(
            "WARNING: php.ini-development had no line for: {}",
            php.missing_extensions.join(", ")
        );
        println!("         They are not enabled. Add them by hand if this version needs them.");
    }

    if php.vcredist == Some(false) {
        println!();
        println!("WARNING: the Visual C++ runtime (vcruntime140.dll) was not found.");
        println!("         php-cgi.exe exits with no output at all without it. Install");
        println!("         https://aka.ms/vs/17/release/vc_redist.x64.exe before starting.");
    }

    println!();
    let label = done.runtime.label();
    match php.fastcgi_port {
        Some(port) => println!(
            "{label} {} installed as php-{} (fastcgi {port})",
            done.release, done.version
        ),
        None => println!(
            "{label} {} installed as php-{} (no default FastCGI port for this \
             version; set one in devcrate.toml)",
            done.release, done.version
        ),
    }
    println!("  serve a site with it   devcrate site add myapp.test --php {}", done.version);
    println!("  make it the CLI PHP    devcrate php use {}", done.version);
    println!("  start its worker       devcrate start php-{}", done.version);
}

fn print_nginx_installed(done: &Installed, nginx: &NginxInstalled) {
    for created in &nginx.prepared.created {
        println!("  created {created}");
    }
    if nginx.activated {
        println!("  {}\\current -> {}", nginx.prefix, done.dir);
    }
    if done.replaced {
        println!("  replaced the previous {} installation", done.version);
    }

    if nginx.prepared.config_missing {
        println!();
        println!("WARNING: {}\\conf\\nginx.conf is not there.", nginx.prefix);
        println!("         It is tracked in the repository rather than generated, so a");
        println!("         missing one means the checkout is incomplete. nginx will not");
        println!("         start without it.");
    }

    // Only worth surfacing when it failed: a passing test says nothing that
    // the install succeeding does not already say.
    if let Some(test) = &nginx.config_test
        && !test.ok
    {
        println!();
        println!("WARNING: this build does not accept the current configuration:");
        println!("         {}", test.detail);
        println!("         On a stack with no certificates yet this is expected --");
        println!("         ssl_certificate names a file mkcert has not issued. Otherwise");
        println!("         it is a directive the new nginx no longer takes.");
    }

    println!();
    println!("{} {} installed as nginx-{}", done.runtime.label(), done.release, done.version);

    match (&nginx.active_instead, nginx.restart_needed) {
        (Some(active), _) => {
            println!("  {}\\current still names {active}, so this build is installed", nginx.prefix);
            println!("  but not the one that runs. To switch:");
            println!("    devcrate nginx use {}", done.version);
        }
        (None, true) => {
            println!("  nginx is running the previous build until it is restarted:");
            println!("    devcrate restart nginx");
        }
        (None, false) => {
            println!("  start it   devcrate start nginx");
            println!("  check it   devcrate nginx list");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The staging directory must not be discoverable as an installed version.
    /// `config::resolve_php` matches directories whose name starts with `php`,
    /// so this is the property that keeps a half-extracted install invisible.
    #[test]
    fn the_staging_directory_cannot_be_read_as_a_version() {
        let staging = staging_name("php-8.4");
        assert!(staging.starts_with('.'));
        assert!(!staging.starts_with("php"));

        let retired = retired_name("php-8.4");
        assert!(retired.starts_with('.'));
        assert!(!retired.starts_with("php"));

        // ...and they are different directories, or replacing a version would
        // move the old one on top of the new one.
        assert_ne!(staging, retired);
    }

    #[test]
    fn the_version_comes_from_the_archive_unless_it_is_overridden() {
        let name = "php-8.4.3-Win32-vs17-x64.zip";
        assert_eq!(
            resolve_version(name, None).unwrap(),
            ("8.4".to_string(), "8.4.3".to_string())
        );

        // An override names the folder; the archive still knows its release.
        assert_eq!(
            resolve_version(name, Some("8.5")).unwrap(),
            ("8.5".to_string(), "8.4.3".to_string())
        );
        // ...in any of the spellings the rest of the tool accepts.
        assert_eq!(resolve_version(name, Some("85")).unwrap().0, "8.5");
        assert_eq!(resolve_version(name, Some("php-8.5")).unwrap().0, "8.5");
    }

    /// A renamed archive with no version to read is refused rather than
    /// installed under a guessed name.
    #[test]
    fn an_unreadable_version_is_an_error_not_a_guess() {
        assert!(resolve_version("php.zip", None).is_err());
        assert!(resolve_version("php.zip", Some("8")).is_err());
        assert!(resolve_version("php.zip", Some("8.4")).is_ok());
    }

    /// Naming a runtime that is planned but not built has to read differently
    /// from naming one that does not exist.
    #[test]
    fn planned_runtimes_are_told_apart_from_typos() {
        assert_eq!(Runtime::parse("php").unwrap(), Runtime::Php);
        assert_eq!(Runtime::parse("PHP").unwrap(), Runtime::Php);
        assert_eq!(Runtime::parse("nginx").unwrap(), Runtime::Nginx);

        let planned = Runtime::parse("mariadb").unwrap_err().to_string();
        assert!(planned.contains("not built yet"), "{planned}");

        let typo = Runtime::parse("pph").unwrap_err().to_string();
        assert!(typo.contains("not a runtime devcrate manages"), "{typo}");
    }

    #[test]
    fn the_nginx_version_comes_from_the_archive_unless_it_is_overridden() {
        let name = "nginx-1.31.3.zip";
        assert_eq!(resolve_nginx_version(name, None).unwrap(), "1.31.3");

        // An override names the folder, in any spelling `nginx use` accepts.
        assert_eq!(resolve_nginx_version(name, Some("1.30.4")).unwrap(), "1.30.4");
        assert_eq!(resolve_nginx_version(name, Some("nginx-1.30.4")).unwrap(), "1.30.4");

        // A renamed archive with no version is refused rather than guessed.
        assert!(resolve_nginx_version("nginx.zip", None).is_err());
        assert!(resolve_nginx_version("nginx.zip", Some("latest")).is_err());
    }

    fn nginx_catalogue() -> Vec<download::NginxRelease> {
        [
            ("1.31.3", download::Channel::Mainline),
            ("1.30.4", download::Channel::Stable),
            ("1.28.3", download::Channel::Legacy),
        ]
        .into_iter()
        .map(|(version, channel)| download::NginxRelease {
            version: version.to_string(),
            file_name: format!("nginx-{version}.zip"),
            channel,
        })
        .collect()
    }

    /// Naming a series rather than a release is the useful shorthand here:
    /// nginx.org lists exactly one release per series, so `1.30` is
    /// unambiguous and saves knowing today's patch level.
    #[test]
    fn a_series_names_the_release_the_page_lists_for_it() {
        let catalogue = nginx_catalogue();
        assert_eq!(pick_nginx(&catalogue, "1.30").unwrap().version, "1.30.4");
        assert_eq!(pick_nginx(&catalogue, "1.30.4").unwrap().version, "1.30.4");
        assert_eq!(pick_nginx(&catalogue, "nginx-1.31.3").unwrap().version, "1.31.3");

        // ...and `1.3` is its own series, not a prefix of 1.30 or 1.31.
        let err = pick_nginx(&catalogue, "1.3").unwrap_err().to_string();
        assert!(err.contains("not on nginx.org's download page"), "{err}");
    }
}
