//! `devcrate.toml` -- the file format -- and the resolved view of a stack.
//!
//! The config file is entirely optional and every key in it is optional too. A
//! stack built by the batch scripts has no `devcrate.toml` at all, so anything
//! missing is discovered from the layout on disk instead: the `nginx-*`
//! directory, and one PHP entry per `php\php*` folder. That keeps the binary
//! read-compatible with the stack as it exists today.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::root::{self, CONFIG_FILE, StackRoot};

// ---------------------------------------------------------------------------
// File format
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub nginx: NginxConfig,
    #[serde(default)]
    pub mariadb: MariaDbConfig,
    #[serde(default)]
    pub rabbitmq: RabbitMqConfig,
    /// One table per installed PHP version. Empty means "discover them".
    #[serde(default)]
    pub php: Vec<PhpConfig>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NginxConfig {
    /// Directory holding `nginx.exe` and `conf\`, relative to the stack root.
    pub dir: Option<PathBuf>,
    pub ports: Option<Vec<u16>>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MariaDbConfig {
    pub dir: Option<PathBuf>,
    pub port: Option<u16>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RabbitMqConfig {
    pub dir: Option<PathBuf>,
    /// Erlang lives in its own directory; the broker runs as `erl.exe` from it.
    pub erlang_dir: Option<PathBuf>,
    pub port: Option<u16>,
    pub management_port: Option<u16>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PhpConfig {
    /// Display version, e.g. `"8.5"`. Derived from `dir` when absent.
    pub version: Option<String>,
    /// Directory holding `php.exe` / `php-cgi.exe`, relative to the stack root.
    pub dir: PathBuf,
    pub fastcgi_port: Option<u16>,
}

impl Config {
    /// Load `<root>\devcrate.toml` if it is there.
    pub fn load(root: &Path) -> Result<(Config, Option<PathBuf>)> {
        let path = root.join(CONFIG_FILE);
        if !path.is_file() {
            return Ok((Config::default(), None));
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let config: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok((config, Some(path)))
    }
}

// ---------------------------------------------------------------------------
// Resolved stack
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    Nginx,
    Php,
    MariaDb,
    RabbitMq,
}

impl ServiceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ServiceKind::Nginx => "nginx",
            ServiceKind::Php => "php",
            ServiceKind::MariaDb => "mariadb",
            ServiceKind::RabbitMq => "rabbitmq",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Service {
    /// Stable identifier for scripting: `nginx`, `php74`, `mariadb`, `rabbitmq`.
    pub id: String,
    /// Human label: `nginx`, `PHP 7.4`, ...
    pub name: String,
    pub kind: ServiceKind,
    /// The file whose presence means "this service is installed".
    pub install_marker: PathBuf,
    /// A running process counts as this service when its executable lives at,
    /// or under, this path. Matching on the path rather than the image name is
    /// what keeps someone else's `nginx.exe` from being reported as ours.
    pub process_prefix: PathBuf,
    /// Image names to ignore inside `process_prefix` -- helpers that outlive
    /// the service and would otherwise read as "still running".
    pub exclude_names: Vec<String>,
    pub ports: Vec<u16>,
}

impl Service {
    pub fn is_installed(&self) -> bool {
        self.install_marker.exists()
    }
}

#[derive(Debug)]
pub struct Stack {
    pub root: PathBuf,
    pub root_source: root::RootSource,
    /// `Some` when a `devcrate.toml` was read; `None` when defaults were used.
    pub config_path: Option<PathBuf>,
    pub nginx_dir: PathBuf,
    pub php_dir: PathBuf,
    pub services: Vec<Service>,
}

impl Stack {
    /// Build the resolved view: config file where present, disk layout elsewhere.
    pub fn open(root: StackRoot) -> Result<Stack> {
        let (config, config_path) = Config::load(&root.path)?;
        let base = root.path.clone();

        let nginx_dir = match &config.nginx.dir {
            Some(dir) => base.join(dir),
            None => root::nginx_dir(&base).unwrap_or_else(|| base.join("nginx-1.31.1")),
        };
        let php_dir = base.join("php");

        let mut services = Vec::new();

        services.push(Service {
            id: "nginx".into(),
            name: "nginx".into(),
            kind: ServiceKind::Nginx,
            install_marker: nginx_dir.join("nginx.exe"),
            process_prefix: nginx_dir.join("nginx.exe"),
            exclude_names: Vec::new(),
            ports: config.nginx.ports.clone().unwrap_or_else(|| vec![80, 443]),
        });

        for php in resolve_php(&config, &php_dir) {
            services.push(php);
        }

        let mariadb_dir =
            base.join(config.mariadb.dir.clone().unwrap_or_else(|| PathBuf::from("mariadb")));
        services.push(Service {
            id: "mariadb".into(),
            name: "MariaDB".into(),
            kind: ServiceKind::MariaDb,
            install_marker: mariadb_dir.join("bin").join("mariadbd.exe"),
            process_prefix: mariadb_dir.join("bin").join("mariadbd.exe"),
            exclude_names: Vec::new(),
            ports: vec![config.mariadb.port.unwrap_or(3306)],
        });

        let rabbit_dir =
            base.join(config.rabbitmq.dir.clone().unwrap_or_else(|| PathBuf::from("rabbitmq")));
        let erlang_dir =
            base.join(config.rabbitmq.erlang_dir.clone().unwrap_or_else(|| PathBuf::from("erlang")));
        services.push(Service {
            id: "rabbitmq".into(),
            name: "RabbitMQ".into(),
            kind: ServiceKind::RabbitMq,
            install_marker: rabbit_dir.join("sbin").join("rabbitmq-server.bat"),
            // The broker is an Erlang node: what actually runs is erl.exe out of
            // the stack's own erlang\ directory, not anything under rabbitmq\.
            process_prefix: erlang_dir,
            // stop.bat kills epmd separately for a reason: the port mapper
            // outlives the broker, so counting it would report a stopped
            // RabbitMQ as running.
            exclude_names: vec!["epmd.exe".into()],
            ports: vec![
                config.rabbitmq.port.unwrap_or(5672),
                config.rabbitmq.management_port.unwrap_or(15672),
            ],
        });

        Ok(Stack {
            root: base,
            root_source: root.source,
            config_path,
            nginx_dir,
            php_dir,
            services,
        })
    }

    /// `<nginx>\conf\sites`, where the per-vhost confs live.
    pub fn sites_dir(&self) -> PathBuf {
        self.nginx_dir.join("conf").join("sites")
    }

    /// The vhost confs, sorted by file name.
    pub fn sites(&self) -> Vec<PathBuf> {
        let mut sites: Vec<PathBuf> = std::fs::read_dir(self.sites_dir())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("conf")))
            .collect();
        sites.sort();
        sites
    }

    /// Which PHP version `php\current` points at -- the version the CLI resolves
    /// to, since that junction is what sits on `PATH`.
    pub fn current_php(&self) -> Option<PathBuf> {
        let current = self.php_dir.join("current");
        std::fs::read_link(&current).ok().or_else(|| root::clean(&current).ok())
    }

    pub fn by_kind(&self, kind: ServiceKind) -> Option<&Service> {
        self.services.iter().find(|s| s.kind == kind)
    }

    pub fn php_services(&self) -> impl Iterator<Item = &Service> {
        self.services.iter().filter(|s| s.kind == ServiceKind::Php)
    }

    /// Root-relative path for display. Config files may spell directories with
    /// forward slashes; Windows accepts either, but printing a mix of both in
    /// one path looks like a bug.
    pub fn rel(&self, path: &Path) -> String {
        root::display_relative(path, &self.root).replace('/', "\\")
    }

    /// Root-relative path with forward slashes, the way it is written in
    /// `devcrate.toml`.
    fn rel_path(&self, path: &Path) -> PathBuf {
        PathBuf::from(self.rel(path).replace('\\', "/"))
    }

    /// The resolved stack expressed in the config file's own format -- what
    /// `devcrate config show` prints, so discovered values can be pinned.
    pub fn to_config(&self) -> Config {
        let mut config = Config::default();

        config.nginx.dir = Some(self.rel_path(&self.nginx_dir));
        if let Some(nginx) = self.by_kind(ServiceKind::Nginx) {
            config.nginx.ports = Some(nginx.ports.clone());
        }

        if let Some(mariadb) = self.by_kind(ServiceKind::MariaDb) {
            config.mariadb.dir = ancestor(&mariadb.install_marker, 2).map(|d| self.rel_path(d));
            config.mariadb.port = mariadb.ports.first().copied();
        }

        if let Some(rabbit) = self.by_kind(ServiceKind::RabbitMq) {
            config.rabbitmq.dir = ancestor(&rabbit.install_marker, 2).map(|d| self.rel_path(d));
            config.rabbitmq.erlang_dir = Some(self.rel_path(&rabbit.process_prefix));
            config.rabbitmq.port = rabbit.ports.first().copied();
            config.rabbitmq.management_port = rabbit.ports.get(1).copied();
        }

        config.php = self
            .php_services()
            .map(|php| PhpConfig {
                version: Some(php.name.trim_start_matches("PHP ").to_string()),
                dir: ancestor(&php.install_marker, 1)
                    .map(|d| self.rel_path(d))
                    .unwrap_or_default(),
                fastcgi_port: php.ports.first().copied(),
            })
            .collect();

        config
    }
}

/// Walk `levels` directories up from a file path.
fn ancestor(path: &Path, levels: usize) -> Option<&Path> {
    path.ancestors().nth(levels)
}

/// PHP entries from the config, or one per `php\php*` directory on disk.
fn resolve_php(config: &Config, php_dir: &Path) -> Vec<Service> {
    let root = php_dir.parent().unwrap_or(php_dir);

    if !config.php.is_empty() {
        return config
            .php
            .iter()
            .map(|entry| {
                let dir = root.join(&entry.dir);
                let tag = dir_tag(&dir);
                let version =
                    entry.version.clone().unwrap_or_else(|| version_from_tag(&tag));
                php_service(&dir, &tag, &version, entry.fastcgi_port.or_else(|| port_from_tag(&tag)))
            })
            .collect();
    }

    let mut dirs: Vec<PathBuf> = std::fs::read_dir(php_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        // `current` is the CLI junction, not a version.
        .filter(|p| p.is_dir() && dir_tag(p).starts_with("php") && dir_tag(p) != "phpcurrent")
        .collect();
    dirs.sort();

    dirs.iter()
        .map(|dir| {
            let tag = dir_tag(dir);
            php_service(dir, &tag, &version_from_tag(&tag), port_from_tag(&tag))
        })
        .collect()
}

fn php_service(dir: &Path, tag: &str, version: &str, port: Option<u16>) -> Service {
    Service {
        id: tag.to_string(),
        name: format!("PHP {version}"),
        kind: ServiceKind::Php,
        install_marker: dir.join("php-cgi.exe"),
        process_prefix: dir.join("php-cgi.exe"),
        exclude_names: Vec::new(),
        ports: port.into_iter().collect(),
    }
}

fn dir_tag(dir: &Path) -> String {
    dir.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase()
}

/// `php74` -> `7.4`, `php85` -> `8.5`.
fn version_from_tag(tag: &str) -> String {
    let digits: String = tag.chars().filter(|c| c.is_ascii_digit()).collect();
    match digits.len() {
        0 => tag.to_string(),
        1 => digits,
        _ => format!("{}.{}", &digits[..1], &digits[1..]),
    }
}

/// The FastCGI port convention the batch scripts use: `90` + the version digits,
/// so `php74` listens on 9074. Anything that does not fit in a port number gets
/// no default and has to be spelled out in `devcrate.toml`.
fn port_from_tag(tag: &str) -> Option<u16> {
    let digits: String = tag.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    format!("90{digits}").parse().ok()
}

impl fmt::Display for ServiceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_come_from_directory_names() {
        assert_eq!(version_from_tag("php74"), "7.4");
        assert_eq!(version_from_tag("php85"), "8.5");
        assert_eq!(version_from_tag("php810"), "8.10");
    }

    #[test]
    fn fastcgi_ports_follow_the_batch_script_convention() {
        assert_eq!(port_from_tag("php74"), Some(9074));
        assert_eq!(port_from_tag("php82"), Some(9082));
        assert_eq!(port_from_tag("php85"), Some(9085));
        // 90 + 810 overflows a port number, so there is no sensible default.
        assert_eq!(port_from_tag("php810"), None);
        assert_eq!(port_from_tag("php"), None);
    }
}
