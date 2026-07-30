//! The command surface.
//!
//! The whole surface was declared here before most of it was built, so the
//! shape was settled once and `devcrate --help` told the truth about what did
//! and did not work yet. By now every declared command is implemented; what
//! remains unbuilt (installing MariaDB, RabbitMQ, or Erlang) says so when
//! named.
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

    /// Install a runtime: download it from the vendor and verify it, or use
    /// --from for an archive already on disk.
    Install(InstallArgs),
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

#[derive(Debug, Subcommand)]
pub enum PhpCommand {
    /// List installed PHP versions and the one the CLI resolves to.
    List,
    /// Switch the CLI PHP version by repointing the php\current junction.
    Use(PhpUseArgs),
}

#[derive(Debug, Args)]
pub struct PhpUseArgs {
    /// Version to switch to: 8.5, 85, and php-8.5 all mean the same thing.
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
    /// Runtime to install: `php`, `nginx`, `composer`, `mariadb`, `rabbitmq`, or `erlang`.
    pub runtime: String,
    /// Version to install: 8.4, 84, and php-8.4 for PHP; 1.31.3 or the series
    /// 1.31 for nginx; a line (stable, lts) or an exact version for Composer.
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
