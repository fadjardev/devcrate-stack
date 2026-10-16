//! The command surface.
//!
//! The whole surface was declared here before most of it was built, so the
//! shape was settled once and `devcrate --help` told the truth about what did
//! and did not work yet. By now every declared command is implemented,
//! `uninstall` included.
//!
//! No subcommand means the dashboard. Every subcommand is reachable from it,
//! and every action it offers is one of these calls -- the interactive and
//! scriptable halves are the same program over the same core.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "devcrate",
    version,
    about = "A portable, multi-PHP development stack for Windows",
    long_about = "Manage a Devcrate stack: nginx, PHP FastCGI workers, MariaDB, and RabbitMQ \
                  running out of a single stack-root folder.\n\n\
                  Run with no subcommand for the interactive dashboard.\n\n\
                  The stack root is taken from --root, then DEVCRATE_HOME, then the folder \
                  holding this executable, then the working directory (searching upward from \
                  each)."
)]
pub struct Cli {
    /// Stack root to operate on (overrides DEVCRATE_HOME and auto-detection).
    #[arg(long, global = true, value_name = "PATH")]
    pub root: Option<PathBuf>,

    /// Omitted: launch the dashboard.
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Show what is installed, what is running, and what holds a port.
    Status(StatusArgs),

    /// Inspect the resolved configuration.
    #[command(subcommand)]
    Config(ConfigCommand),

    /// PHP versions.
    #[command(subcommand)]
    Php(PhpCommand),

    /// Python versions (a toolchain on PATH, not a service).
    #[command(subcommand)]
    Python(PythonCommand),

    /// nginx versions and the layout of its prefix.
    #[command(subcommand)]
    Nginx(NginxCommand),

    /// Vhosts.
    #[command(subcommand)]
    Site(SiteCommand),

    /// Start the stack, or one service, in dependency order.
    Start(ServiceArgs),

    /// Stop the stack, or one service, in the safe shutdown order.
    Stop(ServiceArgs),

    /// Stop, then start again.
    Restart(ServiceArgs),

    /// Node.js versions.
    #[command(subcommand)]
    Node(NodeCommand),

    /// Bun versions.
    #[command(subcommand)]
    Bun(BunCommand),

    /// Set up a working stack in one step: nginx, PHP, MariaDB, Composer,
    /// and mkcert where missing, the local CA trusted, and the stack started.
    Setup(SetupArgs),

    /// Install a runtime: download it from the vendor and verify it, or use
    /// --from for an archive already on disk.
    Install(InstallArgs),

    /// Remove a runtime from the stack root.
    Uninstall(UninstallArgs),

    /// Databases on the stack's MariaDB or PostgreSQL.
    Db(DbArgs),

    /// The vhosts' TLS certificates: expiry, coverage, issuer; issue them again.
    #[command(subcommand)]
    Cert(CertCommand),

    /// A site's `npm run dev` (or `bun run dev`), run and stopped by devcrate.
    #[command(subcommand)]
    Dev(DevCommand),

    /// Start the stack when you log on to Windows. Off unless switched on.
    #[command(subcommand)]
    Autostart(AutostartCommand),

    /// Create a Laravel or WordPress project: the code, its vhost, its
    /// database, and its .env / wp-config.php filled in.
    New(NewArgs),
}

#[derive(Debug, Args)]
pub struct NewArgs {
    /// laravel or wordpress.
    pub kind: String,
    /// Folder under projects\; the site is <name>.test. Letters, digits, - and . only.
    pub name: String,
    /// PHP version (8.5, 85, php-8.5); defaults to the one php\current names.
    #[arg(long)]
    pub php: Option<String>,
    /// mariadb (default), postgres, or sqlite. WordPress takes mariadb only.
    #[arg(long)]
    pub db: Option<String>,
    /// Do not add the hosts-file entry.
    #[arg(long)]
    pub no_hosts: bool,
    /// Do not issue a certificate for the host.
    #[arg(long)]
    pub no_tls: bool,
}

#[derive(Debug, Args)]
pub struct DbArgs {
    /// Which server: mariadb or postgres.
    #[arg(long, global = true, default_value = "mariadb")]
    pub engine: String,
    #[command(subcommand)]
    pub command: DbCommand,
}

#[derive(Debug, Subcommand)]
pub enum DbCommand {
    /// List the databases (the server's own system databases are left out).
    List,
    /// Create a database: utf8mb4 on MariaDB, UTF8 on PostgreSQL.
    Create {
        /// Letters, digits, _ and - only.
        name: String,
    },
    /// Drop a database and everything in it.
    Drop {
        name: String,
        /// Required: dropping deletes the data.
        #[arg(long)]
        force: bool,
    },
    /// Run an SQL file into a database, creating the database if needed.
    Import { name: String, file: PathBuf },
    /// Dump a database to an SQL file (default: <name>.sql here).
    Export {
        name: String,
        file: Option<PathBuf>,
        /// Overwrite the file if it exists.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum NodeCommand {
    /// List installed Node.js versions.
    List,
    /// Select which installed Node.js version CLI `node` resolves to.
    Use(NodeUseArgs),
}

#[derive(Debug, Args)]
pub struct NodeUseArgs {
    /// Version to select: 22, 22.11.0, or v22.11.0.
    pub version: String,
}

#[derive(Debug, Subcommand)]
pub enum BunCommand {
    /// List installed Bun versions.
    List,
    /// Select which installed Bun version CLI `bun` resolves to.
    Use(BunUseArgs),
}

#[derive(Debug, Args)]
pub struct BunUseArgs {
    /// Version to select: 1.2, 1.2.2, or v1.2.2.
    pub version: String,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Emit JSON instead of a table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the full resolved configuration as TOML.
    ///
    /// Redirect it to devcrate.toml in the stack root to pin what is currently
    /// being discovered from the folder layout.
    Show,
    /// Print the path devcrate.toml is read from, whether or not it exists.
    Path,
}

#[derive(Debug, Args)]
pub struct SetupArgs {
    /// Install and trust, but do not start the stack.
    #[arg(long)]
    pub no_start: bool,
    /// Do not make the Start Menu / Desktop shortcuts and Windows Terminal profile.
    #[arg(long)]
    pub no_shortcuts: bool,
    /// Remove what setup and autostart wrote outside the stack root: the
    /// shortcuts, the Windows Terminal profile, the autostart task.
    #[arg(long, conflicts_with_all = ["no_start", "no_shortcuts"])]
    pub undo: bool,
}

#[derive(Debug, Subcommand)]
pub enum CertCommand {
    /// Every HTTPS vhost's certificate: when it expires, and whether it is fine.
    List,
    /// Issue certificates again with mkcert: the hosts named, every one with
    /// --all, or with neither the ones that need it.
    Renew {
        /// Sites: shop or shop.test.
        hosts: Vec<String>,
        #[arg(long, conflicts_with = "hosts")]
        all: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum DevCommand {
    /// Run the site's `dev` script in its project folder, `npm install` first if there is no node_modules.
    Start {
        /// The site: shop or shop.test.
        site: String,
    },
    /// Stop it, and every process it started.
    Stop { site: String },
    /// The dev servers devcrate started, running or exited.
    List,
}

#[derive(Debug, Subcommand)]
pub enum AutostartCommand {
    /// Run `devcrate start` when this user logs on (a Task Scheduler task; no Administrator).
    On,
    /// Remove the logon task.
    Off,
    /// Whether it is on, and how its last run went.
    Status,
    /// What the logon task runs: start the stack with no console, into logs\autostart.log.
    #[command(hide = true)]
    Run,
}

#[derive(Debug, Subcommand)]
pub enum PhpCommand {
    /// List installed PHP versions and the one the CLI resolves to.
    List,
    /// Switch the CLI PHP version by repointing the php\current junction.
    Use(PhpUseArgs),
    /// A version's extensions: list, enable, disable.
    #[command(subcommand)]
    Ext(PhpExtCommand),
}

#[derive(Debug, Subcommand)]
pub enum PhpExtCommand {
    /// List the version's extensions and where each is switched on.
    List(PhpExtListArgs),
    /// Switch extensions on, in devcrate's block of php.ini, and restart that worker.
    Enable(PhpExtArgs),
    /// Switch off extensions devcrate switched on, and restart that worker.
    Disable(PhpExtArgs),
    /// Download a PECL extension (xdebug) built for this exact PHP, and switch it on.
    Install(PhpExtInstallArgs),
}

#[derive(Debug, Args)]
pub struct PhpExtInstallArgs {
    /// The extension: xdebug, or another PECL extension built for Windows.
    pub extension: String,
    /// PHP version (8.5, 85, php-8.5); defaults to the one php\current names.
    #[arg(long)]
    pub php: Option<String>,
    /// A release of the extension (3.5.3); defaults to the newest with a matching build.
    #[arg(long)]
    pub version: Option<String>,
    /// Replace a DLL already in ext\, the bundled one included.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct PhpExtListArgs {
    /// PHP version (8.5, 85, php-8.5); defaults to the one php\current names.
    #[arg(long)]
    pub php: Option<String>,
}

#[derive(Debug, Args)]
pub struct PhpExtArgs {
    /// Extension names: pgsql, php_pgsql.dll, and "pgsql" all mean the same.
    #[arg(required = true)]
    pub extensions: Vec<String>,
    /// PHP version (8.5, 85, php-8.5); defaults to the one php\current names.
    #[arg(long)]
    pub php: Option<String>,
}

#[derive(Debug, Args)]
pub struct PhpUseArgs {
    /// Version to switch to: 8.5, 85, and php-8.5 all mean the same thing.
    pub version: String,
}

#[derive(Debug, Subcommand)]
pub enum PythonCommand {
    /// List installed Python versions and the one the CLI resolves to.
    List,
    /// Switch the CLI Python version by repointing the python\current junction.
    Use(PythonUseArgs),
}

#[derive(Debug, Args)]
pub struct PythonUseArgs {
    /// Version to switch to: 3.8, 38, and python-3.8 all mean the same thing.
    pub version: String,
}

#[derive(Debug, Subcommand)]
pub enum NginxCommand {
    /// List the nginx versions in the prefix and which one is active.
    List,
    /// Switch the active nginx by repointing the nginx\current junction.
    Use(NginxUseArgs),
    /// Move a pre-restructure stack into the current layout: one stable
    /// prefix holding conf\, logs\, and the versioned builds.
    Migrate(NginxMigrateArgs),
}

#[derive(Debug, Args)]
pub struct NginxUseArgs {
    /// Version to switch to: 1.31.1, nginx-1.31.1, or an unambiguous 1.31.
    pub version: String,
}

#[derive(Debug, Args)]
pub struct NginxMigrateArgs {
    /// Print what would move, and change nothing.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Subcommand)]
pub enum SiteCommand {
    /// List the configured vhosts.
    List,
    /// Add a vhost: web root, conf, nginx reload.
    Add(SiteAddArgs),
    /// Point an existing vhost at another PHP version.
    SetPhp(SiteSetPhpArgs),
    /// Remove a vhost's conf. The project folder is left alone.
    Remove(SiteRemoveArgs),
    /// Serve every folder under projects\ that no vhost serves yet, as
    /// <folder>.test (auto-vhost). Only ever adds.
    Sync(SiteSyncArgs),
}

#[derive(Debug, Args)]
pub struct SiteSyncArgs {
    /// Show what would be added, and write nothing.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct SiteAddArgs {
    /// Hostname, e.g. myapp.test.
    pub host: String,
    /// Optional path to an existing project directory.
    #[arg(long, value_name = "PATH")]
    pub path: Option<PathBuf>,
    /// PHP version to serve it with, e.g. 8.5. Defaults to detection or CLI version.
    #[arg(long)]
    pub php: Option<String>,
    /// Skip automatic hosts file update.
    #[arg(long)]
    pub no_hosts: bool,
    /// Skip automatic TLS certificate generation via mkcert.
    #[arg(long)]
    pub no_tls: bool,
    /// Overwrite the conf if one already exists for this host.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct SiteSetPhpArgs {
    /// Hostname of an existing vhost.
    pub host: String,
    /// Version to serve it with: 8.5, 85, and php-8.5 all mean the same thing.
    pub version: String,
}

#[derive(Debug, Args)]
pub struct SiteRemoveArgs {
    /// Hostname to remove.
    pub host: String,
}

#[derive(Debug, Args)]
pub struct ServiceArgs {
    /// Service id from `devcrate status` (nginx, php-8.5, mariadb, rabbitmq),
    /// or `php` for every PHP version. A version may be spelled 8.5 or 85.
    /// Omit for the whole stack.
    pub service: Option<String>,
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Runtime to install: `php`, `nginx`, `composer`, `mariadb`, `rabbitmq`,
    /// `erlang`, `node`, `bun`, `postgres`, or `python`.
    pub runtime: String,
    /// Version to install: 8.4, 84, and php-8.4 for PHP; 1.31.3 or the series
    /// 1.31 for nginx; a line (stable, lts) or an exact version for Composer; an
    /// exact minor (13.23) for PostgreSQL; 3.8 for Python.
    /// Omitted without --from: list the versions available for download.
    /// Omitted with --from: read from the archive's file name.
    pub version: Option<String>,
    /// Install from an archive already on disk instead of downloading one.
    #[arg(long, value_name = "PATH")]
    pub from: Option<PathBuf>,
    /// Replace an existing installation of the same version.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct UninstallArgs {
    /// Runtime to remove: same names as `install` accepts.
    pub runtime: String,
    /// Version to remove, for the runtimes that have versions (php, nginx,
    /// node, bun, python). Omitted: list what is installed. Ignored for
    /// composer, mariadb, rabbitmq, erlang, and postgres, which have only one.
    pub version: Option<String>,
    /// Also delete the data directory a database service (mariadb, postgres,
    /// rabbitmq) keeps beside its binaries. Irreversible, so it must be paired
    /// with --force; left off, the data directory survives and everything else
    /// is removed.
    #[arg(long)]
    pub data: bool,
    /// Remove even when it is the version `current` points at, a vhost still
    /// names its FastCGI port, or (for erlang) RabbitMQ still depends on it.
    /// Also required together with --data to actually delete a data directory.
    #[arg(long)]
    pub force: bool,
}
