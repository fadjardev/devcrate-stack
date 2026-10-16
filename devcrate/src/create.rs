//! `devcrate new laravel|wordpress <name>` -- a working project in one step:
//! the code, its vhost, its database, and its configuration pointing at both.
//!
//! The order is what keeps it safe:
//!
//! 1. **Preflight**, writing nothing: the name, the folder, the conf, the PHP
//!    and the extensions it needs, Composer, the database server answering.
//! 2. **Fetch into a staging folder** -- `projects\_new-<name>`. The leading
//!    `_` is what auto-vhost skips, so a scanner running meanwhile cannot serve
//!    a half-made project from the wrong root. A failure here removes the
//!    staging folder and leaves nothing behind.
//! 3. **Rename into place**, then the vhost straight away -- before anything
//!    slow -- so auto-vhost has no window to claim the folder first.
//! 4. Database, configuration (`.env` / `wp-config.php`), and for Laravel
//!    `php artisan migrate`.
//!
//! From step 3 on, a failure does not undo the project: the folder is real
//! work by then. Each step that did not happen is reported with the command
//! that finishes it. A folder that already has content is never written into.
//!
//! The core never prints. Child processes (Composer, artisan) write where
//! [`Output`] says: the terminal for the CLI, a log file for the dashboard.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};

use crate::config::{Service, Stack};
use crate::db::{self, Engine};
use crate::{autosite, exit, php, site};

/// What kind of project to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Laravel,
    WordPress,
}

impl Kind {
    pub fn parse(name: &str) -> Result<Kind> {
        match name.trim().to_ascii_lowercase().as_str() {
            "laravel" => Ok(Kind::Laravel),
            "wordpress" | "wp" => Ok(Kind::WordPress),
            other => bail!("{other:?} is not a project kind devcrate can create (laravel, wordpress)"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Laravel => "Laravel",
            Kind::WordPress => "WordPress",
        }
    }
}

/// Where the project keeps its data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Database {
    MariaDb,
    Postgres,
    /// Laravel's own default: a file in `database\`, no server.
    Sqlite,
}

impl Database {
    pub fn parse(name: &str) -> Result<Database> {
        match name.trim().to_ascii_lowercase().as_str() {
            "sqlite" => Ok(Database::Sqlite),
            other => Ok(match Engine::parse(other)? {
                Engine::MariaDb => Database::MariaDb,
                Engine::Postgres => Database::Postgres,
            }),
        }
    }

    fn engine(self) -> Option<Engine> {
        match self {
            Database::MariaDb => Some(Engine::MariaDb),
            Database::Postgres => Some(Engine::Postgres),
            Database::Sqlite => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Database::MariaDb => "MariaDB",
            Database::Postgres => "PostgreSQL",
            Database::Sqlite => "SQLite",
        }
    }
}

/// Where Composer's and artisan's own output goes.
#[derive(Debug, Clone)]
pub enum Output {
    /// Straight to the terminal: the CLI, where Composer's progress is wanted.
    Inherit,
    /// Appended to a file: the dashboard, whose stdout is the screen.
    Log(PathBuf),
}

#[derive(Debug, Clone)]
pub struct Request<'a> {
    pub kind: Kind,
    pub name: &'a str,
    pub php: Option<&'a str>,
    /// `None` is the kind's default: MariaDB.
    pub database: Option<Database>,
    pub no_hosts: bool,
    pub no_tls: bool,
    pub output: Output,
}

/// What a running create reports as it goes.
pub enum Progress<'a> {
    /// A step starting, in the words the command prints.
    Step(&'a str),
    /// Bytes of a download.
    Bytes(u64, Option<u64>),
}

/// A project that now exists, and how far the rest got.
#[derive(Debug, Default)]
pub struct Created {
    pub host: String,
    /// Root-relative: `projects\shop`.
    pub folder: String,
    /// What was done, one line each, the same words on both front ends.
    pub lines: Vec<String>,
    /// What was not, each with the command that finishes it.
    pub unfinished: Vec<String>,
}

/// The log file the dashboard sends a create's child output to.
pub fn log_path(stack: &Stack, name: &str) -> PathBuf {
    stack.root.join("logs").join(format!("new-{}.log", name.to_ascii_lowercase()))
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

/// Everything decided before anything is written.
struct Plan<'s> {
    host: String,
    folder: PathBuf,
    staging: PathBuf,
    php: &'s Service,
    php_exe: PathBuf,
    database: Database,
    /// Unused for SQLite.
    db_name: String,
    composer: Option<PathBuf>,
    wordpress: Option<crate::install::wordpress::WordPressRelease>,
}

/// `Shop` -> host `shop.test`, database `shop`; `api.shop` -> `api.shop.test`
/// and `api_shop`. Refused when it cannot be both a hostname and a folder.
pub fn names(name: &str) -> Result<(String, String)> {
    let host = autosite::host_for(name).ok_or_else(|| {
        anyhow!("{name:?} cannot be a hostname: use letters, digits, - and . only")
    })?;
    let stem = name.to_ascii_lowercase();
    let stem = stem.strip_suffix(".test").unwrap_or(&stem);
    let db_name: String = stem.chars().map(|c| if c == '.' || c == '-' { '_' } else { c }).collect();
    db::check_name(&db_name)?;
    Ok((host, db_name))
}

/// A folder may be created into only when it is missing or empty.
pub fn check_folder(stack: &Stack, folder: &Path) -> Result<()> {
    if !folder.exists() {
        return Ok(());
    }
    if !folder.is_dir() {
        bail!("{} exists and is not a folder", stack.rel(folder));
    }
    let has_content = std::fs::read_dir(folder)
        .with_context(|| format!("reading {}", folder.display()))?
        .next()
        .is_some();
    if has_content {
        bail!("{} already has content; quick-create never writes into it", stack.rel(folder));
    }
    Ok(())
}

/// The extensions a project needs loaded in the PHP that will run it.
pub fn required_extensions(kind: Kind, database: Database) -> Vec<&'static str> {
    let mut wanted = vec!["openssl", "mbstring"];
    match kind {
        Kind::Laravel => {
            wanted.push("fileinfo");
            wanted.push(match database {
                Database::MariaDb => "pdo_mysql",
                Database::Postgres => "pdo_pgsql",
                Database::Sqlite => "pdo_sqlite",
            });
        }
        Kind::WordPress => wanted.push("mysqli"),
    }
    wanted
}

/// Which of `wanted` the output of `php -m` does not list.
pub fn missing_extensions(php_m: &str, wanted: &[&str]) -> Vec<String> {
    let loaded: Vec<String> = php_m.lines().map(|line| line.trim().to_ascii_lowercase()).collect();
    wanted.iter().filter(|ext| !loaded.iter().any(|l| l == *ext)).map(|ext| ext.to_string()).collect()
}

/// `8.5` from a PHP service: its folder name without the `php-`.
fn php_version(service: &Service) -> String {
    service.id.trim_start_matches(|c: char| !c.is_ascii_digit()).to_string()
}

/// `8.5` >= `7.4`, comparing as numbers part by part.
pub fn version_at_least(have: &str, want: &str) -> bool {
    let parts = |v: &str| -> Vec<u32> { v.split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    let (have, want) = (parts(have), parts(want));
    for i in 0..have.len().max(want.len()) {
        let (h, w) = (have.get(i).copied().unwrap_or(0), want.get(i).copied().unwrap_or(0));
        if h != w {
            return h > w;
        }
    }
    true
}

fn preflight<'s>(stack: &'s Stack, request: &Request) -> Result<Plan<'s>> {
    let (host, db_name) = names(request.name)?;
    let folder_name = request.name.to_string();
    let projects = stack.root.join("projects");
    let folder = projects.join(&folder_name);
    check_folder(stack, &folder)?;
    let conf = stack.sites_dir().join(format!("{host}.conf"));
    if conf.exists() {
        bail!("{} already exists: {host} is already a site", stack.rel(&conf));
    }

    let database = request.database.unwrap_or(Database::MariaDb);
    if request.kind == Kind::WordPress && database != Database::MariaDb {
        bail!("WordPress runs on MariaDB/MySQL only; leave --db out, or pass --db mariadb");
    }

    let service = match request.php {
        Some(wanted) => php::find(stack, wanted)?,
        None => site::default_php(stack)?,
    };
    let php_exe = service
        .install_marker
        .parent()
        .map(|dir| dir.join("php.exe"))
        .filter(|exe| exe.is_file())
        .ok_or_else(|| anyhow!("{} has no php.exe to run Composer and artisan with", service.id))?;

    let modules = Command::new(&php_exe)
        .arg("-m")
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running {} -m", stack.rel(&php_exe)))?;
    let missing =
        missing_extensions(&String::from_utf8_lossy(&modules.stdout), &required_extensions(request.kind, database));
    if !missing.is_empty() {
        bail!(
            "{} does not load {} -- {} needs {}.\nEnable: devcrate php ext enable {} --php {}",
            service.name,
            missing.join(", "),
            request.kind.label(),
            if missing.len() == 1 { "it" } else { "them" },
            missing.join(" "),
            php_version(service)
        );
    }

    let composer = match request.kind {
        Kind::Laravel => {
            let phar = stack.root.join("composer").join("composer.phar");
            if !phar.is_file() {
                bail!("Composer is not installed. Install it first: `devcrate install composer`.");
            }
            Some(phar)
        }
        Kind::WordPress => None,
    };

    // The server has to answer now, not after a five-minute download.
    if let Some(engine) = database.engine() {
        db::list(stack, engine)?;
    }

    let wordpress = match request.kind {
        Kind::WordPress => {
            let release = crate::install::wordpress::release()?;
            let have = php_version(service);
            if !release.min_php.is_empty() && !version_at_least(&have, &release.min_php) {
                bail!(
                    "WordPress {} needs PHP {} or newer; {} is too old. Pass --php with a newer one.",
                    release.version,
                    release.min_php,
                    service.name
                );
            }
            Some(release)
        }
        Kind::Laravel => None,
    };

    Ok(Plan {
        host,
        staging: projects.join(format!("_new-{folder_name}")),
        folder,
        php: service,
        php_exe,
        database,
        db_name,
        composer,
        wordpress,
    })
}

// ---------------------------------------------------------------------------
// The steps
// ---------------------------------------------------------------------------

pub fn run(stack: &Stack, request: &Request, progress: &mut dyn FnMut(Progress)) -> Result<Created> {
    progress(Progress::Step("checking"));
    let plan = preflight(stack, request)?;
    let mut created = Created { host: plan.host.clone(), folder: stack.rel(&plan.folder), ..Default::default() };

    if let Output::Log(path) = &request.output {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        std::fs::write(path, "").with_context(|| format!("creating {}", path.display()))?;
    }

    // A fresh stack has no projects\ yet; Composer is run from inside it.
    let projects = stack.root.join("projects");
    std::fs::create_dir_all(&projects).with_context(|| format!("creating {}", projects.display()))?;

    // A staging folder left by a run that was killed is ours to clear.
    if plan.staging.exists() {
        std::fs::remove_dir_all(&plan.staging)
            .with_context(|| format!("clearing the leftover {}", stack.rel(&plan.staging)))?;
    }
    let fetched = fetch(stack, request, &plan, progress);
    let fetched = match fetched {
        Ok(line) => line,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&plan.staging);
            return Err(err);
        }
    };
    created.lines.push(fetched);

    progress(Progress::Step("moving into place"));
    if let Err(err) = move_into_place(&plan.staging, &plan.folder) {
        let _ = std::fs::remove_dir_all(&plan.staging);
        return Err(err);
    }
    created.lines.push(format!("created  {}", created.folder));

    // The vhost first: it is quick, and once it exists auto-vhost leaves the
    // folder alone.
    progress(Progress::Step("creating the vhost"));
    let php_version = php_version(plan.php);
    let site_request = site::Request {
        project_path: Some(&plan.folder),
        want_php: Some(&php_version),
        no_hosts: request.no_hosts,
        no_tls: request.no_tls,
        untouched: true,
        force: false,
        proxy: None,
    };
    match site::create_with(stack, &plan.host, &site_request) {
        Ok(made) => {
            created.lines.push(format!("vhost    https://{} -> {} ({})", made.host, made.public, made.php_name));
            if let Some(note) = &made.hosts_note {
                created.lines.push(format!("hosts    not updated ({note}); add 127.0.0.1 {} by hand", made.host));
            }
            match &made.reload {
                site::Reload::Failed(why) => created.unfinished.push(format!(
                    "nginx did not reload: {why}\n         finish with: devcrate restart nginx"
                )),
                reload => created.lines.push(format!("         {}", reload.note())),
            }
        }
        Err(err) => created.unfinished.push(format!(
            "vhost: {err:#}\n         finish with: devcrate site add {} --path {} --php {php_version}",
            plan.host, created.folder
        )),
    }

    let mut db_ready = plan.database == Database::Sqlite;
    if let Some(engine) = plan.database.engine() {
        progress(Progress::Step("creating the database"));
        match db::create(stack, engine, &plan.db_name) {
            Ok(true) => {
                db_ready = true;
                created.lines.push(format!("database {} {} created", engine.label(), plan.db_name));
            }
            Ok(false) => {
                db_ready = true;
                created.lines.push(format!("database {} {} already existed; used as it is", engine.label(), plan.db_name));
            }
            Err(err) => created.unfinished.push(format!(
                "database: {err:#}\n         finish with: devcrate db --engine {} create {}",
                engine_flag(engine),
                plan.db_name
            )),
        }
    }

    progress(Progress::Step("writing the configuration"));
    match configure(stack, request.kind, &plan) {
        Ok(line) => created.lines.push(line),
        Err(err) => created.unfinished.push(format!("configuration: {err:#}")),
    }

    if request.kind == Kind::Laravel && plan.database != Database::Sqlite {
        if db_ready {
            progress(Progress::Step("running php artisan migrate"));
            let mut migrate = Command::new(&plan.php_exe);
            migrate.args(["artisan", "migrate", "--force", "--no-interaction"]).current_dir(&plan.folder);
            match child(&mut migrate, &request.output) {
                Ok(()) => created.lines.push("migrated php artisan migrate".to_string()),
                Err(err) => created.unfinished.push(format!(
                    "migrate: {err:#}\n         finish with: php artisan migrate   (in {})",
                    created.folder
                )),
            }
        } else {
            created.unfinished.push(format!(
                "migrate: skipped, the database is not there\n         finish with: php artisan migrate   (in {})",
                created.folder
            ));
        }
    }

    if let Output::Log(path) = &request.output {
        created.lines.push(format!("log      {}", stack.rel(path)));
    }
    Ok(created)
}

fn engine_flag(engine: Engine) -> &'static str {
    match engine {
        Engine::MariaDb => "mariadb",
        Engine::Postgres => "postgres",
    }
}

/// Put the code into the staging folder; the line saying what arrived.
fn fetch(stack: &Stack, request: &Request, plan: &Plan, progress: &mut dyn FnMut(Progress)) -> Result<String> {
    match request.kind {
        Kind::Laravel => {
            progress(Progress::Step("composer create-project laravel/laravel"));
            let phar = plan.composer.as_ref().ok_or_else(|| anyhow!("no Composer"))?;
            let mut composer = Command::new(&plan.php_exe);
            composer
                .arg(phar)
                .args(["create-project", "laravel/laravel"])
                .arg(&plan.staging)
                .args(["--prefer-dist", "--no-interaction"])
                .current_dir(stack.root.join("projects"));
            // Composer's home and cache stay in the stack root, as the
            // composer shims are documented to have them -- unless the user
            // already points them somewhere.
            for (var, dir) in [("COMPOSER_HOME", "home"), ("COMPOSER_CACHE_DIR", "cache")] {
                if std::env::var_os(var).is_none() {
                    composer.env(var, stack.root.join("composer").join(dir));
                }
            }
            child(&mut composer, &request.output).context("composer create-project laravel/laravel")?;

            let mut version = Command::new(&plan.php_exe);
            version.args(["artisan", "--version"]).current_dir(&plan.staging).stdin(Stdio::null());
            let framework = version
                .output()
                .ok()
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                .filter(|line| !line.is_empty())
                .unwrap_or_else(|| "laravel/laravel".to_string());
            Ok(format!("fetched  {framework} with Composer, under {}", plan.php.name))
        }
        Kind::WordPress => {
            let release = plan.wordpress.as_ref().ok_or_else(|| anyhow!("no WordPress release"))?;
            progress(Progress::Step("downloading WordPress"));
            std::fs::create_dir_all(&plan.staging)
                .with_context(|| format!("creating {}", plan.staging.display()))?;
            let fetched = crate::install::wordpress::unpack(stack, release, &plan.staging, &mut |done, total| {
                progress(Progress::Bytes(done, total))
            })?;
            Ok(format!(
                "fetched  WordPress {}{}, SHA-1 matches the one wordpress.org publishes",
                fetched.version,
                if fetched.cached { " from _downloads\\" } else { "" }
            ))
        }
    }
}

/// Rename the staging folder to its real name. Retried briefly: on Windows a
/// virus scanner looking at the fresh files can hold the folder for a moment.
fn move_into_place(staging: &Path, folder: &Path) -> Result<()> {
    if folder.is_dir() {
        // Checked empty in the preflight; a rename cannot land on it.
        std::fs::remove_dir(folder).with_context(|| format!("replacing the empty {}", folder.display()))?;
    }
    let mut last = None;
    for _ in 0..10 {
        match std::fs::rename(staging, folder) {
            Ok(()) => return Ok(()),
            Err(err) => last = Some(err),
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Err(anyhow!(
        "moving {} to {}: {}",
        staging.display(),
        folder.display(),
        last.map(|e| e.to_string()).unwrap_or_default()
    ))
}

/// Run a child to completion, its output where `output` says.
fn child(command: &mut Command, output: &Output) -> Result<()> {
    command.stdin(Stdio::null());
    if let Output::Log(path) = output {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        command.stdout(file.try_clone()?).stderr(file);
    }
    let status = command
        .status()
        .with_context(|| format!("running {}", command.get_program().to_string_lossy()))?;
    if !status.success() {
        let log = match output {
            Output::Log(path) => format!("; its output is in {}", path.display()),
            Output::Inherit => String::new(),
        };
        bail!("exited with code {}{log}", status.code().unwrap_or(-1));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

fn configure(stack: &Stack, kind: Kind, plan: &Plan) -> Result<String> {
    match kind {
        Kind::Laravel => {
            let path = plan.folder.join(".env");
            let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            let mut pairs = vec![("APP_URL", format!("https://{}", plan.host))];
            if let Some(engine) = plan.database.engine() {
                let connection = match engine {
                    // Laravel 11 added a `mariadb` connection; older skeletons
                    // reach MariaDB through `mysql`.
                    Engine::MariaDb => {
                        let config = std::fs::read_to_string(plan.folder.join("config").join("database.php"))
                            .unwrap_or_default();
                        if config.contains("'mariadb' =>") { "mariadb" } else { "mysql" }
                    }
                    Engine::Postgres => "pgsql",
                };
                let (user, password) = match engine {
                    Engine::MariaDb => ("root", String::new()),
                    Engine::Postgres => ("postgres", db::postgres_password_value()),
                };
                pairs.extend([
                    ("DB_CONNECTION", connection.to_string()),
                    ("DB_HOST", "127.0.0.1".to_string()),
                    ("DB_PORT", db::port(stack, engine).to_string()),
                    ("DB_DATABASE", plan.db_name.clone()),
                    ("DB_USERNAME", user.to_string()),
                    ("DB_PASSWORD", password),
                ]);
            }
            std::fs::write(&path, set_env(&text, &pairs)).with_context(|| format!("writing {}", path.display()))?;
            let keys = if plan.database.engine().is_some() { "APP_URL, DB_*" } else { "APP_URL" };
            Ok(format!("wrote    {} ({keys})", stack.rel(&path)))
        }
        Kind::WordPress => {
            let sample = plan.folder.join("wp-config-sample.php");
            let path = plan.folder.join("wp-config.php");
            if path.exists() {
                bail!("{} already exists; left as it is", stack.rel(&path));
            }
            let text =
                std::fs::read_to_string(&sample).with_context(|| format!("reading {}", sample.display()))?;
            let port = db::port(stack, Engine::MariaDb);
            let config = wp_config(&text, &plan.db_name, port, &mut salt)?;
            std::fs::write(&path, config).with_context(|| format!("writing {}", path.display()))?;
            Ok(format!("wrote    {} (database, fresh salts)", stack.rel(&path)))
        }
    }
}

/// Set each `KEY=value` in a `.env`: the line that sets it, or the commented
/// `# KEY=` Laravel leaves for unused drivers, is replaced; a key that appears
/// nowhere is appended. Everything else -- comments, order, line endings -- is
/// kept.
pub fn set_env(text: &str, pairs: &[(&str, String)]) -> String {
    let crlf = text.contains("\r\n");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    for (key, value) in pairs {
        let wanted = format!("{key}={}", env_value(value));
        let sets = |line: &str| {
            let bare = line.trim_start().trim_start_matches('#').trim_start();
            bare.strip_prefix(key).is_some_and(|rest| rest.trim_start().starts_with('='))
        };
        match lines.iter().position(|line| sets(line)) {
            Some(index) => lines[index] = wanted,
            None => lines.push(wanted),
        }
    }
    let newline = if crlf { "\r\n" } else { "\n" };
    let mut out = lines.join(newline);
    out.push_str(newline);
    out
}

/// Quoted only when it has to be.
fn env_value(value: &str) -> String {
    if value.chars().any(|c| c.is_whitespace() || c == '#' || c == '"' || c == '\'') {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

/// `wp-config.php` from WordPress's sample: the database, as root with no
/// password on the stack's MariaDB, and every "put your unique phrase here"
/// replaced by its own random salt. A sample that has changed shape is an
/// error rather than a half-filled config.
pub fn wp_config(sample: &str, db_name: &str, port: u16, salt: &mut dyn FnMut() -> String) -> Result<String> {
    let mut text = sample.to_string();
    for (from, to) in [
        ("'database_name_here'", format!("'{db_name}'")),
        ("'username_here'", "'root'".to_string()),
        ("'password_here'", "''".to_string()),
        ("'DB_HOST', 'localhost'", format!("'DB_HOST', '127.0.0.1:{port}'")),
    ] {
        if !text.contains(from) {
            bail!("wp-config-sample.php has no {from} to fill in; write wp-config.php by hand");
        }
        text = text.replacen(from, &to, 1);
    }
    let placeholder = "'put your unique phrase here'";
    if !text.contains(placeholder) {
        bail!("wp-config-sample.php has no salts to fill in; write wp-config.php by hand");
    }
    while text.contains(placeholder) {
        text = text.replacen(placeholder, &format!("'{}'", salt()), 1);
    }
    Ok(text)
}

/// 64 random characters for a WordPress salt, from the OS-seeded keys of
/// `RandomState` -- no quote or backslash, so it sits in a PHP string as is.
fn salt() -> String {
    use std::hash::{BuildHasher, Hasher};
    const ALPHABET: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!@#%^&*()-_=+[]{}<>~;:,.?|";
    let mut out = String::with_capacity(64);
    let mut counter = 0u64;
    while out.len() < 64 {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(counter);
        counter += 1;
        let mut bits = hasher.finish();
        for _ in 0..8 {
            out.push(ALPHABET[(bits % ALPHABET.len() as u64) as usize] as char);
            bits /= ALPHABET.len() as u64;
        }
    }
    out.truncate(64);
    out
}

// ---------------------------------------------------------------------------
// The CLI
// ---------------------------------------------------------------------------

/// `devcrate new <kind> <name>`.
pub fn print(stack: &Stack, request: &Request) -> Result<u8> {
    println!("New {} project {}", request.kind.label(), request.name);
    let created = run(stack, request, &mut |progress| {
        if let Progress::Step(step) = progress {
            println!("  ...      {step}");
        }
    })?;
    for line in &created.lines {
        println!("  {line}");
    }
    for line in &created.unfinished {
        println!("  NOT DONE {line}");
    }
    if created.unfinished.is_empty() {
        println!("Ready: https://{}", created.host);
        Ok(exit::OK)
    } else {
        println!("{} is in place, but not finished -- see NOT DONE above.", created.folder);
        Ok(exit::ERROR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_become_a_host_and_a_database() {
        assert_eq!(names("Shop").unwrap(), ("shop.test".into(), "shop".into()));
        assert_eq!(names("api.shop").unwrap(), ("api.shop.test".into(), "api_shop".into()));
        assert_eq!(names("my-blog.test").unwrap(), ("my-blog.test".into(), "my_blog".into()));
        assert!(names("my app").is_err());
        assert!(names("my_app").is_err(), "an underscore cannot be in a hostname");
    }

    #[test]
    fn env_lines_are_replaced_uncommented_or_appended() {
        let env = "APP_NAME=Laravel\r\nAPP_URL=http://localhost\r\n\r\nDB_CONNECTION=sqlite\r\n# DB_HOST=127.0.0.1\r\n# DB_DATABASE=laravel\r\nDB_HOSTNAME=keep\r\n";
        let out = set_env(
            env,
            &[
                ("APP_URL", "https://shop.test".into()),
                ("DB_CONNECTION", "mariadb".into()),
                ("DB_HOST", "127.0.0.1".into()),
                ("DB_DATABASE", "shop".into()),
                ("DB_PASSWORD", String::new()),
                ("APP_NAME", "My Shop".into()),
            ],
        );
        assert_eq!(
            out,
            "APP_NAME=\"My Shop\"\r\nAPP_URL=https://shop.test\r\n\r\nDB_CONNECTION=mariadb\r\nDB_HOST=127.0.0.1\r\nDB_DATABASE=shop\r\nDB_HOSTNAME=keep\r\nDB_PASSWORD=\r\n"
        );
    }

    #[test]
    fn wp_config_is_filled_with_its_own_salts() {
        let sample = "define( 'DB_NAME', 'database_name_here' );\n\
                      define( 'DB_USER', 'username_here' );\n\
                      define( 'DB_PASSWORD', 'password_here' );\n\
                      define( 'DB_HOST', 'localhost' );\n\
                      define( 'AUTH_KEY',         'put your unique phrase here' );\n\
                      define( 'NONCE_SALT',       'put your unique phrase here' );\n";
        let config = wp_config(sample, "blog", 3306, &mut salt).unwrap();
        assert!(config.contains("'DB_NAME', 'blog'"));
        assert!(config.contains("'DB_USER', 'root'"));
        assert!(config.contains("'DB_PASSWORD', ''"));
        assert!(config.contains("'DB_HOST', '127.0.0.1:3306'"));
        assert!(!config.contains("put your unique phrase here"));
        let salts: Vec<&str> = config.lines().skip(4).map(|l| l.split('\'').nth(3).unwrap()).collect();
        assert_eq!(salts.len(), 2);
        assert!(salts.iter().all(|s| s.len() == 64 && !s.contains('\'') && !s.contains('\\')));
        assert_ne!(salts[0], salts[1]);

        assert!(wp_config("<?php // nothing to fill", "blog", 3306, &mut salt).is_err());
    }

    #[test]
    fn missing_extensions_are_named() {
        let php_m = "[PHP Modules]\nCore\nmbstring\nopenssl\nPDO\npdo_mysql\n\n[Zend Modules]\n";
        assert!(missing_extensions(php_m, &required_extensions(Kind::Laravel, Database::MariaDb)) == ["fileinfo"]);
        assert_eq!(missing_extensions(php_m, &required_extensions(Kind::WordPress, Database::MariaDb)), ["mysqli"]);
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(version_at_least("8.5", "7.4"));
        assert!(version_at_least("7.4", "7.4"));
        assert!(version_at_least("8.10", "8.9"));
        assert!(!version_at_least("7.2", "7.4"));
    }

    #[test]
    fn wordpress_offers_are_read() {
        let json = r#"{"offers":[{"response":"upgrade","download":"https://downloads.wordpress.org/release/wordpress-7.1.3.zip","packages":{"full":"https://downloads.wordpress.org/release/wordpress-7.1.3.zip"},"version":"7.1.3","php_version":"7.4"}]}"#;
        let release = crate::install::wordpress::parse(json).unwrap();
        assert_eq!(release.version, "7.1.3");
        assert_eq!(release.min_php, "7.4");
        assert!(release.url.ends_with("wordpress-7.1.3.zip"));
        assert!(crate::install::wordpress::parse(r#"{"offers":[]}"#).is_err());
    }

    fn root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("devcrate-create-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("nginx").join("conf").join("sites")).unwrap();
        std::fs::write(root.join("nginx").join("conf").join("nginx.conf"), "").unwrap();
        std::fs::create_dir_all(root.join("projects")).unwrap();
        root
    }

    /// Missing and empty folders may be created into; one with anything in
    /// it, or a conf for the host, stops the preflight before any write.
    #[test]
    fn a_folder_with_content_or_a_site_is_refused() {
        use crate::root::{RootSource, StackRoot};
        let root = root("refuse");
        let stack = Stack::open(StackRoot { path: root.clone(), source: RootSource::Flag }).unwrap();
        let projects = root.join("projects");
        assert!(check_folder(&stack, &projects.join("fresh")).is_ok());
        std::fs::create_dir_all(projects.join("empty")).unwrap();
        assert!(check_folder(&stack, &projects.join("empty")).is_ok());
        std::fs::create_dir_all(projects.join("full")).unwrap();
        std::fs::write(projects.join("full").join("index.php"), "<?php").unwrap();
        let err = check_folder(&stack, &projects.join("full")).unwrap_err().to_string();
        assert!(err.contains("already has content"), "{err}");

        std::fs::write(root.join("nginx/conf/sites/taken.test.conf"), "").unwrap();
        let request = Request {
            kind: Kind::WordPress,
            name: "taken",
            php: None,
            database: None,
            no_hosts: true,
            no_tls: true,
            output: Output::Inherit,
        };
        let err = run(&stack, &request, &mut |_| {}).unwrap_err().to_string();
        assert!(err.contains("already a site"), "{err}");
        assert!(!projects.join("taken").exists() && !projects.join("_new-taken").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
