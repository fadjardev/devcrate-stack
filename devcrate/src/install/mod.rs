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

mod adminer;
mod archive;
mod bun;
mod composer;
mod download;
mod erlang;
mod mariadb;
mod nginx;
mod node;
mod php;
mod postgres;
mod python;
mod rabbitmq;
mod redis;

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;

use crate::config::{self, Stack};
use crate::{exit, junction};

/// `println!`, but into an [`Emit`] instead of stdout. The CLI's [`TerminalEmit`]
/// prints exactly as `println!` did; the dashboard's [`NullEmit`] discards it,
/// because a background thread must never write to the screen ratatui is
/// drawing on the main thread -- see
/// docs/issue/tui-install-stdout-corruption-2026-09-19.md.
macro_rules! emit_line {
    ($emit:expr) => {
        $emit.line("")
    };
    ($emit:expr, $($arg:tt)*) => {
        $emit.line(&format!($($arg)*))
    };
}

/// Runtimes `install` knows how to name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    Php,
    Nginx,
    Composer,
    MariaDb,
    RabbitMq,
    Erlang,
    Node,
    Bun,
    Postgres,
    Python,
    Redis,
    Adminer,
}

impl Runtime {
    pub fn parse(name: &str) -> Result<Runtime> {
        let name = name.trim().to_ascii_lowercase();
        match name.as_str() {
            "php" => Ok(Runtime::Php),
            "nginx" => Ok(Runtime::Nginx),
            "composer" => Ok(Runtime::Composer),
            "mariadb" => Ok(Runtime::MariaDb),
            "rabbitmq" => Ok(Runtime::RabbitMq),
            "erlang" => Ok(Runtime::Erlang),
            "node" => Ok(Runtime::Node),
            "bun" => Ok(Runtime::Bun),
            "postgres" | "postgresql" => Ok(Runtime::Postgres),
            "python" => Ok(Runtime::Python),
            "redis" => Ok(Runtime::Redis),
            "adminer" => Ok(Runtime::Adminer),
            other => bail!(
                "{other:?} is not a runtime devcrate manages.\n\
                 Known: php, nginx, composer, mariadb, rabbitmq, erlang, node, \
                 bun, postgres, python, redis, adminer"
            ),
        }
    }

    /// The name in a path or a command: `php`, as in `php\php-8.4`.
    pub fn as_str(self) -> &'static str {
        match self {
            Runtime::Php => "php",
            Runtime::Nginx => "nginx",
            Runtime::Composer => "composer",
            Runtime::MariaDb => "mariadb",
            Runtime::RabbitMq => "rabbitmq",
            Runtime::Erlang => "erlang",
            Runtime::Node => "node",
            Runtime::Bun => "bun",
            Runtime::Postgres => "postgres",
            Runtime::Python => "python",
            Runtime::Redis => "redis",
            Runtime::Adminer => "adminer",
        }
    }

    /// The name in a sentence: `PHP`.
    pub fn label(self) -> &'static str {
        match self {
            Runtime::Php => "PHP",
            Runtime::Nginx => "nginx",
            Runtime::Composer => "Composer",
            Runtime::MariaDb => "MariaDB",
            Runtime::RabbitMq => "RabbitMQ",
            Runtime::Erlang => "Erlang/OTP",
            Runtime::Node => "Node.js",
            Runtime::Bun => "Bun",
            Runtime::Postgres => "PostgreSQL",
            Runtime::Python => "Python",
            Runtime::Redis => "Redis",
            Runtime::Adminer => "Adminer",
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
    Composer(ComposerInstalled),
    MariaDb(MariaDbInstalled),
    RabbitMq(RabbitMqInstalled),
    Erlang(ErlangInstalled),
    Node(NodeInstalledDetails),
    Bun(BunInstalledDetails),
    Postgres(PostgresInstalled),
    Python(PythonInstalled),
    Redis(RedisInstalled),
    Adminer(AdminerInstalled),
}

#[derive(Debug)]
pub struct NodeInstalledDetails {
    pub dir: String,
    pub global_dir: String,
}

#[derive(Debug)]
pub struct BunInstalledDetails {
    pub dir: String,
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

#[derive(Debug)]
pub struct PostgresInstalled {
    /// Root-relative data directory (`postgres\data`).
    pub data_dir: String,
    /// `initdb` ran now and created a fresh cluster.
    pub initialised: bool,
    /// An existing cluster was found and carried through the binary replace
    /// untouched. Mutually exclusive with `initialised`.
    pub preserved_data: bool,
    pub superuser: String,
    /// The password set on a freshly created cluster, printed so it is never a
    /// mystery. `None` when an existing cluster was kept.
    pub password: Option<String>,
    pub port: u16,
}

#[derive(Debug)]
pub struct PythonInstalled {
    /// Whether a `._pth` was edited to enable `import site` (false for a full
    /// distribution, which needs no such change).
    pub site_enabled: bool,
    /// What installing pip came to. `None` when it was not attempted at all
    /// (there was no `._pth`, so this is a full distribution with pip already).
    pub pip: Option<python::Pip>,
}

#[derive(Debug)]
pub struct ComposerInstalled {
    /// Root-relative `composer\` directory the phar and shims landed in.
    pub dir: String,
    /// Shim launchers written beside the phar (`composer.bat`, `composer`).
    pub shims: Vec<String>,
    /// `home\` / `cache\` directories that had to be created.
    pub created: Vec<String>,
    /// The version replaced, when a phar was already there. Composer is a single
    /// tool, so installing over one is an update, not the refusal a version gets.
    pub replaced_version: Option<String>,
    /// What running the phar under `php\current` said. Advisory, like nginx's
    /// `-t`; `None` when the stack names no current PHP to try it with.
    pub run: Option<composer::Run>,
}

#[derive(Debug)]
pub struct MariaDbInstalled {
    pub dir: String,
    pub config_created: bool,
    pub data_dir_created: bool,
}

#[derive(Debug)]
pub struct RabbitMqInstalled {
    pub dir: String,
    pub data_dir_created: bool,
    pub plugins_file_created: bool,
}

#[derive(Debug)]
pub struct RedisInstalled {
    pub dir: String,
    pub conf_created: bool,
    pub data_dir_created: bool,
    pub port: u16,
}

#[derive(Debug)]
pub struct AdminerInstalled {
    /// Root-relative `adminer\` directory: `adminer.php` and its `index.php`.
    pub dir: String,
    /// The same directory in full, for the `site add --path` hint -- a
    /// relative `--path` is taken from wherever the command runs.
    pub full_dir: String,
    /// The version replaced, when one was there -- an update, like Composer.
    pub replaced_version: Option<String>,
    /// Each installed PHP version, and which database drivers its php.ini
    /// enables -- what Adminer can reach when served by that version.
    pub drivers: Vec<(String, adminer::Drivers)>,
}

#[derive(Debug)]
pub struct ErlangInstalled {
    pub dir: String,
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
        Runtime::Composer => composer_from_phar(stack, archive_path, opts, progress),
        Runtime::MariaDb => mariadb_from_archive(stack, archive_path, opts, progress),
        Runtime::RabbitMq => rabbitmq_from_archive(stack, archive_path, opts, progress),
        Runtime::Erlang => erlang_from_archive(stack, archive_path, opts, progress),
        Runtime::Node => node_from_archive(stack, archive_path, opts, progress),
        Runtime::Bun => bun_from_archive(stack, archive_path, opts, progress),
        Runtime::Postgres => postgres_from_archive(stack, archive_path, opts, progress),
        Runtime::Python => python_from_archive(stack, archive_path, opts, progress),
        Runtime::Redis => redis_from_archive(stack, archive_path, opts, progress),
        Runtime::Adminer => adminer_from_file(stack, archive_path, opts, progress),
    }
}

fn mariadb_from_archive(
    stack: &Stack,
    archive_path: &Path,
    opts: &Options,
    progress: &mut dyn FnMut(Progress),
) -> Result<Installed> {
    if !archive_path.is_file() {
        bail!("{} is not a file", archive_path.display());
    }
    let file_name = archive_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let source_sha256 = download::sha256_of(archive_path)
        .with_context(|| format!("hashing {}", archive_path.display()))?;

    let version = opts
        .version
        .clone()
        .or_else(|| mariadb::version_from_file_name(&file_name))
        .unwrap_or_else(|| "11.4.5".to_string());

    let dest = stack.root.join("mariadb");

    if dest.exists() && !opts.force {
        bail!("{} already exists; pass --force to replace it", stack.rel(&dest));
    }

    let staging = stack.root.join(staging_name("mariadb"));
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    std::fs::create_dir_all(&staging)?;

    progress(Progress::Extracting { done: 0, total: 1 });
    let extracted = archive::extract(archive_path, &staging, &mut |done, total| {
        progress(Progress::Extracting { done, total });
    })?;

    progress(Progress::Validating);
    mariadb::check(&staging, archive_path)?;

    progress(Progress::Configuring);
    let configured = mariadb::configure(&staging)?;

    progress(Progress::Installing);
    let (replaced, kept) =
        swap_keeping(stack, &stack.root, &staging, &dest, "mariadb", &["data", "my.ini"])?;

    let receipt = Receipt {
        runtime: Runtime::MariaDb.as_str().to_string(),
        version: version.clone(),
        release: version.clone(),
        thread_safe: None,
        source_archive: file_name,
        source_bytes: std::fs::metadata(archive_path).map(|m| m.len()).unwrap_or(0),
        source_sha256,
        files: extracted.files,
    };
    write_receipt(&dest, &receipt)?;

    Ok(Installed {
        runtime: Runtime::MariaDb,
        version: version.clone(),
        release: version,
        dir: stack.rel(&dest),
        files: extracted.files,
        replaced,
        details: Details::MariaDb(MariaDbInstalled {
            dir: stack.rel(&dest),
            config_created: configured.config_created && !kept.iter().any(|k| k == "my.ini"),
            data_dir_created: configured.data_dir_created && !kept.iter().any(|k| k == "data"),
        }),
    })
}

fn rabbitmq_from_archive(
    stack: &Stack,
    archive_path: &Path,
    opts: &Options,
    progress: &mut dyn FnMut(Progress),
) -> Result<Installed> {
    if !archive_path.is_file() {
        bail!("{} is not a file", archive_path.display());
    }
    let file_name = archive_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let source_sha256 = download::sha256_of(archive_path)
        .with_context(|| format!("hashing {}", archive_path.display()))?;

    let version = opts
        .version
        .clone()
        .or_else(|| rabbitmq::version_from_file_name(&file_name))
        .unwrap_or_else(|| "4.0.5".to_string());

    let dest = stack.root.join("rabbitmq");

    if dest.exists() && !opts.force {
        bail!("{} already exists; pass --force to replace it", stack.rel(&dest));
    }

    let staging = stack.root.join(staging_name("rabbitmq"));
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    std::fs::create_dir_all(&staging)?;

    progress(Progress::Extracting { done: 0, total: 1 });
    let extracted = archive::extract(archive_path, &staging, &mut |done, total| {
        progress(Progress::Extracting { done, total });
    })?;

    progress(Progress::Validating);
    rabbitmq::check(&staging, archive_path)?;

    progress(Progress::Configuring);
    let configured = rabbitmq::configure(&staging)?;

    progress(Progress::Installing);
    let (replaced, kept) = swap_keeping(stack, &stack.root, &staging, &dest, "rabbitmq", &["data"])?;
    let kept_data = kept.iter().any(|k| k == "data");

    let receipt = Receipt {
        runtime: Runtime::RabbitMq.as_str().to_string(),
        version: version.clone(),
        release: version.clone(),
        thread_safe: None,
        source_archive: file_name,
        source_bytes: std::fs::metadata(archive_path).map(|m| m.len()).unwrap_or(0),
        source_sha256,
        files: extracted.files,
    };
    write_receipt(&dest, &receipt)?;

    Ok(Installed {
        runtime: Runtime::RabbitMq,
        version: version.clone(),
        release: version,
        dir: stack.rel(&dest),
        files: extracted.files,
        replaced,
        details: Details::RabbitMq(RabbitMqInstalled {
            dir: stack.rel(&dest),
            data_dir_created: configured.data_dir_created && !kept_data,
            plugins_file_created: configured.plugins_file_created && !kept_data,
        }),
    })
}

/// Install Redis into `redis\`, seeding a `redis.conf` and `data\` the same
/// way MariaDB's `my.ini` and `data\` are seeded -- see [`redis::configure`].
/// No PostgreSQL-style careful swap: there is no equivalent-of-`initdb` step.
/// [`swap_keeping`] carries `data\` and `devcrate.conf` across a reinstall, as
/// it does MariaDB's `data\` and `my.ini`.
fn redis_from_archive(
    stack: &Stack,
    archive_path: &Path,
    opts: &Options,
    progress: &mut dyn FnMut(Progress),
) -> Result<Installed> {
    if !archive_path.is_file() {
        bail!("{} is not a file", archive_path.display());
    }
    let file_name = archive_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let source_sha256 = download::sha256_of(archive_path)
        .with_context(|| format!("hashing {}", archive_path.display()))?;

    let version = opts
        .version
        .clone()
        .or_else(|| redis::version_from_file_name(&file_name))
        .unwrap_or_else(|| "8.10.2".to_string());

    let dest = stack.root.join("redis");
    let port = stack
        .by_kind(config::ServiceKind::Redis)
        .and_then(|s| s.ports.first().copied())
        .unwrap_or(6379);

    if dest.exists() && !opts.force {
        bail!("{} already exists; pass --force to replace it", stack.rel(&dest));
    }

    let staging = stack.root.join(staging_name("redis"));
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    std::fs::create_dir_all(&staging)?;

    progress(Progress::Extracting { done: 0, total: 1 });
    let extracted = archive::extract(archive_path, &staging, &mut |done, total| {
        progress(Progress::Extracting { done, total });
    })?;

    progress(Progress::Validating);
    redis::check(&staging, archive_path)?;

    progress(Progress::Configuring);
    let configured = redis::configure(&staging, port)?;

    progress(Progress::Installing);
    let (replaced, kept) =
        swap_keeping(stack, &stack.root, &staging, &dest, "redis", &["data", "devcrate.conf"])?;

    let receipt = Receipt {
        runtime: Runtime::Redis.as_str().to_string(),
        version: version.clone(),
        release: version.clone(),
        thread_safe: None,
        source_archive: file_name,
        source_bytes: std::fs::metadata(archive_path).map(|m| m.len()).unwrap_or(0),
        source_sha256,
        files: extracted.files,
    };
    write_receipt(&dest, &receipt)?;

    Ok(Installed {
        runtime: Runtime::Redis,
        version: version.clone(),
        release: version,
        dir: stack.rel(&dest),
        files: extracted.files,
        replaced,
        details: Details::Redis(RedisInstalled {
            dir: stack.rel(&dest),
            conf_created: configured.conf_created && !kept.iter().any(|k| k == "devcrate.conf"),
            data_dir_created: configured.data_dir_created && !kept.iter().any(|k| k == "data"),
            port,
        }),
    })
}

fn erlang_from_archive(
    stack: &Stack,
    archive_path: &Path,
    opts: &Options,
    progress: &mut dyn FnMut(Progress),
) -> Result<Installed> {
    if !archive_path.is_file() {
        bail!("{} is not a file", archive_path.display());
    }
    let file_name = archive_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let source_sha256 = download::sha256_of(archive_path)
        .with_context(|| format!("hashing {}", archive_path.display()))?;

    let version = opts
        .version
        .clone()
        .or_else(|| erlang::version_from_file_name(&file_name))
        .unwrap_or_else(|| "27.2".to_string());

    let dest = stack.root.join("erlang");

    if dest.exists() && !opts.force {
        bail!("{} already exists; pass --force to replace it", stack.rel(&dest));
    }

    let staging = stack.root.join(staging_name("erlang"));
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    std::fs::create_dir_all(&staging)?;

    let files_count = if file_name.ends_with(".exe") {
        progress(Progress::Installing);
        let status = std::process::Command::new(archive_path)
            .args(["/S", &format!("/D={}", staging.display())])
            .status()
            .with_context(|| format!("running silent installer {}", archive_path.display()))?;
        if !status.success() {
            bail!("installer {} failed with exit code {:?}", archive_path.display(), status.code());
        }
        100
    } else {
        progress(Progress::Extracting { done: 0, total: 1 });
        let extracted = archive::extract(archive_path, &staging, &mut |done, total| {
            progress(Progress::Extracting { done, total });
        })?;
        extracted.files
    };

    progress(Progress::Validating);
    erlang::check(&staging, archive_path)?;

    progress(Progress::Installing);
    let replaced = swap_into_place(stack, &stack.root, &staging, &dest, "erlang")?;

    let receipt = Receipt {
        runtime: Runtime::Erlang.as_str().to_string(),
        version: version.clone(),
        release: version.clone(),
        thread_safe: None,
        source_archive: file_name,
        source_bytes: std::fs::metadata(archive_path).map(|m| m.len()).unwrap_or(0),
        source_sha256,
        files: files_count,
    };
    write_receipt(&dest, &receipt)?;

    Ok(Installed {
        runtime: Runtime::Erlang,
        version: version.clone(),
        release: version,
        dir: stack.rel(&dest),
        files: files_count,
        replaced,
        details: Details::Erlang(ErlangInstalled {
            dir: stack.rel(&dest),
        }),
    })
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

/// Install PostgreSQL into `postgres\`, then create the cluster if there is not
/// one there already.
///
/// The one install that must protect what it replaces. PHP and nginx keep their
/// versions side by side, so replacing one cannot cost data; PostgreSQL keeps a
/// single `postgres\` directory with the cluster *inside* it, so the swap has to
/// carry `data\` across from the old binaries to the new ones rather than let it
/// go with the retired folder. [`swap_postgres`] is that careful move, and
/// [`postgres::initialise`] runs `initdb` only when the swap left no cluster
/// behind.
fn postgres_from_archive(
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

    let version = resolve_postgres_version(&file_name, opts.version.as_deref())?;
    let dest = stack.root.join("postgres");
    let port = stack
        .by_kind(config::ServiceKind::Postgres)
        .and_then(|s| s.ports.first().copied())
        .unwrap_or(5432);

    // Replacing the binaries a running server is executing from would fail at
    // the rename anyway; saying which command fixes it beats an access-denied,
    // and a live cluster is the last thing to swap under.
    let bin_exe = dest.join("bin").join("postgres.exe");
    if let Some(pid) = crate::probe::ProcessTable::scan().matching(&bin_exe, &[]).first() {
        bail!(
            "PostgreSQL is running (pid {pid}) out of {}.\n\
             Stop it before replacing it: `devcrate stop postgres`.",
            stack.rel(&dest)
        );
    }

    // The refusal comes before the extract, not after it. The existing cluster
    // in postgres\data is preserved either way -- --force replaces the binaries,
    // not the database.
    if dest.exists() && !opts.force {
        bail!(
            "{} already exists; pass --force to replace the binaries.\n\
             An existing cluster in {}\\data is kept, not overwritten.",
            stack.rel(&dest),
            stack.rel(&dest)
        );
    }

    let staging = stack.root.join(staging_name("postgres"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .with_context(|| format!("clearing {}", stack.rel(&staging)))?;
    }
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("creating {}", stack.rel(&staging)))?;

    let files = match assemble_postgres(&staging, archive_path, progress) {
        Ok(files) => files,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(err);
        }
    };

    progress(Progress::Installing);
    let swapped = swap_postgres(stack, &staging, &dest)?;

    // Only now, with the binaries in place, is there an initdb.exe to run -- and
    // it runs only if the swap did not carry an existing cluster across.
    progress(Progress::Configuring);
    let initialised = postgres::initialise(&dest, |p| stack.rel(p))?;

    let receipt = Receipt {
        runtime: Runtime::Postgres.as_str().to_string(),
        version: version.clone(),
        release: version.clone(),
        thread_safe: None,
        source_archive: file_name,
        source_bytes: std::fs::metadata(archive_path).map(|m| m.len()).unwrap_or(0),
        source_sha256,
        files,
    };
    write_receipt(&dest, &receipt)
        .with_context(|| format!("writing the install receipt in {}", stack.rel(&dest)))?;

    Ok(Installed {
        runtime: Runtime::Postgres,
        version: version.clone(),
        release: version,
        dir: stack.rel(&dest),
        files,
        replaced: swapped.replaced,
        details: Details::Postgres(PostgresInstalled {
            data_dir: initialised.data_dir,
            initialised: initialised.created,
            preserved_data: swapped.preserved_data,
            superuser: initialised.superuser.to_string(),
            password: initialised.password.map(str::to_string),
            port,
        }),
    })
}

/// Unpack and check a PostgreSQL archive in the staging directory. No configure
/// step: the cluster is created after the swap, by [`postgres::initialise`],
/// because there is no `initdb.exe` to run until the binaries are in place.
fn assemble_postgres(
    staging: &Path,
    archive_path: &Path,
    progress: &mut dyn FnMut(Progress),
) -> Result<usize> {
    // EDB wraps everything in `pgsql\`; the extractor strips that wrapper, so
    // bin\ and share\ land at the top of the staging directory.
    let extracted = archive::extract(archive_path, staging, &mut |done, total| {
        progress(Progress::Extracting { done, total })
    })?;

    progress(Progress::Validating);
    postgres::check(staging, archive_path)?;

    Ok(extracted.files)
}

/// The outcome of moving fresh PostgreSQL binaries into place over any that were
/// there, keeping the cluster.
struct SwappedPostgres {
    /// Binaries were already present and have been replaced.
    replaced: bool,
    /// An existing `data\` cluster was carried across to the new binaries.
    preserved_data: bool,
}

/// Move the staged binaries into `postgres\`, carrying an existing `data\`
/// cluster across from the old install to the new one.
///
/// The version runtimes' [`swap_into_place`] moves the whole directory aside and
/// the whole new one in, which for PostgreSQL would send the database off with
/// the retired binaries. So the cluster is lifted out of the retired folder and
/// set back under the new binaries before the retired one is removed -- and if
/// the new binaries fail to move in, the old folder (cluster and all) is put
/// straight back.
fn swap_postgres(stack: &Stack, staging: &Path, dest: &Path) -> Result<SwappedPostgres> {
    let replaced = dest.exists();
    let retired = stack.root.join(retired_name("postgres"));

    if replaced {
        if retired.exists() {
            std::fs::remove_dir_all(&retired)
                .with_context(|| format!("clearing {}", stack.rel(&retired)))?;
        }
        std::fs::rename(dest, &retired).with_context(|| {
            format!(
                "moving the existing {} aside (is something from it still \
                 running? `devcrate stop postgres`)",
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

    let mut preserved_data = false;
    if replaced {
        let old_data = retired.join("data");
        if old_data.join("PG_VERSION").is_file() {
            let new_data = dest.join("data");
            // The vendor archive carries no data\, but guard against a staging
            // dir that somehow had one rather than move onto it.
            if new_data.exists() {
                std::fs::remove_dir_all(&new_data)
                    .with_context(|| format!("clearing {}", stack.rel(&new_data)))?;
            }
            std::fs::rename(&old_data, &new_data).with_context(|| {
                format!("restoring the existing cluster into {}", stack.rel(&new_data))
            })?;
            preserved_data = true;
        }
        let _ = std::fs::remove_dir_all(&retired);
    }

    Ok(SwappedPostgres { replaced, preserved_data })
}

/// Settle on the PostgreSQL version to record.
///
/// The vendor's file name is authoritative; an override only stands in when the
/// file has been renamed past recognition. There is no versioned folder to name
/// (PostgreSQL is a single install), so this is for the receipt and the printout
/// rather than a path.
fn resolve_postgres_version(file_name: &str, wanted: Option<&str>) -> Result<String> {
    if let Some(version) = postgres::version_from_file_name(file_name) {
        return Ok(version);
    }
    if let Some(wanted) = wanted {
        let wanted = wanted.trim();
        if !wanted.is_empty() {
            return Ok(wanted.to_string());
        }
    }
    bail!(
        "cannot tell which PostgreSQL version {file_name:?} holds.\n\
         The vendor's own name carries it \
         (postgresql-13.16-1-windows-x64-binaries.zip); \
         pass --version 13.16 if the file has been renamed."
    )
}

/// Install Python into `python\python-<X.Y>`, enable `site`, and bootstrap pip.
///
/// The shape is PHP's -- versions side by side, a `current` junction switched
/// separately -- so the staging-and-swap is the shared [`swap_into_place`] and
/// there is nothing to protect on replace (no data, unlike PostgreSQL). What is
/// its own is the configure: the embeddable distribution ships with `site`
/// switched off, so [`python::enable_site`] turns it on, and pip is fetched and
/// installed afterwards -- the one step that needs the network, and the one that
/// degrades to a message rather than a failure when there is none.
fn python_from_archive(
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

    let (version, release) = resolve_python_version(&file_name, opts.version.as_deref())?;
    let tag = format!("python-{version}");
    let dest = stack.python_dir.join(&tag);

    if dest.exists() && !opts.force {
        bail!("{} already exists; pass --force to replace it", stack.rel(&dest));
    }

    std::fs::create_dir_all(&stack.python_dir)
        .with_context(|| format!("creating {}", stack.rel(&stack.python_dir)))?;

    let staging = stack.python_dir.join(staging_name(&tag));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .with_context(|| format!("clearing {}", stack.rel(&staging)))?;
    }
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("creating {}", stack.rel(&staging)))?;

    let assembled = match assemble_python(&staging, archive_path, progress) {
        Ok(assembled) => assembled,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(err);
        }
    };

    progress(Progress::Installing);
    let replaced = swap_into_place(stack, &stack.python_dir, &staging, &dest, &tag)?;

    // pip is the part that reaches the network, so it happens after the version
    // is in place and only when there is a `._pth` that turning site on made
    // pip-able. A full distribution (no `._pth`) already has pip.
    let pip = if assembled.site_enabled {
        Some(bootstrap_pip(stack, &dest, &version))
    } else {
        None
    };

    let receipt = Receipt {
        runtime: Runtime::Python.as_str().to_string(),
        version: version.clone(),
        release: release.clone(),
        thread_safe: None,
        source_archive: file_name,
        source_bytes: std::fs::metadata(archive_path).map(|m| m.len()).unwrap_or(0),
        source_sha256,
        files: assembled.files,
    };
    write_receipt(&dest, &receipt)
        .with_context(|| format!("writing the install receipt in {}", stack.rel(&dest)))?;

    Ok(Installed {
        runtime: Runtime::Python,
        version,
        release,
        dir: stack.rel(&dest),
        files: assembled.files,
        replaced,
        details: Details::Python(PythonInstalled {
            site_enabled: assembled.site_enabled,
            pip,
        }),
    })
}

struct AssembledPython {
    files: usize,
    site_enabled: bool,
}

/// Unpack, check, and enable `site` -- everything local, so a `--from` install
/// runs it with no network. pip, which needs one, is left to the caller.
fn assemble_python(
    staging: &Path,
    archive_path: &Path,
    progress: &mut dyn FnMut(Progress),
) -> Result<AssembledPython> {
    let extracted = archive::extract(archive_path, staging, &mut |done, total| {
        progress(Progress::Extracting { done, total })
    })?;

    progress(Progress::Validating);
    python::check(staging, archive_path)?;

    progress(Progress::Configuring);
    let site_enabled = python::enable_site(staging)?;

    Ok(AssembledPython { files: extracted.files, site_enabled })
}

/// Fetch the branch's `get-pip.py` and run it under the installed Python.
///
/// Every failure here is soft: no network, a 404 for a branch pypa no longer
/// hosts, or get-pip itself erroring all come back as [`python::Pip::Skipped`],
/// because the Python is installed and usable regardless -- pip is the one thing
/// that can be finished later with a single command.
fn bootstrap_pip(stack: &Stack, dest: &Path, branch: &str) -> python::Pip {
    let downloads = stack.root.join(DOWNLOADS_DIR);
    let source = download::Download {
        url: python::get_pip_url(branch),
        // Per branch, so 3.8's script and 3.12's do not collide in the cache.
        file_name: format!("get-pip-{branch}.py"),
        sha256: None,
    };
    let fetched = match download::fetch(&source, &downloads, &mut |_, _| {}) {
        Ok(fetched) => fetched,
        Err(err) => {
            return python::Pip::Skipped(format!("could not fetch get-pip.py ({err:#})"));
        }
    };
    python::run_get_pip(dest, &fetched.path)
}

/// Settle on the Python branch to install as, and the release it came from.
///
/// The branch (`3.8`) is the folder name, so an override names it; the archive
/// still knows its own release (`3.8.10`) for the receipt, exactly as PHP does.
fn resolve_python_version(file_name: &str, wanted: Option<&str>) -> Result<(String, String)> {
    let from_name = python::version_from_file_name(file_name);

    match wanted {
        Some(wanted) => {
            let digits = crate::php::digits(wanted);
            if digits.len() < 2 {
                bail!("{wanted:?} does not name a Python version (try 3.8)");
            }
            let branch = config::version_from_tag(&digits);
            let release = from_name.map(|(_, release)| release).unwrap_or_else(|| branch.clone());
            Ok((branch, release))
        }
        None => from_name.ok_or_else(|| {
            anyhow!(
                "cannot tell which Python version {file_name:?} holds.\n\
                 The vendor's own name carries it (python-3.8.10-embed-amd64.zip); \
                 pass --version 3.8 if the file has been renamed."
            )
        }),
    }
}

/// Install Composer: put the verified phar in `composer\`, write the shims that
/// launch it, and make the home/cache directories it keeps in the stack root.
///
/// The shortest install of the three, because Composer is a single file and not
/// a version. There is no archive to extract and nothing side by side to
/// disturb, so the staging dance the other two need collapses to one atomic
/// rename. It is also the only install that *updates* rather than refuses: a
/// phar already there is a Composer to replace, not a version to protect, so an
/// existing one is reported and overwritten instead of standing in the way.
fn composer_from_phar(
    stack: &Stack,
    phar_path: &Path,
    opts: &Options,
    progress: &mut dyn FnMut(Progress),
) -> Result<Installed> {
    if !phar_path.is_file() {
        bail!("{} is not a file", phar_path.display());
    }
    let file_name = phar_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let source_sha256 = download::sha256_of(phar_path)
        .with_context(|| format!("hashing {}", phar_path.display()))?;

    progress(Progress::Validating);
    composer::check(phar_path)?;

    let dir = stack.root.join("composer");
    let dest = dir.join("composer.phar");
    let php_exe = composer::current_php_exe(stack.current_php());

    // The version, in order of what actually knows it: an explicit override, the
    // download's own name (`composer-2.10.2.phar`), then asking the phar itself.
    // A bare `composer.phar` from an offline `--from`, on a stack with no PHP to
    // run it, is the one case left unnamed.
    let version = opts
        .version
        .clone()
        .or_else(|| composer::version_from_file_name(&file_name))
        .or_else(|| {
            php_exe
                .as_deref()
                .and_then(|exe| composer::probe(exe, phar_path))
                .filter(|run| run.ok)
                .and_then(|run| composer::version_from_output(&run.detail))
        });

    // What was there before, for the "updated X -> Y" line. Read from the
    // receipt so it costs nothing and needs no PHP; an unnamed previous install
    // is not worth reporting a replacement of.
    let existed = dest.is_file();
    let replaced_version = read_receipt_version(&dir).filter(|v| v != "unknown");

    progress(Progress::Configuring);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", stack.rel(&dir)))?;
    let shims = composer::write_shims(&dir, |p| stack.rel(p))?;
    let created = composer::ensure_home(&dir, |p| stack.rel(p))?;

    // One file, so "staging" is a sibling `.part` renamed over the destination
    // -- the same atomic-rename trust the download already leans on, with no
    // directory to swap.
    progress(Progress::Installing);
    let staged = dir.join(".devcrate-composer.phar.part");
    std::fs::copy(phar_path, &staged)
        .with_context(|| format!("staging the phar in {}", stack.rel(&dir)))?;
    if let Err(err) = std::fs::rename(&staged, &dest) {
        let _ = std::fs::remove_file(&staged);
        return Err(anyhow::Error::new(err).context(format!("installing {}", stack.rel(&dest))));
    }

    let known = version.unwrap_or_else(|| "unknown".to_string());
    let receipt = Receipt {
        runtime: Runtime::Composer.as_str().to_string(),
        version: known.clone(),
        release: known.clone(),
        thread_safe: None,
        source_archive: file_name,
        source_bytes: std::fs::metadata(phar_path).map(|m| m.len()).unwrap_or(0),
        source_sha256,
        files: 1,
    };
    // The receipt lives beside the phar, not in a version folder of its own.
    write_receipt(&dir, &receipt)
        .with_context(|| format!("writing the install receipt in {}", stack.rel(&dir)))?;

    // The real "does this PHP run it" answer, from the installed phar.
    let run = php_exe.as_deref().and_then(|exe| composer::probe(exe, &dest));

    Ok(Installed {
        runtime: Runtime::Composer,
        version: known.clone(),
        release: known,
        dir: stack.rel(&dir),
        files: 1,
        replaced: existed,
        details: Details::Composer(ComposerInstalled {
            dir: stack.rel(&dir),
            shims: shims.written,
            created,
            replaced_version,
            run,
        }),
    })
}

/// Install Adminer: the verified file as `adminer\adminer.php`, beside the
/// `index.php` devcrate serves it through. Like Composer, a tool -- installing
/// over one updates it -- and like Composer the single file goes in by an
/// atomic rename rather than a staged directory.
fn adminer_from_file(
    stack: &Stack,
    file_path: &Path,
    opts: &Options,
    progress: &mut dyn FnMut(Progress),
) -> Result<Installed> {
    if !file_path.is_file() {
        bail!("{} is not a file", file_path.display());
    }
    let file_name = file_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let source_sha256 = download::sha256_of(file_path)
        .with_context(|| format!("hashing {}", file_path.display()))?;

    progress(Progress::Validating);
    adminer::check(file_path)?;

    let dir = stack.root.join("adminer");
    let dest = dir.join("adminer.php");
    let version = opts
        .version
        .clone()
        .or_else(|| adminer::version_from_file_name(&file_name))
        .unwrap_or_else(|| "unknown".to_string());
    let existed = dest.is_file();
    let replaced_version = read_receipt_version(&dir).filter(|v| v != "unknown");

    progress(Progress::Configuring);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", stack.rel(&dir)))?;
    adminer::write_wrapper(&dir)?;

    progress(Progress::Installing);
    let staged = dir.join(".devcrate-adminer.php.part");
    std::fs::copy(file_path, &staged)
        .with_context(|| format!("staging Adminer in {}", stack.rel(&dir)))?;
    if let Err(err) = std::fs::rename(&staged, &dest) {
        let _ = std::fs::remove_file(&staged);
        return Err(anyhow::Error::new(err).context(format!("installing {}", stack.rel(&dest))));
    }

    let receipt = Receipt {
        runtime: Runtime::Adminer.as_str().to_string(),
        version: version.clone(),
        release: version.clone(),
        thread_safe: None,
        source_archive: file_name,
        source_bytes: std::fs::metadata(file_path).map(|m| m.len()).unwrap_or(0),
        source_sha256,
        files: 2,
    };
    write_receipt(&dir, &receipt)
        .with_context(|| format!("writing the install receipt in {}", stack.rel(&dir)))?;

    let drivers = stack
        .php_services()
        .filter(|php| php.is_installed())
        .filter_map(|php| {
            let ini = php.install_marker.parent()?.join("php.ini");
            let text = std::fs::read_to_string(ini).unwrap_or_default();
            Some((php.id.clone(), adminer::drivers(&text)))
        })
        .collect();

    Ok(Installed {
        runtime: Runtime::Adminer,
        version: version.clone(),
        release: version,
        dir: stack.rel(&dir),
        files: 2,
        replaced: existed,
        details: Details::Adminer(AdminerInstalled {
            dir: stack.rel(&dir),
            full_dir: dir.display().to_string(),
            replaced_version,
            drivers,
        }),
    })
}

/// The version an install receipt records, if one is there.
fn read_receipt_version(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(RECEIPT_FILE)).ok()?;
    let value: toml::Value = toml::from_str(&text).ok()?;
    value.get("version")?.as_str().map(str::to_string)
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
    swap_keeping(stack, dir, staging, dest, tag, &[]).map(|(replaced, _)| replaced)
}

/// [`swap_into_place`] for a service that keeps live state beside its
/// binaries: each name in `keep` (`data`, `my.ini`, ...) that the replaced
/// installation had is moved into the new one -- over the fresh copy the
/// staging made -- before the old one is deleted. Returns whether something was
/// replaced, and which of `keep` were carried across.
///
/// Without this a `--force` reinstall of MariaDB deleted every database with
/// the retired binaries. If carrying one across fails, the retired folder is
/// left where it is and the error names it, rather than deleting the only copy.
fn swap_keeping(
    stack: &Stack,
    dir: &Path,
    staging: &Path,
    dest: &Path,
    tag: &str,
    keep: &[&str],
) -> Result<(bool, Vec<String>)> {
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

    let mut kept = Vec::new();
    if replaced {
        for name in keep {
            let old = retired.join(name);
            if !old.exists() {
                continue;
            }
            let fresh = dest.join(name);
            let cleared = if fresh.is_dir() {
                std::fs::remove_dir_all(&fresh)
            } else if fresh.exists() {
                std::fs::remove_file(&fresh)
            } else {
                Ok(())
            };
            if let Err(err) = cleared.and_then(|()| std::fs::rename(&old, &fresh)) {
                return Err(anyhow::Error::new(err).context(format!(
                    "carrying {name} across to the new {tag}; the previous installation, \
                     {name} included, is still in {}",
                    stack.rel(&retired)
                )));
            }
            kept.push(name.to_string());
        }
        let _ = std::fs::remove_dir_all(&retired);
    }
    Ok((replaced, kept))
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

/// What `install_with` actually did.
pub enum InstallOutcome {
    /// A version installed successfully.
    Installed(Box<Installed>),
    /// No version was named, so a catalogue (or, for the vendors that publish
    /// none, guidance) was shown instead and nothing was installed.
    Listed,
}

/// `devcrate install <runtime>` -- the CLI entry point. Prints to the real
/// terminal exactly as before; the logic lives in [`install_with`], which the
/// dashboard calls too, through [`install_quiet`], without any of this
/// printing.
pub fn install(
    stack: &Stack,
    runtime: &str,
    version: Option<&str>,
    from: Option<&Path>,
    force: bool,
) -> Result<u8> {
    let mut emit = TerminalEmit::new();
    install_with(stack, runtime, version, from, force, &mut emit)?;
    Ok(exit::OK)
}

/// The dashboard's entry point: identical to [`install`], minus the
/// printing, plus the outcome the CLI's printing is normally built from --
/// see docs/issue/tui-install-stdout-corruption-2026-09-19.md. Always
/// downloads (the `--from` archive path is a CLI-only affordance).
///
/// `on_progress` receives the same in-place progress text the CLI redraws
/// (`downloading  42%  12.0 MB / 28.6 MB`, `extracting  80%`) -- and nothing
/// else, so the caller decides how often to show it.
pub fn install_quiet(
    stack: &Stack,
    runtime: &str,
    version: Option<&str>,
    force: bool,
    on_progress: &mut dyn FnMut(&str),
) -> Result<InstallOutcome> {
    let mut emit = ProgressEmit(on_progress);
    install_with(stack, runtime, version, None, force, &mut emit)
}

/// The password a freshly initialised PostgreSQL cluster's `postgres` user has
/// -- what `devcrate db` connects with unless `PGPASSWORD` is set.
pub fn postgres_default_password() -> &'static str {
    postgres::DEFAULT_PASSWORD
}

/// One entry in the dashboard's install picker: a runtime and a version
/// spelling [`install_quiet`] resolves -- always named, so choosing it installs
/// something rather than listing a catalogue.
#[derive(Debug, Clone)]
pub struct Offering {
    pub runtime: &'static str,
    pub label: String,
    pub version: String,
}

impl Offering {
    fn new(runtime: &'static str, label: impl Into<String>, version: impl Into<String>) -> Offering {
        Offering { runtime, label: label.into(), version: version.into() }
    }
}

/// What the picker offers before the catalogues are read, or when one cannot
/// be: every runtime by a line or keyword rather than a version, so nothing
/// here goes stale. Installing still needs the network either way.
pub fn fallback_offerings() -> Vec<Offering> {
    vec![
        Offering::new("php", "PHP (newest branch)", "latest"),
        Offering::new("nginx", "nginx (stable line)", "stable"),
        Offering::new("nginx", "nginx (mainline)", "mainline"),
        Offering::new("composer", "Composer (stable)", "stable"),
        Offering::new("mariadb", "MariaDB (newest)", "latest"),
        Offering::new("postgres", "PostgreSQL 13.23", "13.23"),
        Offering::new("redis", "Redis (newest)", "latest"),
        Offering::new("erlang", "Erlang/OTP (newest) -- RabbitMQ needs it", "latest"),
        Offering::new("rabbitmq", "RabbitMQ (newest)", "latest"),
        Offering::new("node", "Node.js (newest LTS)", "lts"),
        Offering::new("bun", "Bun (newest)", "latest"),
        Offering::new("python", "Python 3.8", "3.8"),
        Offering::new("adminer", "Adminer (newest) -- database browser", "latest"),
    ]
}

/// The picker's entries, named from the vendors' catalogues so the label says
/// exactly what will be installed. Network-bound and silent -- the dashboard
/// runs it on a thread of its own. A runtime whose catalogue cannot be read
/// keeps its [`fallback_offerings`] entries instead, so the list is never
/// shorter offline, only vaguer. PostgreSQL and Python have no index to read
/// (see [`choose_postgres`], [`choose_python`]) and always use the fallback.
pub fn offerings() -> Vec<Offering> {
    let fallback = fallback_offerings();
    let mut out = Vec::new();
    let keep = |runtime: &str, out: &mut Vec<Offering>| {
        out.extend(fallback.iter().filter(|o| o.runtime == runtime).cloned());
    };

    match download::php_catalogue() {
        // Sorted oldest first; the newest few branches are the useful ones.
        Ok(catalogue) => out.extend(catalogue.iter().rev().take(5).map(|r| {
            Offering::new("php", format!("PHP {}  ({})", r.version, r.release), r.version.clone())
        })),
        Err(_) => keep("php", &mut out),
    }

    match download::nginx_catalogue() {
        Ok(catalogue) => {
            for (channel, name) in
                [(download::Channel::Stable, "stable"), (download::Channel::Mainline, "mainline")]
            {
                if let Some(r) = catalogue.iter().find(|r| r.channel == channel) {
                    out.push(Offering::new("nginx", format!("nginx {}  ({name})", r.version), r.version.clone()));
                }
            }
        }
        Err(_) => keep("nginx", &mut out),
    }

    match download::composer_catalogue().ok().and_then(|c| c.latest_stable().map(|r| r.version.clone())) {
        Some(version) => out.push(Offering::new("composer", format!("Composer {version}  (stable)"), "stable")),
        None => keep("composer", &mut out),
    }

    // These four lists are built into the binary rather than fetched (see
    // download.rs), newest first -- the head is what `latest` would pick.
    let mut newest = |runtime: &'static str, label: &str, head: Option<String>| match head {
        Some(v) => out.push(Offering::new(runtime, label.replace("{v}", &v), v)),
        None => keep(runtime, &mut out),
    };
    newest("mariadb", "MariaDB {v}", download::mariadb_catalogue().ok().and_then(|c| c.first().map(|r| r.version.clone())));
    newest("postgres", "", None);
    newest("redis", "Redis {v}", download::redis_catalogue().ok().and_then(|c| c.first().map(|r| r.version.clone())));
    newest(
        "erlang",
        "Erlang/OTP {v} -- RabbitMQ needs it",
        download::erlang_catalogue().ok().and_then(|c| c.first().map(|r| r.version.clone())),
    );
    newest("rabbitmq", "RabbitMQ {v}", download::rabbitmq_catalogue().ok().and_then(|c| c.first().map(|r| r.version.clone())));

    match download::fetch_node_catalogue() {
        Ok(catalogue) => {
            let lts = catalogue.releases.iter().find(|r| r.lts.is_some());
            if let Some(r) = lts {
                let line = r.lts.as_deref().unwrap_or_default();
                out.push(Offering::new("node", format!("Node.js {}  (LTS {line})", r.version), r.version.clone()));
            }
            if let Some(r) = catalogue.releases.first().filter(|r| Some(r.version.as_str()) != lts.map(|l| l.version.as_str())) {
                out.push(Offering::new("node", format!("Node.js {}  (current)", r.version), r.version.clone()));
            }
        }
        Err(_) => keep("node", &mut out),
    }

    match download::fetch_bun_catalogue().ok().and_then(|c| c.releases.first().map(|r| r.version.clone())) {
        Some(version) => out.push(Offering::new("bun", format!("Bun {version}"), version)),
        None => keep("bun", &mut out),
    }

    keep("python", &mut out);

    match download::adminer_catalogue().ok().and_then(|c| c.first().map(|r| r.version.clone())) {
        Some(version) => {
            out.push(Offering::new("adminer", format!("Adminer {version}  (database browser)"), version))
        }
        None => keep("adminer", &mut out),
    }
    out
}

fn install_with(
    stack: &Stack,
    runtime: &str,
    version: Option<&str>,
    from: Option<&Path>,
    force: bool,
    emit: &mut dyn Emit,
) -> Result<InstallOutcome> {
    let runtime = Runtime::parse(runtime)?;

    let Some(archive_path) = from else {
        return install_by_download(stack, runtime, version, force, emit);
    };

    let opts = Options { version: version.map(str::to_string), force };
    let done = from_archive(stack, runtime, archive_path, &opts, &mut |progress| {
        emit.progress_event(runtime, progress)
    })?;
    emit.end_progress();

    print_installed(&done, emit);
    Ok(InstallOutcome::Installed(Box::new(done)))
}

/// Fetch the vendor's release list, download the wanted version into
/// `_downloads\`, and install it -- or, with no version named, print what is
/// available and stop.
fn install_by_download(
    stack: &Stack,
    runtime: Runtime,
    version: Option<&str>,
    force: bool,
    emit: &mut dyn Emit,
) -> Result<InstallOutcome> {
    // Each runtime answers the same two questions -- what can be had, and where
    // would this one land -- and everything after that is common. Exhaustive on
    // purpose: the next downloadable runtime has to answer them here too.
    let chosen = match runtime {
        Runtime::Php => choose_php(stack, version, emit)?,
        Runtime::Nginx => choose_nginx(stack, version, emit)?,
        Runtime::Composer => choose_composer(stack, version, emit)?,
        Runtime::MariaDb => choose_mariadb(stack, version, emit)?,
        Runtime::RabbitMq => choose_rabbitmq(stack, version, emit)?,
        Runtime::Erlang => choose_erlang(stack, version, emit)?,
        Runtime::Node => choose_node(stack, version, emit)?,
        Runtime::Bun => choose_bun(stack, version, emit)?,
        Runtime::Postgres => choose_postgres(stack, version, emit)?,
        Runtime::Python => choose_python(stack, version, emit)?,
        Runtime::Redis => choose_redis(stack, version, emit)?,
        Runtime::Adminer => choose_adminer(stack, version, emit)?,
    };
    // Nothing chosen means the catalogue was printed instead.
    let Some(Chosen { download: source, dest, replace_freely }) = chosen else {
        return Ok(InstallOutcome::Listed);
    };

    // The refusal --force overrides comes before the transfer, not after
    // thirty megabytes of it. Composer opts out: re-installing it is an update,
    // so an existing phar is never the thing that stops one.
    if dest.exists() && !force && !replace_freely {
        bail!("{} already exists; pass --force to replace it", stack.rel(&dest));
    }

    let downloads = stack.root.join(DOWNLOADS_DIR);
    let fetched = download::fetch(&source, &downloads, &mut |done, total| {
        emit.progress_event(runtime, Progress::Downloading { done, total })
    })?;
    emit.end_progress();
    report_transfer(stack, runtime, &downloads, &fetched, emit);

    let opts = Options { version: None, force };
    let done = from_archive(stack, runtime, &fetched.path, &opts, &mut |progress| {
        emit.progress_event(runtime, progress)
    })?;
    emit.end_progress();

    print_installed(&done, emit);
    Ok(InstallOutcome::Installed(Box::new(done)))
}

/// A release picked out of a catalogue: where to get it, and where it goes.
struct Chosen {
    download: download::Download,
    dest: PathBuf,
    /// An existing installation at `dest` is replaced rather than refused.
    /// True for the tools -- Composer and Adminer -- which update in place
    /// rather than being versions to protect.
    replace_freely: bool,
}

fn choose_php(stack: &Stack, version: Option<&str>, emit: &mut dyn Emit) -> Result<Option<Chosen>> {
    emit_line!(emit, "  fetching the release list from windows.php.net");
    let catalogue = download::php_catalogue()?;

    let Some(wanted) = version else {
        print_php_catalogue(stack, &catalogue, emit);
        return Ok(None);
    };

    // The feed is sorted oldest branch first, so the newest is the last.
    let latest = wanted.trim().eq_ignore_ascii_case("latest");
    let wanted = match catalogue.last() {
        Some(newest) if latest => newest.version.as_str(),
        _ => wanted,
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
        replace_freely: false,
    }))
}

fn choose_nginx(stack: &Stack, version: Option<&str>, emit: &mut dyn Emit) -> Result<Option<Chosen>> {
    // Refused before the network call rather than after it: a stack in the old
    // layout has nowhere to put a second build, and finding that out is not
    // worth a round trip.
    let prefix = nginx::install_prefix(&stack.nginx_prefix)?.to_path_buf();

    emit_line!(emit, "  fetching the download page from nginx.org");
    let catalogue = download::nginx_catalogue()?;

    let Some(wanted) = version else {
        print_nginx_catalogue(stack, &prefix, &catalogue, emit);
        return Ok(None);
    };

    let release = pick_nginx(&catalogue, wanted)?;
    Ok(Some(Chosen {
        download: release.download(),
        dest: prefix.join(format!("nginx-{}", release.version)),
        replace_freely: false,
    }))
}

fn choose_composer(stack: &Stack, version: Option<&str>, emit: &mut dyn Emit) -> Result<Option<Chosen>> {
    emit_line!(emit, "  fetching the version list from getcomposer.org");
    let catalogue = download::composer_catalogue()?;

    let Some(wanted) = version else {
        print_composer_catalogue(stack, &catalogue, emit);
        return Ok(None);
    };

    let release = pick_composer(&catalogue, wanted)?;
    emit_line!(emit, "  fetching the checksum for {}", release.version);
    Ok(Some(Chosen {
        // Composer's is the download that fetches its own sha256 sidecar, so
        // this is the one `download()` that can fail.
        download: release.download()?,
        dest: stack.root.join("composer").join("composer.phar"),
        replace_freely: true,
    }))
}

/// The EDB binaries zip a named PostgreSQL minor resolves to.
///
/// Unlike the other three runtimes there is no catalogue to print: EDB serves no
/// machine-readable index of these zips, so a no-version invocation cannot list
/// them and says so instead. A named `major.minor` resolves to a predictable URL
/// on EDB's mirror, verified against its declared length over TLS -- the same
/// weaker check nginx gets, for the same reason (no published checksum).
fn choose_postgres(stack: &Stack, version: Option<&str>, emit: &mut dyn Emit) -> Result<Option<Chosen>> {
    let Some(wanted) = version else {
        print_postgres_guidance(stack, emit);
        return Ok(None);
    };

    emit_line!(emit, "  locating PostgreSQL {wanted} on get.enterprisedb.com");
    let release = download::postgres_release(wanted)?;
    emit_line!(emit, "  PostgreSQL {} -> {}", release.version, release.file_name);
    Ok(Some(Chosen {
        download: release.download(),
        dest: stack.root.join("postgres"),
        replace_freely: false,
    }))
}

/// What `devcrate install postgres` with no version prints. Not a catalogue --
/// EDB publishes none -- but the honest shape of one: name an exact minor, and
/// here is the current final of a series that is now EOL.
fn print_postgres_guidance(stack: &Stack, emit: &mut dyn Emit) {
    let dir = stack.root.join("postgres");
    let installed = read_receipt_version(&dir).filter(|v| v != "unknown");

    emit_line!(emit);
    emit_line!(emit, "PostgreSQL (EDB Windows x64 binaries):");
    emit_line!(emit);
    match installed {
        Some(version) => emit_line!(emit, "  installed: {version}   in {}", stack.rel(&dir)),
        None => emit_line!(emit, "  not installed"),
    }
    emit_line!(emit);
    emit_line!(emit, "EDB serves no version index, so there is nothing to list -- name an");
    emit_line!(emit, "exact minor release and it is fetched from EDB's mirror:");
    emit_line!(emit);
    emit_line!(emit, "  devcrate install postgres 13.23");
    emit_line!(emit);
    emit_line!(emit, "Note: PostgreSQL 13 reached end-of-life on 2025-11-13; 13.23 is its");
    emit_line!(emit, "final release. The download is checked against its declared length");
    emit_line!(emit, "over TLS -- EDB publishes no checksum for these zips.");
}

/// The embeddable Python zip a named branch or release resolves to.
///
/// Like PostgreSQL there is no catalogue to print -- python.org publishes no
/// machine-readable index of which patch releases carry a Windows zip -- so a
/// no-version invocation guides rather than lists, and a named version resolves
/// to a URL on python.org's archive, verified against its declared length over
/// TLS (python.org publishes MD5 and GPG, no sha256).
fn choose_python(stack: &Stack, version: Option<&str>, emit: &mut dyn Emit) -> Result<Option<Chosen>> {
    let Some(wanted) = version else {
        print_python_guidance(stack, emit);
        return Ok(None);
    };

    emit_line!(emit, "  locating Python {wanted} on python.org");
    let release = download::python_release(wanted)?;
    emit_line!(emit, "  Python {} -> {}", release.release, release.file_name);
    Ok(Some(Chosen {
        download: release.download(),
        dest: stack.python_dir.join(format!("python-{}", release.version)),
        replace_freely: false,
    }))
}

/// What `devcrate install python` with no version prints.
fn print_python_guidance(stack: &Stack, emit: &mut dyn Emit) {
    emit_line!(emit);
    emit_line!(emit, "Python (embeddable amd64, from python.org):");
    emit_line!(emit);
    let installed = python_versions_installed(stack);
    if installed.is_empty() {
        emit_line!(emit, "  none installed");
    } else {
        emit_line!(emit, "  installed: {}", installed.join(", "));
    }
    emit_line!(emit);
    emit_line!(emit, "python.org serves no version index, so there is nothing to list --");
    emit_line!(emit, "name a branch or an exact release and it is fetched:");
    emit_line!(emit);
    emit_line!(emit, "  devcrate install python 3.8       (the newest 3.8.x Windows build)");
    emit_line!(emit, "  devcrate install python 3.8.10    (an exact release)");
    emit_line!(emit);
    emit_line!(emit, "Note: Python 3.8 is end-of-life (final Windows build 3.8.10). The zip");
    emit_line!(emit, "is checked against its declared length over TLS -- python.org");
    emit_line!(emit, "publishes MD5 and GPG for it, not sha256. pip is bootstrapped after.");
}

/// The installed Python branch names, for the guidance line. Scans `python\` the
/// same way the toolchain does, but without pulling in that module's printing.
fn python_versions_installed(stack: &Stack) -> Vec<String> {
    stack
        .python_versions()
        .iter()
        .map(|dir| {
            let tag = dir.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
            config::version_from_tag(&tag)
        })
        .collect()
}

/// The entry of a newest-first catalogue a spelling names: `latest` is the
/// first, anything else an exact version or a prefix of one (`11.4` reaches
/// the newest 11.4.x), with a leading `v` ignored.
fn pick_by_version<'a, T>(
    catalogue: &'a [T],
    wanted: &str,
    version: impl Fn(&T) -> &str,
) -> Option<&'a T> {
    let wanted = wanted.trim().trim_start_matches(['v', 'V']);
    if wanted.eq_ignore_ascii_case("latest") {
        return catalogue.first();
    }
    catalogue
        .iter()
        .find(|r| version(r) == wanted)
        .or_else(|| catalogue.iter().find(|r| version(r).starts_with(wanted)))
}

/// As [`pick_by_version`], plus `lts` for the newest long-term-support release.
fn pick_node<'a>(
    catalogue: &'a [download::NodeRelease],
    wanted: &str,
) -> Option<&'a download::NodeRelease> {
    if wanted.trim().eq_ignore_ascii_case("lts") {
        return catalogue.iter().find(|r| r.lts.is_some());
    }
    pick_by_version(catalogue, wanted, |r| &r.version)
}

fn choose_mariadb(stack: &Stack, version: Option<&str>, emit: &mut dyn Emit) -> Result<Option<Chosen>> {
    emit_line!(emit, "  fetching the MariaDB release list");
    let catalogue = download::mariadb_catalogue()?;

    let Some(wanted) = version else {
        emit_line!(emit, "\n  Available MariaDB releases:");
        for rel in &catalogue {
            emit_line!(emit, "    mariadb {} ({})", rel.version, rel.file_name);
        }
        emit_line!(emit, "\n  Pass a version to install: `devcrate install mariadb 11.4.5`");
        return Ok(None);
    };

    let release = pick_by_version(&catalogue, wanted, |r| &r.version)
        .ok_or_else(|| anyhow!("no MariaDB release matching {wanted:?}"))?;

    let download = release.download();
    let dest = stack.root.join("mariadb");

    Ok(Some(Chosen {
        download,
        dest,
        replace_freely: false,
    }))
}

fn choose_rabbitmq(stack: &Stack, version: Option<&str>, emit: &mut dyn Emit) -> Result<Option<Chosen>> {
    emit_line!(emit, "  fetching the RabbitMQ release list");
    let catalogue = download::rabbitmq_catalogue()?;

    let Some(wanted) = version else {
        emit_line!(emit, "\n  Available RabbitMQ releases:");
        for rel in &catalogue {
            emit_line!(emit, "    rabbitmq {} ({})", rel.version, rel.file_name);
        }
        emit_line!(emit, "\n  Pass a version to install: `devcrate install rabbitmq 4.0.5`");
        return Ok(None);
    };

    let release = pick_by_version(&catalogue, wanted, |r| &r.version)
        .ok_or_else(|| anyhow!("no RabbitMQ release matching {wanted:?}"))?;

    let download = release.download();
    let dest = stack.root.join("rabbitmq");

    Ok(Some(Chosen {
        download,
        dest,
        replace_freely: false,
    }))
}

fn choose_redis(stack: &Stack, version: Option<&str>, emit: &mut dyn Emit) -> Result<Option<Chosen>> {
    emit_line!(emit, "  fetching the Redis release list");
    let catalogue = download::redis_catalogue()?;

    let Some(wanted) = version else {
        emit_line!(emit, "\n  Available Redis releases (redis-windows/redis-windows on GitHub):");
        for rel in &catalogue {
            emit_line!(emit, "    redis {} ({})", rel.version, rel.file_name);
        }
        emit_line!(emit, "\n  Pass a version to install: `devcrate install redis 8.10.2`");
        return Ok(None);
    };

    let release = pick_by_version(&catalogue, wanted, |r| &r.version)
        .ok_or_else(|| anyhow!("no Redis release matching {wanted:?}"))?;

    let download = release.download();
    let dest = stack.root.join("redis");

    Ok(Some(Chosen {
        download,
        dest,
        replace_freely: false,
    }))
}

fn choose_adminer(stack: &Stack, version: Option<&str>, emit: &mut dyn Emit) -> Result<Option<Chosen>> {
    emit_line!(emit, "  fetching the Adminer release list from GitHub");
    let catalogue = download::adminer_catalogue()?;

    let Some(wanted) = version else {
        emit_line!(emit, "\n  Adminer releases (vrana/adminer on GitHub):");
        for rel in catalogue.iter().take(5) {
            emit_line!(emit, "    adminer {}", rel.version);
        }
        emit_line!(emit, "\n  Install the newest: `devcrate install adminer latest`");
        return Ok(None);
    };

    let release = pick_by_version(&catalogue, wanted, |r| &r.version)
        .ok_or_else(|| anyhow!("no Adminer release matching {wanted:?}"))?;
    Ok(Some(Chosen {
        download: release.download(),
        dest: stack.root.join("adminer").join("adminer.php"),
        replace_freely: true,
    }))
}

fn choose_erlang(stack: &Stack, version: Option<&str>, emit: &mut dyn Emit) -> Result<Option<Chosen>> {
    emit_line!(emit, "  fetching the Erlang release list");
    let catalogue = download::erlang_catalogue()?;

    let Some(wanted) = version else {
        emit_line!(emit, "\n  Available Erlang/OTP releases:");
        for rel in &catalogue {
            emit_line!(emit, "    erlang {} ({})", rel.version, rel.file_name);
        }
        emit_line!(emit, "\n  Pass a version to install: `devcrate install erlang 27.2`");
        return Ok(None);
    };

    let release = pick_by_version(&catalogue, wanted, |r| &r.version)
        .ok_or_else(|| anyhow!("no Erlang release matching {wanted:?}"))?;

    let download = release.download();
    let dest = stack.root.join("erlang");

    Ok(Some(Chosen {
        download,
        dest,
        replace_freely: false,
    }))
}

/// The release a spelling names: a line by name (`stable`, `lts`, `preview`,
/// `snapshot`), or an exact version off the stable list.
///
/// No dotted-prefix shorthand like nginx's -- the index carries only the
/// current release of each line, so there is no series of point releases for
/// `2.10` to disambiguate, and the line names cover what the shorthand was for.
/// An exact version therefore resolves only while it is still current; an older
/// one installs with `--from`.
fn pick_composer<'a>(
    catalogue: &'a download::ComposerCatalogue,
    wanted: &str,
) -> Result<&'a download::ComposerRelease> {
    let named = |line: Option<&'a download::ComposerRelease>, what: &str| {
        line.ok_or_else(|| anyhow!("getcomposer.org lists no {what} release right now"))
    };

    match wanted.trim().to_ascii_lowercase().as_str() {
        "stable" | "latest" => named(catalogue.latest_stable(), "stable"),
        "lts" => named(catalogue.lts.as_ref(), "LTS"),
        "preview" => named(catalogue.preview.as_ref(), "preview"),
        "snapshot" => named(catalogue.snapshot.as_ref(), "snapshot"),
        exact => catalogue
            .stable
            .iter()
            .find(|r| r.version == exact)
            .or_else(|| catalogue.lts.iter().find(|r| r.version == exact))
            .ok_or_else(|| {
                anyhow!(
                    "Composer {exact} is not on getcomposer.org's version list.\n\
                     Name a line -- stable, lts, preview, snapshot -- or a version it \
                     carries; the newest are: {}\n\
                     An older phar can still be installed with --from (docs/installation.md).",
                    catalogue
                        .stable
                        .iter()
                        .take(5)
                        .map(|r| r.version.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }),
    }
}

/// The release a spelling names, by the same rule `devcrate nginx use` uses on
/// the versions already installed -- exact, or an unambiguous dotted prefix, so
/// `1.30` reaches the current stable release without naming its patch level.
/// A line by name works too: `stable`, or `mainline` (also `latest`).
fn pick_nginx<'a>(
    catalogue: &'a [download::NginxRelease],
    wanted: &str,
) -> Result<&'a download::NginxRelease> {
    let keyword = wanted.trim().to_ascii_lowercase();
    let line = match keyword.as_str() {
        "stable" => Some(download::Channel::Stable),
        "mainline" | "latest" => Some(download::Channel::Mainline),
        _ => None,
    };
    if let Some(line) = line {
        return catalogue
            .iter()
            .find(|r| r.channel == line)
            .ok_or_else(|| anyhow!("nginx.org's download page lists no {keyword} release right now"));
    }

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

/// What the transfer came to. Said out loud because the vendors give different
/// amounts to go on, and which one you got is worth knowing.
fn report_transfer(
    stack: &Stack,
    runtime: Runtime,
    downloads: &Path,
    fetched: &download::Fetched,
    emit: &mut dyn Emit,
) {
    // Where the verifying hash came from, for the runtimes that publish one.
    let against = match runtime {
        Runtime::Php => "the release list",
        Runtime::Composer => "getcomposer.org's checksum",
        Runtime::MariaDb => "the vendor's archive",
        Runtime::RabbitMq => "GitHub releases",
        Runtime::Erlang => "GitHub releases",
        Runtime::Node => "nodejs.org releases index",
        Runtime::Bun => "GitHub releases",
        Runtime::Redis => "GitHub releases",
        Runtime::Adminer => "the sha256 GitHub records for the release",
        // nginx, PostgreSQL (EDB), and python.org publish no sha256, so these
        // arms are never the verified one -- the "no checksum" branch below
        // reports them instead.
        Runtime::Nginx | Runtime::Postgres | Runtime::Python => "the vendor's checksum",
    };
    match (fetched.cached, fetched.verified) {
        (true, true) => emit_line!(emit,
            "  already in {}, checksum still good -- nothing downloaded",
            stack.rel(downloads)
        ),
        (true, false) => {
            emit_line!(emit, "  already in {} -- nothing downloaded", stack.rel(downloads))
        }
        (false, true) => emit_line!(emit, "  sha256 verified against {against}"),
        (false, false) => {
            // The vendor that publishes no hash, named so the weaker check is
            // attributed rather than mysterious. RabbitMQ, Erlang, and Bun's
            // catalogues here are hardcoded lists with no hash either (see
            // download.rs), so they fall into this branch too -- named
            // properly rather than defaulting to "nginx".
            let vendor = match runtime {
                Runtime::Postgres => "EDB",
                Runtime::Python => "python.org",
                Runtime::MariaDb => "MariaDB",
                Runtime::RabbitMq => "RabbitMQ",
                Runtime::Erlang => "Erlang/OTP",
                Runtime::Bun => "Bun",
                Runtime::Redis => "redis-windows",
                Runtime::Adminer => "GitHub",
                _ => "nginx",
            };
            let host = match runtime {
                Runtime::Postgres => "the download host",
                Runtime::Python => "python.org",
                Runtime::MariaDb => "archive.mariadb.org",
                Runtime::RabbitMq | Runtime::Erlang | Runtime::Bun | Runtime::Redis | Runtime::Adminer => {
                    "github.com"
                }
                _ => "nginx.org",
            };
            emit_line!(emit, "  sha256 {}", fetched.sha256);
            emit_line!(emit, "  ({vendor} publishes no checksum; the transfer was checked");
            emit_line!(emit, "   against its declared length over TLS to {host})");
        }
    }
}

fn print_php_catalogue(stack: &Stack, catalogue: &[download::PhpRelease], emit: &mut dyn Emit) {
    emit_line!(emit);
    emit_line!(emit, "PHP releases on windows.php.net (thread-safe x64):");
    emit_line!(emit);
    let width = catalogue.iter().map(|r| r.release.len()).max().unwrap_or(0);
    for release in catalogue {
        let tag = format!("php-{}", release.version);
        let installed =
            if stack.php_dir.join(&tag).is_dir() { "   installed" } else { "" };
        emit_line!(emit, 
            "  {:<5}  {:<width$}  {:>8}{installed}",
            release.version, release.release, release.size
        );
    }
    emit_line!(emit);
    emit_line!(emit, "Install one with   devcrate install php 8.4");
    emit_line!(emit, "Only each branch's current release is offered; an older build");
    emit_line!(emit, "installs from a downloaded archive with --from.");
}

fn print_nginx_catalogue(
    stack: &Stack,
    prefix: &Path,
    catalogue: &[download::NginxRelease],
    emit: &mut dyn Emit,
) {
    emit_line!(emit);
    emit_line!(emit, "nginx for Windows on nginx.org:");
    emit_line!(emit);
    let width = catalogue.iter().map(|r| r.version.len()).max().unwrap_or(0);
    for release in catalogue {
        let dir = prefix.join(format!("nginx-{}", release.version));
        let installed = if dir.is_dir() { "   installed" } else { "" };
        let row = format!(
            "  {:<width$}  {:<8}{installed}",
            release.version,
            release.channel.label()
        );
        emit_line!(emit, "{}", row.trim_end());
    }
    emit_line!(emit);
    emit_line!(emit, "Install one with   devcrate install nginx 1.31.3");
    emit_line!(emit, "...or name a series: `1.30` takes the current stable release.");
    emit_line!(emit, "Each build gets its own folder in {}.", stack.rel(prefix));
}

fn print_composer_catalogue(stack: &Stack, catalogue: &download::ComposerCatalogue, emit: &mut dyn Emit) {
    let dir = stack.root.join("composer");
    let installed = read_receipt_version(&dir).filter(|v| v != "unknown");

    emit_line!(emit);
    emit_line!(emit, "Composer on getcomposer.org:");
    emit_line!(emit);

    // One line each -- Composer is a tool you take the current of, not a version
    // you pick among, so the lines are what to name and not a wall of releases.
    let mut row = |release: Option<&download::ComposerRelease>| {
        let Some(release) = release else { return };
        let here = installed.as_deref() == Some(release.version.as_str());
        let php = release
            .min_php
            .map(|n| format!("php >= {}", composer::format_min_php(n)))
            .unwrap_or_default();
        // snapshot's "version" is a 40-char commit hash; keep it from shoving
        // the columns after it off the line.
        let version = ellipsize(&release.version, 14);
        let line = format!(
            "  {:<9} {:<15} {:<14} {}",
            release.line.label(),
            version,
            php,
            if here { "installed" } else { "" }
        );
        emit_line!(emit, "{}", line.trim_end());
    };
    row(catalogue.latest_stable());
    row(catalogue.lts.as_ref());
    row(catalogue.preview.as_ref());
    row(catalogue.snapshot.as_ref());

    emit_line!(emit);
    emit_line!(emit, "Install the current stable    devcrate install composer stable");
    emit_line!(emit, "...the LTS line (for old PHP)  devcrate install composer lts");
    emit_line!(emit, "An older release installs from a downloaded phar with --from.");
    emit_line!(emit, "It lands in {}, runs under whatever php is on PATH.", stack.rel(&dir));
}

/// Where every status line, catalogue, and progress update an install
/// produces actually goes. [`TerminalEmit`] is what the CLI has always
/// printed; [`NullEmit`] is what the dashboard uses, because it must never
/// write to the screen ratatui is drawing on the main thread -- see
/// docs/issue/tui-install-stdout-corruption-2026-09-19.md. `choose_*`,
/// `print_*`, and `report_transfer` write through this instead of calling
/// `println!` directly, via the [`emit_line!`] macro.
trait Emit {
    /// A normal line of text.
    fn line(&mut self, text: &str);
    /// A percentage-style update, meant to redraw in place on a real terminal
    /// and to be otherwise discarded -- a download is millions of bytes and
    /// an extraction thousands of entries, and neither an install log
    /// redirected to a file nor the dashboard wants that many lines.
    fn progress(&mut self, text: &str);
    /// End a run of `progress` calls, so whatever prints next starts on its
    /// own line.
    fn end_progress(&mut self);

    /// Turn a [`Progress`] step into `line`/`progress` calls. One place for
    /// the formatting, shared by every `Emit` impl.
    fn progress_event(&mut self, runtime: Runtime, progress: Progress) {
        match progress {
            Progress::Downloading { done, total } => match total {
                Some(total) => self.progress(&format!(
                    "  downloading  {:>3}%  {} / {}",
                    done * 100 / total.max(1),
                    megabytes(done),
                    megabytes(total)
                )),
                None => self.progress(&format!("  downloading  {}", megabytes(done))),
            },
            Progress::Extracting { done, total } => {
                let percent = done * 100 / total.max(1);
                self.progress(&format!("  extracting   {percent:>3}%"));
            }
            Progress::Validating => {
                self.end_progress();
                match runtime {
                    Runtime::Composer => self.line("  checking the phar"),
                    Runtime::Adminer => self.line("  checking the file"),
                    _ => self.line("  checking the build"),
                }
            }
            // The one step that means genuinely different things: PHP's
            // configuration is the version's own, nginx's belongs to the prefix
            // it is about to sit in, and Composer's is the shim that launches it.
            Progress::Configuring => {
                self.end_progress();
                let text = match runtime {
                    Runtime::Php => "generating php.ini",
                    Runtime::Nginx => "preparing the prefix",
                    Runtime::Composer => "writing the composer shim",
                    Runtime::MariaDb => "generating my.ini",
                    Runtime::RabbitMq => "configuring rabbitmq plugins",
                    Runtime::Erlang => "configuring erlang environment",
                    Runtime::Node => "configuring portable npm environment",
                    Runtime::Bun => "preparing bun environment",
                    Runtime::Postgres => "initialising the cluster (initdb)",
                    Runtime::Python => "enabling site (for pip)",
                    Runtime::Redis => "generating devcrate.conf",
                    Runtime::Adminer => "writing the devcrate wrapper (index.php)",
                };
                self.line(&format!("  {text}"));
            }
            Progress::Installing => {
                self.end_progress();
                self.line("  moving it into place");
            }
        }
    }
}

/// Prints to the real terminal, redrawing `progress` in place -- exactly what
/// this file printed before it was split from the pipeline it reports on.
struct TerminalEmit {
    interactive: bool,
    last: String,
    drawing: bool,
}

impl TerminalEmit {
    fn new() -> TerminalEmit {
        TerminalEmit { interactive: std::io::stdout().is_terminal(), last: String::new(), drawing: false }
    }
}

impl Emit for TerminalEmit {
    fn line(&mut self, text: &str) {
        self.end_progress();
        println!("{text}");
    }

    /// Repaint the in-place line, but only when its text has changed -- the
    /// download callback fires on every 64 KiB read.
    fn progress(&mut self, text: &str) {
        if !self.interactive || text == self.last {
            return;
        }
        self.last = text.to_string();
        self.drawing = true;
        print!("\r{}", self.last);
        let _ = std::io::stdout().flush();
    }

    fn end_progress(&mut self) {
        if self.drawing {
            println!();
            self.drawing = false;
        }
    }
}

/// Discards everything. The dashboard's `Job::Install` uses this: it already
/// reports its own result through `JobResult`, and must never write to the
/// screen ratatui owns.
/// Hands progress text to a callback and drops every line -- the dashboard's
/// `Emit`, which shows where an install is up to and builds its own summary.
struct ProgressEmit<'a>(&'a mut dyn FnMut(&str));

impl Emit for ProgressEmit<'_> {
    fn line(&mut self, _text: &str) {}
    fn progress(&mut self, text: &str) {
        (self.0)(text.trim());
    }
    fn end_progress(&mut self) {}
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

/// Trim a long display string to `max` characters, marking the cut with `…`.
fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

fn print_installed(done: &Installed, emit: &mut dyn Emit) {
    emit_line!(emit);
    let files = if done.files == 1 { "1 file".to_string() } else { format!("{} files", done.files) };
    emit_line!(emit, "  {files} into {}", done.dir);
    match &done.details {
        Details::Php(php) => print_php_installed(done, php, emit),
        Details::Nginx(nginx) => print_nginx_installed(done, nginx, emit),
        Details::Composer(composer) => print_composer_installed(done, composer, emit),
        Details::MariaDb(mariadb) => print_mariadb_installed(done, mariadb, emit),
        Details::RabbitMq(rabbitmq) => print_rabbitmq_installed(done, rabbitmq, emit),
        Details::Erlang(erlang) => print_erlang_installed(done, erlang, emit),
        Details::Node(node) => print_node_installed(done, node, emit),
        Details::Bun(bun) => print_bun_installed(done, bun, emit),
        Details::Postgres(postgres) => print_postgres_installed(done, postgres, emit),
        Details::Python(py) => print_python_installed(done, py, emit),
        Details::Redis(redis) => print_redis_installed(done, redis, emit),
        Details::Adminer(adminer) => print_adminer_installed(done, adminer, emit),
    }
}

fn print_python_installed(done: &Installed, py: &PythonInstalled, emit: &mut dyn Emit) {
    if py.site_enabled {
        emit_line!(emit, "  enabled `import site` (so pip can be installed and found)");
    }
    if done.replaced {
        emit_line!(emit, "  replaced the previous {} installation", done.version);
    }

    emit_line!(emit);
    emit_line!(emit, "{} {} installed as python-{}", done.runtime.label(), done.release, done.version);

    match &py.pip {
        Some(python::Pip::Installed) => emit_line!(emit, "  pip installed"),
        Some(python::Pip::Skipped(why)) => {
            emit_line!(emit);
            emit_line!(emit, "NOTE: pip was not installed ({why}).");
            emit_line!(emit, "      The Python is installed and usable; finish pip when online:");
            emit_line!(emit, 
                "        python\\python-{}\\python.exe -m ensurepip  (or re-run the install)",
                done.version
            );
        }
        None => {}
    }

    emit_line!(emit);
    emit_line!(emit, "  make it the CLI Python   devcrate python use {}", done.version);
    emit_line!(emit, "  then                     python --version");
    emit_line!(emit, "  (put python\\current, and its Scripts\\, on PATH once -- like php\\current)");
}

fn print_node_installed(done: &Installed, node: &NodeInstalledDetails, emit: &mut dyn Emit) {
    emit_line!(emit, "  Node.js v{} installed into {}", done.version, node.dir);
    emit_line!(emit, "  NPM global prefix set to {}", node.global_dir);
    emit_line!(emit);
    emit_line!(emit, "Use `devcrate node use {}` to activate it.", done.version);
}

fn print_bun_installed(done: &Installed, bun: &BunInstalledDetails, emit: &mut dyn Emit) {
    emit_line!(emit, "  Bun v{} installed into {}", done.version, bun.dir);
    emit_line!(emit);
    emit_line!(emit, "Use `devcrate bun use {}` to activate it.", done.version);
}

fn choose_node(stack: &Stack, version: Option<&str>, emit: &mut dyn Emit) -> Result<Option<Chosen>> {
    emit_line!(emit, "  fetching release index from nodejs.org");
    let catalogue = download::fetch_node_catalogue()?;

    let Some(wanted) = version else {
        print_node_catalogue(&catalogue, emit);
        return Ok(None);
    };

    let Some(release) = pick_node(&catalogue.releases, wanted) else {
        bail!(
            "Node.js {wanted:?} is not on nodejs.org's release list.\n\
             An archive can still be installed with --from."
        );
    };

    let dl = download::Download {
        url: release.url.clone(),
        file_name: release.archive_name.clone(),
        sha256: None,
    };

    Ok(Some(Chosen {
        download: dl,
        dest: stack.root.join("node").join(format!("v{}", release.version)),
        replace_freely: false,
    }))
}

fn print_node_catalogue(catalogue: &download::NodeCatalogue, emit: &mut dyn Emit) {
    emit_line!(emit, "\nNode.js releases available on nodejs.org:");
    for (i, rel) in catalogue.releases.iter().take(15).enumerate() {
        let lts_tag = rel.lts.as_ref().map(|l| format!(" [{l}]")).unwrap_or_default();
        emit_line!(emit, "  {:2}. v{}{lts_tag}", i + 1, rel.version);
    }
    emit_line!(emit, "\nRun `devcrate install node <version>` to install one.");
}

fn choose_bun(stack: &Stack, version: Option<&str>, emit: &mut dyn Emit) -> Result<Option<Chosen>> {
    emit_line!(emit, "  fetching Bun releases from GitHub");
    let catalogue = download::fetch_bun_catalogue()?;

    let Some(wanted) = version else {
        print_bun_catalogue(&catalogue, emit);
        return Ok(None);
    };

    let Some(release) = pick_by_version(&catalogue.releases, wanted, |r| &r.version) else {
        bail!(
            "Bun {wanted:?} is not on GitHub release list.\n\
             An archive can still be installed with --from."
        );
    };

    let dl = download::Download {
        url: release.url.clone(),
        file_name: release.archive_name.clone(),
        sha256: None,
    };

    Ok(Some(Chosen {
        download: dl,
        dest: stack.root.join("bun").join(format!("v{}", release.version)),
        replace_freely: false,
    }))
}

fn print_bun_catalogue(catalogue: &download::BunCatalogue, emit: &mut dyn Emit) {
    emit_line!(emit, "\nBun releases available on GitHub:");
    for (i, rel) in catalogue.releases.iter().take(15).enumerate() {
        emit_line!(emit, "  {:2}. v{}", i + 1, rel.version);
    }
    emit_line!(emit, "\nRun `devcrate install bun <version>` to install one.");
}

fn node_from_archive(
    stack: &Stack,
    archive_path: &Path,
    opts: &Options,
    progress: &mut dyn FnMut(Progress),
) -> Result<Installed> {
    if !archive_path.is_file() {
        bail!("{} is not a file", archive_path.display());
    }
    let file_name = archive_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let source_sha256 = download::sha256_of(archive_path)
        .with_context(|| format!("hashing {}", archive_path.display()))?;

    let version = opts
        .version
        .clone()
        .or_else(|| node::version_from_file_name(&file_name))
        .unwrap_or_else(|| "22.11.0".to_string());

    let node_root = stack.root.join("node");
    let dest = node_root.join(format!("v{version}"));

    if dest.exists() && !opts.force {
        bail!("{} already exists; pass --force to replace it", stack.rel(&dest));
    }

    let staging = stack.root.join(staging_name("node"));
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    std::fs::create_dir_all(&staging)?;

    let extracted = archive::extract(archive_path, &staging, &mut |done, total| {
        progress(Progress::Extracting { done, total });
    })?;

    progress(Progress::Validating);
    let valid_dir = node::validate(&staging)?;

    progress(Progress::Installing);
    std::fs::create_dir_all(&node_root)?;
    if dest.exists() {
        let _ = std::fs::remove_dir_all(&dest);
    }

    std::fs::rename(&valid_dir, &dest)?;
    let _ = std::fs::remove_dir_all(&staging);

    let npm_global = node_root.join("npm-global");
    let npm_cache = node_root.join("npm-cache");
    let _ = std::fs::create_dir_all(&npm_global);
    let _ = std::fs::create_dir_all(&npm_cache);

    let npmrc = dest.join(".npmrc");
    if !npmrc.exists() {
        let text = format!(
            "prefix={}\ncache={}\n",
            npm_global.display().to_string().replace('\\', "/"),
            npm_cache.display().to_string().replace('\\', "/")
        );
        let _ = std::fs::write(&npmrc, text);
    }

    let receipt = Receipt {
        runtime: Runtime::Node.as_str().to_string(),
        version: version.clone(),
        release: version.clone(),
        thread_safe: None,
        source_archive: file_name,
        source_bytes: std::fs::metadata(archive_path).map(|m| m.len()).unwrap_or(0),
        source_sha256,
        files: extracted.files,
    };
    write_receipt(&dest, &receipt)?;

    Ok(Installed {
        runtime: Runtime::Node,
        version: version.clone(),
        release: version,
        dir: stack.rel(&dest),
        files: extracted.files,
        replaced: false,
        details: Details::Node(NodeInstalledDetails {
            dir: stack.rel(&dest),
            global_dir: stack.rel(&npm_global),
        }),
    })
}

fn bun_from_archive(
    stack: &Stack,
    archive_path: &Path,
    opts: &Options,
    progress: &mut dyn FnMut(Progress),
) -> Result<Installed> {
    if !archive_path.is_file() {
        bail!("{} is not a file", archive_path.display());
    }
    let file_name = archive_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let source_sha256 = download::sha256_of(archive_path)
        .with_context(|| format!("hashing {}", archive_path.display()))?;

    let version = opts
        .version
        .clone()
        .or_else(|| bun::version_from_file_name(&file_name))
        .unwrap_or_else(|| "1.2.2".to_string());

    let bun_root = stack.root.join("bun");
    let dest = bun_root.join(format!("v{version}"));

    if dest.exists() && !opts.force {
        bail!("{} already exists; pass --force to replace it", stack.rel(&dest));
    }

    let staging = stack.root.join(staging_name("bun"));
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    std::fs::create_dir_all(&staging)?;

    let extracted = archive::extract(archive_path, &staging, &mut |done, total| {
        progress(Progress::Extracting { done, total });
    })?;

    progress(Progress::Validating);
    let valid_dir = bun::validate(&staging)?;

    progress(Progress::Installing);
    std::fs::create_dir_all(&bun_root)?;
    if dest.exists() {
        let _ = std::fs::remove_dir_all(&dest);
    }

    std::fs::rename(&valid_dir, &dest)?;
    let _ = std::fs::remove_dir_all(&staging);

    let receipt = Receipt {
        runtime: Runtime::Bun.as_str().to_string(),
        version: version.clone(),
        release: version.clone(),
        thread_safe: None,
        source_archive: file_name,
        source_bytes: std::fs::metadata(archive_path).map(|m| m.len()).unwrap_or(0),
        source_sha256,
        files: extracted.files,
    };
    write_receipt(&dest, &receipt)?;

    Ok(Installed {
        runtime: Runtime::Bun,
        version: version.clone(),
        release: version,
        dir: stack.rel(&dest),
        files: extracted.files,
        replaced: false,
        details: Details::Bun(BunInstalledDetails {
            dir: stack.rel(&dest),
        }),
    })
}

fn print_mariadb_installed(done: &Installed, mariadb: &MariaDbInstalled, emit: &mut dyn Emit) {
    if mariadb.config_created {
        emit_line!(emit, "  my.ini generated with default settings");
    }
    if mariadb.data_dir_created {
        emit_line!(emit, "  created data/ directory");
    }
    if done.replaced {
        emit_line!(emit, "  replaced the previous MariaDB installation");
    }
    emit_line!(emit);
    emit_line!(emit, "MariaDB {} installed to {}", done.version, mariadb.dir);
    emit_line!(emit, "  start MariaDB service   devcrate start mariadb");
}

fn print_rabbitmq_installed(done: &Installed, rabbitmq: &RabbitMqInstalled, emit: &mut dyn Emit) {
    if rabbitmq.data_dir_created {
        emit_line!(emit, "  created data/ directory");
    }
    if rabbitmq.plugins_file_created {
        emit_line!(emit, "  enabled rabbitmq_management plugin");
    }
    if done.replaced {
        emit_line!(emit, "  replaced the previous RabbitMQ installation");
    }
    emit_line!(emit);
    emit_line!(emit, "RabbitMQ {} installed to {}", done.version, rabbitmq.dir);
    emit_line!(emit, "  start RabbitMQ service  devcrate start rabbitmq");
}

fn print_redis_installed(done: &Installed, redis: &RedisInstalled, emit: &mut dyn Emit) {
    if redis.conf_created {
        emit_line!(emit, "  devcrate.conf generated with default settings");
    }
    if redis.data_dir_created {
        emit_line!(emit, "  created data/ directory");
    }
    if done.replaced {
        emit_line!(emit, "  replaced the previous Redis installation");
    }
    emit_line!(emit);
    emit_line!(emit, "Redis {} installed to {}", done.version, redis.dir);
    emit_line!(emit, "  start it   devcrate start redis        (listens on {})", redis.port);
    emit_line!(emit, "  connect    redis-cli -p {}", redis.port);
    emit_line!(emit, "  stop it    devcrate stop redis");
}

fn print_erlang_installed(done: &Installed, erlang: &ErlangInstalled, emit: &mut dyn Emit) {
    if done.replaced {
        emit_line!(emit, "  replaced the previous Erlang/OTP installation");
    }
    emit_line!(emit);
    emit_line!(emit, "Erlang/OTP {} installed to {}", done.version, erlang.dir);
}

fn print_php_installed(done: &Installed, php: &PhpInstalled, emit: &mut dyn Emit) {
    emit_line!(emit, 
        "  php.ini generated from php.ini-development, {} extensions enabled",
        php.enabled_extensions.len()
    );
    if done.replaced {
        emit_line!(emit, "  replaced the previous {} installation", done.version);
    }

    if !php.missing_extensions.is_empty() {
        emit_line!(emit);
        emit_line!(emit, 
            "WARNING: php.ini-development had no line for: {}",
            php.missing_extensions.join(", ")
        );
        emit_line!(emit, "         They are not enabled. Add them by hand if this version needs them.");
    }

    if php.vcredist == Some(false) {
        emit_line!(emit);
        emit_line!(emit, "WARNING: the Visual C++ runtime (vcruntime140.dll) was not found.");
        emit_line!(emit, "         php-cgi.exe exits with no output at all without it. Install");
        emit_line!(emit, "         https://aka.ms/vs/17/release/vc_redist.x64.exe before starting.");
    }

    emit_line!(emit);
    let label = done.runtime.label();
    match php.fastcgi_port {
        Some(port) => emit_line!(emit, 
            "{label} {} installed as php-{} (fastcgi {port})",
            done.release, done.version
        ),
        None => emit_line!(emit, 
            "{label} {} installed as php-{} (no default FastCGI port for this \
             version; set one in devcrate.toml)",
            done.release, done.version
        ),
    }
    emit_line!(emit, "  serve a site with it   devcrate site add myapp.test --php {}", done.version);
    emit_line!(emit, "  make it the CLI PHP    devcrate php use {}", done.version);
    emit_line!(emit, "  start its worker       devcrate start php-{}", done.version);
}

fn print_nginx_installed(done: &Installed, nginx: &NginxInstalled, emit: &mut dyn Emit) {
    for created in &nginx.prepared.created {
        emit_line!(emit, "  created {created}");
    }
    if nginx.activated {
        emit_line!(emit, "  {}\\current -> {}", nginx.prefix, done.dir);
    }
    if done.replaced {
        emit_line!(emit, "  replaced the previous {} installation", done.version);
    }

    if nginx.prepared.config_missing {
        emit_line!(emit);
        emit_line!(emit, "WARNING: {}\\conf\\nginx.conf is not there.", nginx.prefix);
        emit_line!(emit, "         It is tracked in the repository rather than generated, so a");
        emit_line!(emit, "         missing one means the checkout is incomplete. nginx will not");
        emit_line!(emit, "         start without it.");
    }

    // Only worth surfacing when it failed: a passing test says nothing that
    // the install succeeding does not already say.
    if let Some(test) = &nginx.config_test
        && !test.ok
    {
        emit_line!(emit);
        emit_line!(emit, "WARNING: this build does not accept the current configuration:");
        emit_line!(emit, "         {}", test.detail);
        emit_line!(emit, "         On a stack with no certificates yet this is expected --");
        emit_line!(emit, "         ssl_certificate names a file mkcert has not issued. Otherwise");
        emit_line!(emit, "         it is a directive the new nginx no longer takes.");
    }

    emit_line!(emit);
    emit_line!(emit, "{} {} installed as nginx-{}", done.runtime.label(), done.release, done.version);

    match (&nginx.active_instead, nginx.restart_needed) {
        (Some(active), _) => {
            emit_line!(emit, "  {}\\current still names {active}, so this build is installed", nginx.prefix);
            emit_line!(emit, "  but not the one that runs. To switch:");
            emit_line!(emit, "    devcrate nginx use {}", done.version);
        }
        (None, true) => {
            emit_line!(emit, "  nginx is running the previous build until it is restarted:");
            emit_line!(emit, "    devcrate restart nginx");
        }
        (None, false) => {
            emit_line!(emit, "  start it   devcrate start nginx");
            emit_line!(emit, "  check it   devcrate nginx list");
        }
    }
}

fn print_postgres_installed(done: &Installed, postgres: &PostgresInstalled, emit: &mut dyn Emit) {
    if postgres.initialised {
        emit_line!(emit, "  cluster created in {} (initdb)", postgres.data_dir);
    } else if postgres.preserved_data {
        emit_line!(emit, "  kept the existing cluster in {}", postgres.data_dir);
    }
    if done.replaced && !postgres.preserved_data {
        emit_line!(emit, "  replaced the previous binaries");
    }

    emit_line!(emit);
    emit_line!(emit, "{} {} installed as postgres", done.runtime.label(), done.release);

    match (&postgres.password, postgres.preserved_data) {
        (Some(password), _) => {
            emit_line!(emit, "  superuser  {} / {password}  (local dev default)", postgres.superuser);
            emit_line!(emit, "  change it  ALTER USER {} PASSWORD '...';", postgres.superuser);
        }
        (None, true) => {
            emit_line!(emit, "  superuser  {} (existing cluster; its password is unchanged)", postgres.superuser);
        }
        (None, false) => {}
    }

    emit_line!(emit);
    emit_line!(emit, "  start it   devcrate start postgres        (listens on {})", postgres.port);
    emit_line!(emit, "  connect    psql -U {} -h 127.0.0.1 -p {}", postgres.superuser, postgres.port);
    emit_line!(emit, "  stop it    devcrate stop postgres");
}

fn print_adminer_installed(done: &Installed, installed: &AdminerInstalled, emit: &mut dyn Emit) {
    match &installed.replaced_version {
        Some(old) if old == &done.version => emit_line!(emit, "  reinstalled the same version"),
        Some(old) => emit_line!(emit, "  updated from {old}"),
        None => {}
    }
    emit_line!(emit);
    emit_line!(emit, "Adminer {} installed in {}", done.version, installed.dir);
    emit_line!(emit, "  {}\\index.php serves it: to this machine only, and with no", installed.dir);
    emit_line!(emit, "  password needed (the stack's MariaDB root has none)");

    if !installed.drivers.is_empty() {
        emit_line!(emit);
        emit_line!(emit, "  database drivers, by the PHP that serves it:");
        let yes = |on: bool| if on { "yes" } else { "no " };
        for (php, drivers) in &installed.drivers {
            emit_line!(emit, "    {php:<9} MariaDB {}   PostgreSQL {}", yes(drivers.mariadb), yes(drivers.postgres));
        }
        if installed.drivers.iter().any(|(_, d)| !d.postgres) {
            emit_line!(emit, "  PostgreSQL needs `extension=pgsql` in that version's php.ini");
        }
    }

    emit_line!(emit);
    emit_line!(emit, "  serve it   devcrate site add {} --path {}", adminer::SITE, installed.full_dir);
}

fn print_composer_installed(done: &Installed, composer: &ComposerInstalled, emit: &mut dyn Emit) {
    for shim in &composer.shims {
        emit_line!(emit, "  wrote {shim}");
    }
    for created in &composer.created {
        emit_line!(emit, "  created {created}");
    }
    match &composer.replaced_version {
        Some(old) if old == &done.version => emit_line!(emit, "  reinstalled the same version"),
        Some(old) => emit_line!(emit, "  updated from {old}"),
        None if done.replaced => emit_line!(emit, "  replaced the previous phar"),
        None => {}
    }

    emit_line!(emit);
    if done.version == "unknown" {
        emit_line!(emit, "Composer installed in {}", composer.dir);
        emit_line!(emit, "  (its version could not be read -- no php\\current to run it, and the");
        emit_line!(emit, "   file name did not carry one)");
    } else {
        emit_line!(emit, "Composer {} installed in {}", done.version, composer.dir);
    }

    // The functional check: did the PHP on PATH actually run it? A failure here
    // is advisory -- the phar is installed and verified regardless.
    match &composer.run {
        Some(run) if run.ok => emit_line!(emit, "  {}", run.detail),
        Some(run) => {
            emit_line!(emit);
            emit_line!(emit, "WARNING: php\\current could not run the phar:");
            emit_line!(emit, "         {}", run.detail);
            emit_line!(emit, "         The phar is installed; this is the PHP it would run under.");
        }
        None => {
            emit_line!(emit);
            emit_line!(emit, "NOTE: no php\\current to run it with yet. Composer needs a PHP:");
            emit_line!(emit, "      devcrate install php 8.4  &&  devcrate php use 8.4");
        }
    }

    emit_line!(emit);
    emit_line!(emit, "  put it on PATH   add {} (once, like php\\current)", composer.dir);
    emit_line!(emit, "  then             composer --version");
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
        assert_eq!(Runtime::parse("composer").unwrap(), Runtime::Composer);
        assert_eq!(Runtime::parse("Composer").unwrap(), Runtime::Composer);
        assert_eq!(Runtime::parse("mariadb").unwrap(), Runtime::MariaDb);
        assert_eq!(Runtime::parse("rabbitmq").unwrap(), Runtime::RabbitMq);
        assert_eq!(Runtime::parse("erlang").unwrap(), Runtime::Erlang);
        assert_eq!(Runtime::parse("node").unwrap(), Runtime::Node);
        assert_eq!(Runtime::parse("bun").unwrap(), Runtime::Bun);
        assert_eq!(Runtime::parse("postgres").unwrap(), Runtime::Postgres);
        assert_eq!(Runtime::parse("postgresql").unwrap(), Runtime::Postgres);
        assert_eq!(Runtime::parse("python").unwrap(), Runtime::Python);

        let typo = Runtime::parse("pph").unwrap_err().to_string();
        assert!(typo.contains("not a runtime devcrate manages"), "{typo}");
    }

    fn composer_catalogue() -> download::ComposerCatalogue {
        let release = |version: &str, line| download::ComposerRelease {
            version: version.to_string(),
            path: format!("/download/{version}/composer.phar"),
            min_php: Some(70205),
            line,
        };
        download::ComposerCatalogue {
            stable: vec![
                release("2.10.2", download::ComposerLine::Stable),
                release("2.10.1", download::ComposerLine::Stable),
                release("2.7.1", download::ComposerLine::Stable),
            ],
            lts: Some(release("2.2.29", download::ComposerLine::Lts)),
            preview: Some(release("2.11.0-RC1", download::ComposerLine::Preview)),
            snapshot: None,
        }
    }

    /// A line by name, or an exact version off the history -- the two ways to
    /// pick a Composer, since it has no series to shorthand.
    #[test]
    fn a_composer_line_or_version_is_named() {
        let catalogue = composer_catalogue();
        assert_eq!(pick_composer(&catalogue, "stable").unwrap().version, "2.10.2");
        assert_eq!(pick_composer(&catalogue, "latest").unwrap().version, "2.10.2");
        assert_eq!(pick_composer(&catalogue, "LTS").unwrap().version, "2.2.29");
        assert_eq!(pick_composer(&catalogue, "preview").unwrap().version, "2.11.0-RC1");
        // An exact version resolves off the stable history...
        assert_eq!(pick_composer(&catalogue, "2.7.1").unwrap().version, "2.7.1");
        // ...or the LTS line.
        assert_eq!(pick_composer(&catalogue, "2.2.29").unwrap().version, "2.2.29");
    }

    /// A line the index does not carry today, and a version it never had, both
    /// have to say so rather than install the wrong thing.
    #[test]
    fn an_absent_line_or_unknown_version_is_an_error() {
        let catalogue = composer_catalogue();
        let no_snapshot = pick_composer(&catalogue, "snapshot").unwrap_err().to_string();
        assert!(no_snapshot.contains("no snapshot release"), "{no_snapshot}");

        let unknown = pick_composer(&catalogue, "9.9.9").unwrap_err().to_string();
        assert!(unknown.contains("not on getcomposer.org"), "{unknown}");
        // The newest few are offered as a hint, not the whole history.
        assert!(unknown.contains("2.10.2"), "{unknown}");
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

    /// A line by name, so the dashboard can offer nginx without baking a
    /// version into the binary.
    #[test]
    fn a_line_names_the_release_on_it() {
        let catalogue = nginx_catalogue();
        assert_eq!(pick_nginx(&catalogue, "stable").unwrap().version, "1.30.4");
        assert_eq!(pick_nginx(&catalogue, "mainline").unwrap().version, "1.31.3");
        assert_eq!(pick_nginx(&catalogue, "Latest").unwrap().version, "1.31.3");
    }

    /// `latest` is the head of a newest-first list; anything else is an exact
    /// version or a prefix of one, the first (newest) match winning.
    #[test]
    fn latest_and_prefixes_pick_from_a_newest_first_list() {
        let versions = ["11.4.5", "11.4.4", "10.11.11"];
        let pick = |wanted| pick_by_version(&versions, wanted, |v| v).copied();
        assert_eq!(pick("latest"), Some("11.4.5"));
        assert_eq!(pick("11.4"), Some("11.4.5"));
        assert_eq!(pick("11.4.4"), Some("11.4.4"));
        assert_eq!(pick("v10"), Some("10.11.11"));
        assert_eq!(pick("12"), None);
    }

    /// Live: reads every vendor's catalogue. `cargo test -- --ignored
    /// offerings_from_the_live_catalogues --nocapture` prints the picker.
    #[test]
    #[ignore = "needs the network"]
    fn offerings_from_the_live_catalogues() {
        let offerings = offerings();
        for o in &offerings {
            println!("{:<9} {:<10} {}", o.runtime, o.version, o.label);
        }
        for runtime in ["php", "nginx", "composer", "mariadb", "postgres", "redis", "erlang", "rabbitmq", "node", "bun", "python"] {
            assert!(offerings.iter().any(|o| o.runtime == runtime), "{runtime} is missing");
        }
    }

    /// A `--force` reinstall carries the service's state across instead of
    /// deleting it with the retired binaries -- it used to take every MariaDB
    /// database with it.
    #[test]
    fn a_reinstall_keeps_data_and_config() {
        let root = std::env::temp_dir().join(format!("devcrate-swap-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dest = root.join("mariadb");
        std::fs::create_dir_all(dest.join("data").join("myapp")).unwrap();
        std::fs::write(dest.join("data").join("myapp").join("t.frm"), "rows").unwrap();
        std::fs::write(dest.join("my.ini"), "port=3307").unwrap();
        std::fs::write(dest.join("old.exe"), "old").unwrap();

        let staging = root.join("staging");
        std::fs::create_dir_all(staging.join("data")).unwrap();
        std::fs::write(staging.join("my.ini"), "port=3306").unwrap();
        std::fs::write(staging.join("new.exe"), "new").unwrap();

        let stack = Stack::open(crate::root::StackRoot {
            path: root.clone(),
            source: crate::root::RootSource::Flag,
        })
        .unwrap();
        let (replaced, kept) =
            swap_keeping(&stack, &root, &staging, &dest, "mariadb", &["data", "my.ini", "absent"]).unwrap();

        assert!(replaced);
        assert_eq!(kept, ["data", "my.ini"]);
        assert!(dest.join("new.exe").is_file() && !dest.join("old.exe").exists(), "binaries replaced");
        assert_eq!(std::fs::read_to_string(dest.join("data").join("myapp").join("t.frm")).unwrap(), "rows");
        assert_eq!(std::fs::read_to_string(dest.join("my.ini")).unwrap(), "port=3307", "the hand-edited one");
        assert!(!root.join(retired_name("mariadb")).exists(), "nothing left behind");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn node_lines_pick_lts_or_current() {
        let release = |version: &str, lts: Option<&str>| download::NodeRelease {
            version: version.to_string(),
            lts: lts.map(str::to_string),
            url: String::new(),
            archive_name: String::new(),
        };
        let catalogue = [release("24.1.0", None), release("22.16.0", Some("Jod")), release("22.15.1", Some("Jod"))];
        assert_eq!(pick_node(&catalogue, "lts").unwrap().version, "22.16.0");
        assert_eq!(pick_node(&catalogue, "latest").unwrap().version, "24.1.0");
        assert_eq!(pick_node(&catalogue, "22").unwrap().version, "22.16.0");
    }
}
