//! `devcrate install` -- putting a runtime into the stack root.
//!
//! The pipeline starts from a *local archive*. Downloading ([`download`]) is a
//! separate step that ends by handing this code a verified file on disk, so
//! everything genuinely hard about installing -- proving the archive holds the
//! build the stack needs, keeping the extraction inside the stack root,
//! generating the first-run config, and never leaving a half-written version
//! behind -- happens after the bytes have landed, and can be tested without a
//! network.
//!
//! Printing lives in [`install`]. [`from_archive`] returns a structured result
//! and reports progress through a callback, because the dashboard drives the
//! same function and must never write to the screen it just drew.

mod archive;
mod download;
mod php;

use std::io::{IsTerminal, Write};
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;

use crate::config::{self, Stack};
use crate::exit;

/// Runtimes `install` knows how to name.
///
/// Only PHP can be installed today. The rest are listed so that naming one gets
/// "not built yet, here is what does it", rather than "unknown runtime" -- the
/// difference between a missing feature and a typo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    Php,
}

/// Named in `devcrate install --help`, installable in the order issue #2 sets.
const PLANNED: [&str; 5] = ["nginx", "mariadb", "rabbitmq", "erlang", "composer"];

impl Runtime {
    pub fn parse(name: &str) -> Result<Runtime> {
        let name = name.trim().to_ascii_lowercase();
        match name.as_str() {
            "php" => Ok(Runtime::Php),
            other if PLANNED.contains(&other) => bail!(
                "installing {other} is not built yet -- PHP is the first runtime \
                 (docs/roadmap.md item 2).\n\
                 Unpack it by hand for now: docs/installation.md"
            ),
            other => bail!(
                "{other:?} is not a runtime devcrate manages.\nKnown: php, {}",
                PLANNED.join(", ")
            ),
        }
    }

    /// The name in a path or a command: `php`, as in `php\php-8.4`.
    pub fn as_str(self) -> &'static str {
        match self {
            Runtime::Php => "php",
        }
    }

    /// The name in a sentence: `PHP`.
    pub fn label(self) -> &'static str {
        match self {
            Runtime::Php => "PHP",
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
    /// major.minor -- the name the stack knows a version by.
    pub version: String,
    /// The full release the archive held, e.g. `8.4.3`.
    pub release: String,
    /// Root-relative install directory.
    pub dir: String,
    pub fastcgi_port: Option<u16>,
    pub files: usize,
    pub enabled_extensions: Vec<String>,
    pub missing_extensions: Vec<String>,
    /// An existing installation of the same version was replaced.
    pub replaced: bool,
    /// Whether the Visual C++ runtime was found. `None` when the check could
    /// not run at all.
    pub vcredist: Option<bool>,
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
    }
}

/// Where an install is assembled before it is put in place.
///
/// The leading dot is load-bearing. [`config`] discovers PHP versions by
/// scanning `php\` for directories whose name starts with `php`, so a directory
/// called `php-8.4` shows up in `devcrate status`, `devcrate php list`, and the
/// dashboard the instant it exists. Assembling under a name that cannot match
/// that scan is what keeps a half-extracted version from ever being offered as
/// an installed one.
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
    let replaced = swap_into_place(stack, &staging, &dest, &tag)?;

    let receipt = Receipt {
        runtime: Runtime::Php.as_str().to_string(),
        version: version.clone(),
        release: release.clone(),
        thread_safe: true,
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
        fastcgi_port: config::port_from_tag(&tag),
        files: assembled.files,
        enabled_extensions: assembled.ini.enabled,
        missing_extensions: assembled.ini.missing,
        replaced,
        vcredist: vcredist_present(),
    })
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
/// directory are as close to atomic as this gets on Windows.
fn swap_into_place(stack: &Stack, staging: &Path, dest: &Path, tag: &str) -> Result<bool> {
    let replaced = dest.exists();
    let retired = stack.php_dir.join(retired_name(tag));

    if replaced {
        if retired.exists() {
            std::fs::remove_dir_all(&retired)
                .with_context(|| format!("clearing {}", stack.rel(&retired)))?;
        }
        std::fs::rename(dest, &retired).with_context(|| {
            format!(
                "moving the existing {} aside (is a php-cgi.exe from it still \
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
    thread_safe: bool,
    source_archive: String,
    source_bytes: u64,
    source_sha256: String,
    files: usize,
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
    let mut reporter = Reporter::new();
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
    // Exhaustive on purpose: a second downloadable runtime has to decide what
    // its catalogue looks like here.
    match runtime {
        Runtime::Php => {}
    }

    println!("  fetching the release list from windows.php.net");
    let catalogue = download::php_catalogue()?;

    let Some(wanted) = version else {
        print_catalogue(stack, &catalogue);
        return Ok(exit::OK);
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

    // The refusal --force overrides comes before the transfer, not after
    // thirty megabytes of it.
    let dest = stack.php_dir.join(format!("php-{version}"));
    if dest.exists() && !force {
        bail!("{} already exists; pass --force to replace it", stack.rel(&dest));
    }

    let downloads = stack.root.join(DOWNLOADS_DIR);
    let mut reporter = Reporter::new();
    let fetched = download::fetch_php(release, &downloads, &mut |done, total| {
        reporter.report(Progress::Downloading { done, total })
    })?;
    reporter.finish();
    match fetched.cached {
        true => println!(
            "  already in {}, checksum still good -- nothing downloaded",
            stack.rel(&downloads)
        ),
        false => println!("  sha256 verified against the release list"),
    }

    let opts = Options { version: None, force };
    let mut reporter = Reporter::new();
    let done = from_archive(stack, runtime, &fetched.path, &opts, &mut |progress| {
        reporter.report(progress)
    })?;
    reporter.finish();

    print_installed(&done);
    Ok(exit::OK)
}

/// Where downloads land, and stay: the archive doubles as the offline
/// fallback for `--from`, and the whole directory is gitignored.
const DOWNLOADS_DIR: &str = "_downloads";

fn print_catalogue(stack: &Stack, catalogue: &[download::PhpRelease]) {
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

/// Progress on the way to the terminal.
///
/// A download is millions of bytes and an extraction thousands of entries, so
/// both are redrawn in place on a terminal and suppressed entirely otherwise
/// -- an install log redirected to a file does not want three thousand
/// progress lines, and `\r` is meaningless in one.
struct Reporter {
    interactive: bool,
    last: String,
    drawing: bool,
}

impl Reporter {
    fn new() -> Reporter {
        Reporter {
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
            Progress::Configuring => self.line("generating php.ini"),
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
    println!(
        "  php.ini generated from php.ini-development, {} extensions enabled",
        done.enabled_extensions.len()
    );
    if done.replaced {
        println!("  replaced the previous {} installation", done.version);
    }

    if !done.missing_extensions.is_empty() {
        println!();
        println!(
            "WARNING: php.ini-development had no line for: {}",
            done.missing_extensions.join(", ")
        );
        println!("         They are not enabled. Add them by hand if this version needs them.");
    }

    if done.vcredist == Some(false) {
        println!();
        println!("WARNING: the Visual C++ runtime (vcruntime140.dll) was not found.");
        println!("         php-cgi.exe exits with no output at all without it. Install");
        println!("         https://aka.ms/vs/17/release/vc_redist.x64.exe before starting.");
    }

    println!();
    let (label, prefix) = (done.runtime.label(), done.runtime.as_str());
    match done.fastcgi_port {
        Some(port) => println!(
            "{label} {} installed as {prefix}-{} (fastcgi {port})",
            done.release, done.version
        ),
        None => println!(
            "{label} {} installed as {prefix}-{} (no default FastCGI port for this \
             version; set one in devcrate.toml)",
            done.release, done.version
        ),
    }
    println!("  serve a site with it   devcrate site add myapp.test --php {}", done.version);
    println!("  make it the CLI PHP    devcrate php use {}", done.version);
    println!("  start its worker       devcrate start php-{}", done.version);
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

        let planned = Runtime::parse("nginx").unwrap_err().to_string();
        assert!(planned.contains("not built yet"), "{planned}");

        let typo = Runtime::parse("pph").unwrap_err().to_string();
        assert!(typo.contains("not a runtime devcrate manages"), "{typo}");
    }
}
