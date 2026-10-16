//! devcrate -- manage a portable, multi-PHP development stack for Windows.
//!
//! Two front ends over one core: the subcommands below, and the dashboard in
//! [`tui`] that runs when none is given. Every batch script in the stack root
//! has an equivalent in both, and `install` downloads and installs PHP, nginx,
//! Composer, MariaDB, RabbitMQ, Erlang/OTP, Node.js, Bun, PostgreSQL, Python,
//! and Redis end to end. `uninstall` is the mirror of it, one runtime at a time.

mod autosite;
mod bun;
mod cli;
mod config;
mod control;
mod create;
mod db;
mod devserver;
mod hosts;
mod integration;
mod ini_block;
mod install;
mod junction;
mod launch;
mod mkcert;
mod nginx;
mod node;
mod php;
mod php_ext;
mod probe;
mod python;
mod root;
mod setup;
mod site;
mod status;
mod term;
mod tui;
mod uninstall;

use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;

use crate::cli::{
    BunCommand, Cli, Command, ConfigCommand, DbCommand, NginxCommand, NodeCommand, PhpCommand, PhpExtCommand,
    PythonCommand, SiteCommand,
};
use crate::config::Stack;

/// Exit codes. 2 is clap's own code for a usage error. 3 used to mean "not
/// built yet, a batch script does this today"; nothing exits 3 any more, but
/// scripts written against it lose nothing by keeping the check.
pub mod exit {
    pub const OK: u8 = 0;
    pub const ERROR: u8 = 1;
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            eprintln!("devcrate: {err:#}");
            ExitCode::from(exit::ERROR)
        }
    }
}

fn run(cli: Cli) -> Result<u8> {
    let stack = Stack::open(root::resolve(cli.root)?)?;
    // Version lists are saved here as they are fetched, and read back offline.
    install::set_catalogue_cache(stack.root.join("_downloads").join("catalogues"));

    let Some(command) = cli.command else {
        return tui::run(stack);
    };

    match command {
        Command::Status(args) => {
            let report = status::report(&stack);
            if args.json {
                status::print_json(&report)?;
            } else {
                status::print_text(&report);
            }
            Ok(exit::OK)
        }

        Command::Config(ConfigCommand::Show) => {
            print!("{}", config_toml(&stack)?);
            Ok(exit::OK)
        }
        Command::Config(ConfigCommand::Path) => {
            println!("{}", stack.root.join(root::CONFIG_FILE).display());
            Ok(exit::OK)
        }

        Command::Php(PhpCommand::List) => {
            php_list(&stack);
            Ok(exit::OK)
        }
        Command::Php(PhpCommand::Use(args)) => php::switch(&stack, &args.version),
        Command::Php(PhpCommand::Ext(PhpExtCommand::List(args))) => {
            php_ext::print_list(&stack, args.php.as_deref())
        }
        Command::Php(PhpCommand::Ext(PhpExtCommand::Enable(args))) => {
            php_ext::print_change(&stack, args.php.as_deref(), &args.extensions, true)
        }
        Command::Php(PhpCommand::Ext(PhpExtCommand::Disable(args))) => {
            php_ext::print_change(&stack, args.php.as_deref(), &args.extensions, false)
        }
        Command::Php(PhpCommand::Ext(PhpExtCommand::Install(args))) => php_ext::print_install(
            &stack,
            args.php.as_deref(),
            &args.extension,
            args.version.as_deref(),
            args.force,
        ),

        Command::Python(PythonCommand::List) => python::list(&stack),
        Command::Python(PythonCommand::Use(args)) => python::switch(&stack, &args.version),

        Command::Nginx(NginxCommand::List) => nginx::list(&stack),
        Command::Nginx(NginxCommand::Use(args)) => nginx::switch(&stack, &args.version),
        Command::Nginx(NginxCommand::Migrate(args)) => {
            nginx::migrate_command(&stack, args.dry_run)
        }

        Command::Site(SiteCommand::List) => {
            site_list(&stack);
            Ok(exit::OK)
        }
        Command::Site(SiteCommand::Add(args)) => {
            site::add(
                &stack,
                &args.host,
                args.path.as_deref(),
                args.php.as_deref(),
                args.no_hosts,
                args.no_tls,
                args.force,
            )
        }
        Command::Site(SiteCommand::SetPhp(args)) => {
            site::set_php(&stack, &args.host, &args.version)
        }
        Command::Site(SiteCommand::Remove(args)) => site::remove(&stack, &args.host),
        Command::Site(SiteCommand::Sync(args)) => autosite::print(&stack, args.dry_run),

        Command::Node(NodeCommand::List) => {
            node_list(&stack);
            Ok(exit::OK)
        }
        Command::Node(NodeCommand::Use(args)) => node::switch(&stack, &args.version),

        Command::Bun(BunCommand::List) => {
            bun_list(&stack);
            Ok(exit::OK)
        }
        Command::Bun(BunCommand::Use(args)) => bun::switch(&stack, &args.version),

        Command::Start(args) => control::start(&stack, args.service.as_deref()),
        Command::Stop(args) => control::stop(&stack, args.service.as_deref()),
        Command::Restart(args) => control::restart(&stack, args.service.as_deref()),
        Command::Setup(args) if args.undo => integration::print_undo(&stack),
        Command::Setup(args) => setup::print(&stack, !args.no_start, !args.no_shortcuts),
        Command::Autostart(cli::AutostartCommand::On) => integration::print_autostart(&stack, "on"),
        Command::Autostart(cli::AutostartCommand::Off) => integration::print_autostart(&stack, "off"),
        Command::Autostart(cli::AutostartCommand::Status) => integration::print_autostart(&stack, "status"),
        Command::Autostart(cli::AutostartCommand::Run) => integration::run_at_logon(&stack),
        Command::New(args) => {
            let database = args.db.as_deref().map(create::Database::parse).transpose()?;
            let request = create::Request {
                kind: create::Kind::parse(&args.kind)?,
                name: &args.name,
                php: args.php.as_deref(),
                database,
                no_hosts: args.no_hosts,
                no_tls: args.no_tls,
                output: create::Output::Inherit,
            };
            create::print(&stack, &request)
        }
        Command::Install(args) => install::install(
            &stack,
            &args.runtime,
            args.version.as_deref(),
            args.from.as_deref(),
            args.force,
        ),
        Command::Uninstall(args) => uninstall::uninstall(
            &stack,
            &args.runtime,
            args.version.as_deref(),
            args.data,
            args.force,
        ),
        Command::Dev(cli::DevCommand::Start { site }) => devserver::print_start(&stack, &site),
        Command::Dev(cli::DevCommand::Stop { site }) => devserver::print_stop(&stack, &site),
        Command::Dev(cli::DevCommand::List) => devserver::print_list(&stack),
        Command::Db(args) => {
            let engine = db::Engine::parse(&args.engine)?;
            let action = match &args.command {
                DbCommand::List => db::Action::List,
                DbCommand::Create { name } => db::Action::Create(name),
                DbCommand::Drop { name, force } => db::Action::Drop { name, force: *force },
                DbCommand::Import { name, file } => db::Action::Import { name, file },
                DbCommand::Export { name, file, force } => {
                    db::Action::Export { name, file: file.as_deref(), force: *force }
                }
            };
            db::run(&stack, engine, action)
        }
    }
}

fn node_list(stack: &Stack) {
    let installed = node::list_installed(stack);
    if installed.is_empty() {
        println!("No Node.js versions installed under node\\");
        return;
    }
    println!("Node.js versions:");
    for v in installed {
        let active = if v.active { " (active)" } else { "" };
        println!("  v{}{active}", v.version);
    }
}

fn bun_list(stack: &Stack) {
    let installed = bun::list_installed(stack);
    if installed.is_empty() {
        println!("No Bun versions installed under bun\\");
        return;
    }
    println!("Bun versions:");
    for v in installed {
        let active = if v.active { " (active)" } else { "" };
        println!("  v{}{active}", v.version);
    }
}

fn config_toml(stack: &Stack) -> Result<String> {
    let header = format!(
        "# Devcrate configuration for {}\n\
         # Generated by `devcrate config show` from the folder layout.\n\
         # Save this as devcrate.toml in the stack root to pin these values.\n\n",
        stack.root.display()
    );
    Ok(header + &toml::to_string_pretty(&stack.to_config())?)
}

fn php_list(stack: &Stack) {
    let current = stack.current_php();
    let current_name =
        current.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());

    let services: Vec<_> = stack.php_services().collect();
    if services.is_empty() {
        println!("No PHP versions found in {}", stack.rel(&stack.php_dir));
        return;
    }

    let width = services.iter().map(|p| p.id.chars().count()).max().unwrap_or(0);
    for php in services {
        let is_current = current_name.as_deref() == Some(php.id.as_str());
        let active = if is_current { "*" } else { " " };
        let port = php.ports.first().map(|p| p.to_string()).unwrap_or_else(|| "-".into());
        let state = if php.is_installed() { "" } else { "  (php-cgi.exe missing)" };
        let row = format!(
            "{active} {:<8}  {:<width$}  fastcgi {port}{state}",
            php.name, php.id
        );
        match is_current {
            true => println!("{}", term::paint(&row, term::Color::Green)),
            false => println!("{row}"),
        }
    }

    match current_name {
        Some(name) => println!("\n* = php\\current -> {name} (what the CLI resolves to)"),
        None => println!("\nphp\\current is not set; run `devcrate php use 8.5` to create it"),
    }
}

fn site_list(stack: &Stack) {
    let confs = stack.sites();
    if confs.is_empty() {
        println!("No vhosts in {}", stack.rel(&stack.sites_dir()));
        return;
    }

    let sites: Vec<site::Site> = confs.iter().map(|c| site::Site::read(c)).collect();
    let width = sites.iter().map(|s| s.host.chars().count()).max().unwrap_or(0);

    for site in &sites {
        let php = site
            .fastcgi_port
            .map(|p| format!("php {p}"))
            .unwrap_or_else(|| "no fastcgi_pass".into());
        let root = site.root.as_deref().unwrap_or("no root directive");
        println!("  {:<width$}  {php:<10}  {root}", site.host);
    }
    println!("\n{} vhost(s) in {}", sites.len(), stack.rel(&stack.sites_dir()));
}
