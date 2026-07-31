//! `devcrate.toml` -- the file format -- and the resolved view of a stack.
//!
//! The config file is entirely optional and every key in it is optional too. A
//! stack built by the batch scripts has no `devcrate.toml` at all, so anything
//! missing is discovered from the layout on disk instead: the nginx prefix, and
//! one PHP entry per `php\php-*` folder. That keeps the binary read-compatible
//! with the stack as it exists today, in either nginx layout.

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
    pub postgres: PostgresConfig,
    #[serde(default)]
    pub rabbitmq: RabbitMqConfig,
    /// One table per installed PHP version. Empty means "discover them".
    #[serde(default)]
    pub php: Vec<PhpConfig>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NginxConfig {
    /// The nginx prefix, relative to the stack root: the directory holding
    /// `conf\`, `logs\`, and `temp\`, which is what `nginx -p` is given.
    ///
    /// Which *binary* runs is not configured here. It is whichever version the
    /// `current` junction inside this directory names, so switching versions
    /// stays a junction rewrite rather than a config edit -- and pinning a
    /// stack that still keeps `nginx.exe` beside `conf\` needs no extra key,
    /// since that is one of the places the binary is looked for.
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
pub struct PostgresConfig {
    /// Directory holding `bin\postgres.exe` and the `data\` cluster, relative to
    /// the stack root. One version at a time, unlike PHP: a PostgreSQL data
    /// directory is written in a version-specific on-disk format, so the builds
    /// cannot coexist the way `php-7.4` and `php-8.5` do.
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
    Postgres,
    RabbitMq,
    Node,
    Bun,
}

impl ServiceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ServiceKind::Nginx => "nginx",
            ServiceKind::Php => "php",
            ServiceKind::MariaDb => "mariadb",
            ServiceKind::Postgres => "postgres",
            ServiceKind::RabbitMq => "rabbitmq",
            ServiceKind::Node => "node",
            ServiceKind::Bun => "bun",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Service {
    /// Stable identifier for scripting: `nginx`, `php-7.4`, `mariadb`,
    /// `rabbitmq`. It is the directory name, so it follows a rename -- which is
    /// why every command that takes one also accepts the version digits alone.
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
        if self.kind == ServiceKind::Node || self.kind == ServiceKind::Bun {
            if self.install_marker.exists() {
                return true;
            }
            if let Ok(entries) = std::fs::read_dir(&self.process_prefix) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with('v') && entry.path().is_dir() {
                        return true;
                    }
                }
            }
            false
        } else {
            self.install_marker.exists()
        }
    }
}

#[derive(Debug)]
pub struct Stack {
    pub root: PathBuf,
    pub root_source: root::RootSource,
    /// `Some` when a `devcrate.toml` was read; `None` when defaults were used.
    pub config_path: Option<PathBuf>,
    /// What `nginx -p` is given: `conf\`, `logs\`, `temp\`, and the `projects`
    /// junction all hang off this, and it stays put across a version switch.
    pub nginx_prefix: PathBuf,
    /// The `nginx.exe` that would be run, resolved through `current` where
    /// there is one. Points at the conventional spot when nothing is installed,
    /// so it doubles as the "not installed" marker.
    pub nginx_bin: PathBuf,
    pub php_dir: PathBuf,
    /// `python\`, holding one `python-<X.Y>` directory per installed version and
    /// the `current` junction naming the one on `PATH`. Python is a toolchain,
    /// not a service, so it lives here rather than in `services`.
    pub python_dir: PathBuf,
    pub services: Vec<Service>,
}

impl Stack {
    /// Build the resolved view: config file where present, disk layout elsewhere.
    pub fn open(root: StackRoot) -> Result<Stack> {
        let (config, config_path) = Config::load(&root.path)?;
        let base = root.path.clone();

        let nginx_prefix = match &config.nginx.dir {
            Some(dir) => base.join(dir),
            None => root::nginx_prefix(&base).unwrap_or_else(|| base.join("nginx")),
        };
        let nginx_bin =
            root::nginx_exe(&nginx_prefix).unwrap_or_else(|| nginx_prefix.join("nginx.exe"));
        let php_dir = base.join("php");
        let python_dir = base.join("python");

        let mut services = Vec::new();

        services.push(Service {
            id: "nginx".into(),
            name: "nginx".into(),
            kind: ServiceKind::Nginx,
            install_marker: nginx_bin.clone(),
            // The prefix directory, not the binary. Any nginx.exe under the
            // prefix is this stack's -- which keeps a running nginx findable
            // after `nginx use` repoints `current` at a different version, and
            // still cannot match an nginx installed anywhere else.
            process_prefix: nginx_prefix.clone(),
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

        let postgres_dir =
            base.join(config.postgres.dir.clone().unwrap_or_else(|| PathBuf::from("postgres")));
        services.push(Service {
            id: "postgres".into(),
            name: "PostgreSQL".into(),
            kind: ServiceKind::Postgres,
            // The server binary, run directly rather than through pg_ctl: pg_ctl
            // forks the postmaster and exits, so matching on its path would never
            // find the process that is actually serving.
            install_marker: postgres_dir.join("bin").join("postgres.exe"),
            process_prefix: postgres_dir.join("bin").join("postgres.exe"),
            exclude_names: Vec::new(),
            ports: vec![config.postgres.port.unwrap_or(5432)],
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

        let node_dir = base.join("node");
        services.push(Service {
            id: "node".into(),
            name: "Node.js".into(),
            kind: ServiceKind::Node,
            install_marker: node_dir.join("current").join("node.exe"),
            process_prefix: node_dir,
            exclude_names: Vec::new(),
            ports: Vec::new(),
        });

        let bun_dir = base.join("bun");
        services.push(Service {
            id: "bun".into(),
            name: "Bun".into(),
            kind: ServiceKind::Bun,
            install_marker: bun_dir.join("current").join("bun.exe"),
            process_prefix: bun_dir,
            exclude_names: Vec::new(),
            ports: Vec::new(),
        });

        Ok(Stack {
            root: base,
            root_source: root.source,
            config_path,
            nginx_prefix,
            nginx_bin,
            php_dir,
            python_dir,
            services,
        })
    }

    /// `<prefix>\conf\sites`, where the per-vhost confs live.
    pub fn sites_dir(&self) -> PathBuf {
        self.nginx_prefix.join("conf").join("sites")
    }

    /// Which nginx version `nginx\current` points at, if the stack names one.
    pub fn current_nginx(&self) -> Option<PathBuf> {
        crate::junction::target(&self.nginx_prefix.join("current"))
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
        crate::junction::target(&self.php_dir.join("current"))
    }

    /// Which Python version `python\current` points at -- the version the CLI
    /// resolves to, since that junction is what sits on `PATH`.
    pub fn current_python(&self) -> Option<PathBuf> {
        crate::junction::target(&self.python_dir.join("current"))
    }

    /// The installed Python version directories, `python\python-*`, sorted.
    /// `current` is excluded -- it is the junction naming one of these, not a
    /// version of its own, exactly as `php\current` is.
    pub fn python_versions(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&self.python_dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                let name =
                    path.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
                path.is_dir() && name.starts_with("python") && name != "current"
            })
            .collect();
        dirs.sort();
        dirs
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

        config.nginx.dir = Some(self.rel_path(&self.nginx_prefix));
        if let Some(nginx) = self.by_kind(ServiceKind::Nginx) {
            config.nginx.ports = Some(nginx.ports.clone());
        }

        if let Some(mariadb) = self.by_kind(ServiceKind::MariaDb) {
            config.mariadb.dir = ancestor(&mariadb.install_marker, 2).map(|d| self.rel_path(d));
            config.mariadb.port = mariadb.ports.first().copied();
        }

        if let Some(postgres) = self.by_kind(ServiceKind::Postgres) {
            config.postgres.dir = ancestor(&postgres.install_marker, 2).map(|d| self.rel_path(d));
            config.postgres.port = postgres.ports.first().copied();
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
        // `current` is the CLI junction, not a version, and the downloaded
        // archives sitting beside these folders are files rather than dirs.
        .filter(|p| p.is_dir() && dir_tag(p).starts_with("php") && dir_tag(p) != "current")
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

/// `php-7.4` -> `7.4`, `php85` -> `8.5`.
///
/// Reading the digits rather than the punctuation is what let the directories
/// be renamed from `php85` to `php-8.5` without touching any of this.
pub(crate) fn version_from_tag(tag: &str) -> String {
    let digits: String = tag.chars().filter(|c| c.is_ascii_digit()).collect();
    match digits.len() {
        0 => tag.to_string(),
        1 => digits,
        _ => format!("{}.{}", &digits[..1], &digits[1..]),
    }
}

/// The FastCGI port convention the batch scripts use: `90` + the version digits,
/// so `php-7.4` listens on 9074. Anything that does not fit in a port number
/// gets no default and has to be spelled out in `devcrate.toml`.
pub(crate) fn port_from_tag(tag: &str) -> Option<u16> {
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
        assert_eq!(version_from_tag("php-7.4"), "7.4");
        assert_eq!(version_from_tag("php-8.5"), "8.5");
        assert_eq!(version_from_tag("php-8.10"), "8.10");
        // The same digit rule names Python folders: `python-3.8` -> `3.8`, which
        // is what makes unpacking `python-3.8\` the whole of installing one.
        assert_eq!(version_from_tag("python-3.8"), "3.8");
        assert_eq!(version_from_tag("python-3.12"), "3.12");
    }

    /// The folders were called `php85` before they were called `php-8.5`, and
    /// nothing stops someone unpacking a stack that still uses the old names.
    #[test]
    fn the_previous_directory_naming_still_reads() {
        assert_eq!(version_from_tag("php74"), version_from_tag("php-7.4"));
        assert_eq!(port_from_tag("php85"), port_from_tag("php-8.5"));
    }

    #[test]
    fn fastcgi_ports_follow_the_batch_script_convention() {
        assert_eq!(port_from_tag("php-7.4"), Some(9074));
        assert_eq!(port_from_tag("php-8.2"), Some(9082));
        assert_eq!(port_from_tag("php-8.5"), Some(9085));
        // 90 + 810 overflows a port number, so there is no sensible default.
        assert_eq!(port_from_tag("php-8.10"), None);
        assert_eq!(port_from_tag("php"), None);
    }
}
