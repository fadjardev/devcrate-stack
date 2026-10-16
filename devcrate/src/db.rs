//! `devcrate db` -- the databases on the stack's MariaDB and PostgreSQL.
//!
//! Every action is a call to the server's own client from its `bin\`
//! (`mariadb` / `mariadb-dump`, `psql` / `pg_dump`), over TCP to 127.0.0.1 on
//! the service's configured port, as the superuser the stack sets up: MariaDB
//! `root` with no password, PostgreSQL `postgres` with the password its
//! install prints (`postgres`, unless `PGPASSWORD` says otherwise). Nothing
//! here speaks either wire protocol itself -- the clients already do, and
//! they report errors in the words their users know.
//!
//! A database name is limited to letters, digits, `_`, and `-`. That is
//! narrower than either server allows, and it is what lets a name be put into
//! a statement inside the server's identifier quotes (a backtick, a double
//! quote) with no escaping to get wrong: none of the allowed characters can
//! close them. `-` is in because real projects use it (`odoo-backend-test`). The servers' own
//! databases (`mysql`, `postgres`, the templates, ...) are never dropped.
//!
//! The core returns results and never prints; [`run`] is the CLI's wrapper.

use std::fs::File;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use crate::config::{ServiceKind, Stack};
use crate::exit;

/// Which server a command talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    MariaDb,
    Postgres,
}

impl Engine {
    pub fn parse(name: &str) -> Result<Engine> {
        match name.trim().to_ascii_lowercase().as_str() {
            "mariadb" | "mysql" => Ok(Engine::MariaDb),
            "postgres" | "postgresql" | "pgsql" | "pg" => Ok(Engine::Postgres),
            other => bail!("{other:?} is not a database engine devcrate manages (mariadb, postgres)"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Engine::MariaDb => "MariaDB",
            Engine::Postgres => "PostgreSQL",
        }
    }

    fn kind(self) -> ServiceKind {
        match self {
            Engine::MariaDb => ServiceKind::MariaDb,
            Engine::Postgres => ServiceKind::Postgres,
        }
    }

    /// The servers' own databases: listed by neither `list` nor touched by `drop`.
    fn is_system(self, name: &str) -> bool {
        let system: &[&str] = match self {
            Engine::MariaDb => &["mysql", "information_schema", "performance_schema", "sys"],
            Engine::Postgres => &["postgres", "template0", "template1"],
        };
        system.iter().any(|s| s.eq_ignore_ascii_case(name))
    }
}

/// A database name devcrate will pass to a server: 1-63 of `[A-Za-z0-9_-]`.
pub fn check_name(name: &str) -> Result<&str> {
    let ok = !name.is_empty()
        && name.len() <= 63
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !ok {
        bail!("{name:?} is not a database name devcrate accepts: use 1-63 letters, digits, _ or -");
    }
    Ok(name)
}

/// The server's client tools and where to reach it.
struct Client {
    engine: Engine,
    bin: PathBuf,
    port: u16,
}

impl Client {
    fn open(stack: &Stack, engine: Engine) -> Result<Client> {
        let service = stack
            .by_kind(engine.kind())
            .ok_or_else(|| anyhow!("{} is not configured in this stack", engine.label()))?;
        if !service.is_installed() {
            bail!(
                "{} is not installed. Install it first: `devcrate install {}`.",
                engine.label(),
                service.kind.as_str()
            );
        }
        let bin = service
            .install_marker
            .parent()
            .ok_or_else(|| anyhow!("{} has no parent", service.install_marker.display()))?
            .to_path_buf();
        let port = service.ports.first().copied().unwrap_or(match engine {
            Engine::MariaDb => 3306,
            Engine::Postgres => 5432,
        });

        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        if TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_err() {
            bail!(
                "{} is not answering on 127.0.0.1:{port}. Start it: `devcrate start {}`.",
                engine.label(),
                service.id
            );
        }
        Ok(Client { engine, bin, port })
    }

    /// The first of `names` present in `bin\` -- MariaDB's tools were `mysql*`
    /// before they were `mariadb*`, and an older install still has those.
    fn tool(&self, names: &[&str]) -> Result<PathBuf> {
        names
            .iter()
            .map(|name| self.bin.join(name))
            .find(|path| path.is_file())
            .ok_or_else(|| anyhow!("{} not found in {}", names[0], self.bin.display()))
    }

    /// The SQL shell, connected as the superuser, to `database` (or none).
    fn shell(&self, database: Option<&str>) -> Result<Command> {
        let mut command = match self.engine {
            Engine::MariaDb => {
                let mut c = Command::new(self.tool(&["mariadb.exe", "mysql.exe"])?);
                c.args(mariadb_connection(self.port));
                c.args(["--batch", "--skip-column-names"]);
                if let Some(db) = database {
                    c.arg(db);
                }
                c
            }
            Engine::Postgres => {
                let mut c = Command::new(self.tool(&["psql.exe"])?);
                c.args(postgres_connection(self.port));
                c.args(["-X", "-q", "-A", "-t", "-v", "ON_ERROR_STOP=1"]);
                c.args(["-d", database.unwrap_or("postgres")]);
                postgres_password(&mut c);
                c
            }
        };
        command.stdin(Stdio::null());
        Ok(command)
    }

    /// Run one statement with no database selected; return its output lines.
    fn query(&self, sql: &str) -> Result<Vec<String>> {
        let mut command = self.shell(None)?;
        match self.engine {
            Engine::MariaDb => command.args(["-e", sql]),
            Engine::Postgres => command.args(["-c", sql]),
        };
        let out = output(&mut command)?;
        Ok(out.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
    }

    fn databases(&self) -> Result<Vec<String>> {
        let sql = match self.engine {
            Engine::MariaDb => "SHOW DATABASES",
            Engine::Postgres => "SELECT datname FROM pg_database ORDER BY datname",
        };
        Ok(self.query(sql)?.into_iter().filter(|d| !self.engine.is_system(d)).collect())
    }

    fn exists(&self, name: &str) -> Result<bool> {
        Ok(self.databases()?.iter().any(|d| d == name))
    }
}

/// `--host` and friends for the MariaDB tools: TCP to loopback, as `root`.
fn mariadb_connection(port: u16) -> Vec<String> {
    vec![
        "--protocol=TCP".into(),
        "--host=127.0.0.1".into(),
        format!("--port={port}"),
        "--user=root".into(),
    ]
}

fn postgres_connection(port: u16) -> Vec<String> {
    vec!["-h".into(), "127.0.0.1".into(), "-p".into(), port.to_string(), "-U".into(), "postgres".into()]
}

/// The password the install sets, unless the caller has one of their own.
fn postgres_password(command: &mut Command) {
    if std::env::var_os("PGPASSWORD").is_none() {
        command.env("PGPASSWORD", crate::install::postgres_default_password());
    }
}

/// Run a client to completion; its stdout on success, its first error line on
/// failure -- the clients say what went wrong better than a paraphrase would.
fn output(command: &mut Command) -> Result<String> {
    let out = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("running {}", command.get_program().to_string_lossy()))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let message = stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("exit code {}", out.status.code().unwrap_or(-1)));
    Err(anyhow!("{message}"))
}

// ---------------------------------------------------------------------------
// The core
// ---------------------------------------------------------------------------

/// The port the stack's server for `engine` listens on, as configured.
pub fn port(stack: &Stack, engine: Engine) -> u16 {
    stack.by_kind(engine.kind()).and_then(|service| service.ports.first().copied()).unwrap_or(match engine {
        Engine::MariaDb => 3306,
        Engine::Postgres => 5432,
    })
}

/// The PostgreSQL superuser's password as the clients here send it.
pub fn postgres_password_value() -> String {
    std::env::var("PGPASSWORD").unwrap_or_else(|_| crate::install::postgres_default_password().to_string())
}

/// The databases on a server, its own system databases left out.
pub fn list(stack: &Stack, engine: Engine) -> Result<Vec<String>> {
    Client::open(stack, engine)?.databases()
}

/// Create a database; `Ok(false)` when it already existed and nothing changed.
pub fn create(stack: &Stack, engine: Engine, name: &str) -> Result<bool> {
    let name = check_name(name)?;
    let client = Client::open(stack, engine)?;
    if client.exists(name)? {
        return Ok(false);
    }
    let sql = match engine {
        // utf8mb4, so a Laravel or WordPress install does not have to say so.
        Engine::MariaDb => {
            format!("CREATE DATABASE `{name}` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci")
        }
        Engine::Postgres => format!("CREATE DATABASE \"{name}\" ENCODING 'UTF8'"),
    };
    client.query(&sql)?;
    Ok(true)
}

/// Drop a database. Refuses the servers' own, and one that does not exist.
pub fn drop(stack: &Stack, engine: Engine, name: &str) -> Result<()> {
    let name = check_name(name)?;
    if engine.is_system(name) {
        bail!("{name} is one of {}'s own databases; devcrate does not drop it", engine.label());
    }
    let client = Client::open(stack, engine)?;
    if !client.exists(name)? {
        bail!("there is no {} database named {name}", engine.label());
    }
    let sql = match engine {
        Engine::MariaDb => format!("DROP DATABASE `{name}`"),
        Engine::Postgres => format!("DROP DATABASE \"{name}\""),
    };
    client.query(&sql)?;
    Ok(())
}

/// Run an SQL file into a database, creating the database first if it is not
/// there. Returns whether it had to be created.
pub fn import(stack: &Stack, engine: Engine, name: &str, file: &Path) -> Result<bool> {
    let name = check_name(name)?;
    if !file.is_file() {
        bail!("{} is not a file", file.display());
    }
    let created = create(stack, engine, name)?;
    let client = Client::open(stack, engine)?;
    let mut command = client.shell(Some(name))?;
    match engine {
        Engine::MariaDb => {
            let input = File::open(file).with_context(|| format!("opening {}", file.display()))?;
            command.stdin(input);
        }
        Engine::Postgres => {
            command.arg("-f").arg(file);
        }
    }
    output(&mut command).with_context(|| format!("importing {} into {name}", file.display()))?;
    Ok(created)
}

/// Dump a database to an SQL file. Written beside the target and renamed into
/// place, so a failed dump never leaves a half file under the real name; an
/// existing file is refused unless `force`.
pub fn export(stack: &Stack, engine: Engine, name: &str, file: &Path, force: bool) -> Result<u64> {
    let name = check_name(name)?;
    if file.exists() && !force {
        bail!("{} already exists; pass --force to overwrite it", file.display());
    }
    let client = Client::open(stack, engine)?;
    if !client.exists(name)? {
        bail!("there is no {} database named {name}", engine.label());
    }

    let partial = file.with_extension("sql.part");
    let result = (|| -> Result<()> {
        match engine {
            Engine::MariaDb => {
                let mut c = Command::new(client.tool(&["mariadb-dump.exe", "mysqldump.exe"])?);
                c.args(mariadb_connection(client.port));
                c.args(["--single-transaction", "--routines", "--triggers", "--events"]);
                c.arg(format!("--result-file={}", partial.display()));
                c.arg(name).stdin(Stdio::null());
                output(&mut c)?;
            }
            Engine::Postgres => {
                let mut c = Command::new(client.tool(&["pg_dump.exe"])?);
                c.args(postgres_connection(client.port));
                c.args(["-d", name, "-f"]).arg(&partial).stdin(Stdio::null());
                postgres_password(&mut c);
                output(&mut c)?;
            }
        }
        std::fs::rename(&partial, file).with_context(|| format!("writing {}", file.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result.with_context(|| format!("exporting {name}"))?;
    Ok(std::fs::metadata(file).map(|m| m.len()).unwrap_or(0))
}

// ---------------------------------------------------------------------------
// The CLI
// ---------------------------------------------------------------------------

/// What `devcrate db` was asked to do.
pub enum Action<'a> {
    List,
    Create(&'a str),
    Drop { name: &'a str, force: bool },
    Import { name: &'a str, file: &'a Path },
    Export { name: &'a str, file: Option<&'a Path>, force: bool },
}

pub fn run(stack: &Stack, engine: Engine, action: Action) -> Result<u8> {
    let label = engine.label();
    match action {
        Action::List => {
            let databases = list(stack, engine)?;
            if databases.is_empty() {
                println!("No {label} databases yet (its own system databases are not listed).");
            }
            for db in databases {
                println!("{db}");
            }
        }
        Action::Create(name) => match create(stack, engine, name)? {
            true => println!("created  {label} database {name}"),
            false => println!("exists   {label} database {name} -- nothing changed"),
        },
        Action::Drop { name, force } => {
            if !force {
                bail!("dropping {name} deletes its data; pass --force to do it");
            }
            drop(stack, engine, name)?;
            println!("dropped  {label} database {name}");
        }
        Action::Import { name, file } => {
            if import(stack, engine, name, file)? {
                println!("created  {label} database {name}");
            }
            println!("imported {} into {name}", file.display());
        }
        Action::Export { name, file, force } => {
            let file = file.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(format!("{name}.sql")));
            let bytes = export(stack, engine, name, &file, force)?;
            println!("exported {name} to {} ({bytes} bytes)", file.display());
        }
    }
    Ok(exit::OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The name goes inside identifier quotes with no escaping, so the check
    /// is what keeps a name from closing them and becoming a statement.
    #[test]
    fn only_plain_names_are_accepted() {
        assert!(check_name("myapp").is_ok());
        assert!(check_name("my_app_2").is_ok());
        assert!(check_name("odoo-backend-test").is_ok());
        for bad in ["", "a b", "a.b", "x`; DROP DATABASE mysql; --", "x\"", "é", &"a".repeat(64)] {
            assert!(check_name(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn system_databases_are_known_per_engine() {
        assert!(Engine::MariaDb.is_system("mysql"));
        assert!(Engine::MariaDb.is_system("INFORMATION_SCHEMA"));
        assert!(!Engine::MariaDb.is_system("postgres"));
        assert!(Engine::Postgres.is_system("template1"));
        assert!(!Engine::Postgres.is_system("myapp"));
    }

    #[test]
    fn engines_answer_to_their_usual_names() {
        assert_eq!(Engine::parse("MySQL").unwrap(), Engine::MariaDb);
        assert_eq!(Engine::parse("pgsql").unwrap(), Engine::Postgres);
        assert!(Engine::parse("sqlite").is_err());
    }
}
