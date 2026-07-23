//! The command surface.
//!
//! The whole surface is declared here even where the implementation is still a
//! batch file, so the shape is settled once and `devcrate --help` tells the
//! truth about what does and does not work yet. Commands that are not built
//! print the script that does the job today and exit with
//! [`exit::NOT_IMPLEMENTED`](crate::exit::NOT_IMPLEMENTED).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "devcrate",
    version,
    about = "A portable, multi-PHP development stack for Windows",
    long_about = "Manage a Devcrate stack: nginx, PHP FastCGI workers, MariaDB, and RabbitMQ \
                  running out of a single stack-root folder.\n\n\
                  The stack root is taken from --root, then DEVCRATE_HOME, then the folder \
                  holding this executable, then the working directory (searching upward from \
                  each).",
    arg_required_else_help = true
)]
pub struct Cli {
    /// Stack root to operate on (overrides DEVCRATE_HOME and auto-detection).
    #[arg(long, global = true, value_name = "PATH")]
    pub root: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
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

    /// Vhosts.
    #[command(subcommand)]
    Site(SiteCommand),

    /// Start the stack, or one service. (Not built yet -- use start.bat.)
    Start(ServiceArgs),

    /// Stop the stack, or one service. (Not built yet -- use stop.bat.)
    Stop(ServiceArgs),

    /// Restart the stack, or one service. (Not built yet.)
    Restart(ServiceArgs),

    /// Download and install a runtime. (Not built yet -- see docs/installation.md.)
    Install(InstallArgs),
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
    /// Switch the CLI PHP version. (Not built yet -- use phpuse.)
    Use(PhpUseArgs),
}

#[derive(Debug, Args)]
pub struct PhpUseArgs {
    /// Version to switch to, e.g. 8.5 or 85.
    pub version: String,
}

#[derive(Debug, Subcommand)]
pub enum SiteCommand {
    /// List the configured vhosts.
    List,
    /// Add a vhost. (Not built yet -- use new-vhost.bat.)
    Add(SiteAddArgs),
    /// Remove a vhost. (Not built yet.)
    Remove(SiteRemoveArgs),
}

#[derive(Debug, Args)]
pub struct SiteAddArgs {
    /// Hostname, e.g. myapp.test.
    pub host: String,
    /// PHP version to serve it with, e.g. 8.5.
    #[arg(long)]
    pub php: Option<String>,
}

#[derive(Debug, Args)]
pub struct SiteRemoveArgs {
    /// Hostname to remove.
    pub host: String,
}

#[derive(Debug, Args)]
pub struct ServiceArgs {
    /// Service id from `devcrate status` (nginx, php85, mariadb, rabbitmq).
    /// Omit for the whole stack.
    pub service: Option<String>,
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Runtime to install: php, nginx, mariadb, rabbitmq, erlang, composer.
    pub runtime: String,
    /// Version to install; omit for the latest.
    pub version: Option<String>,
}
