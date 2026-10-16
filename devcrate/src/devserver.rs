//! Dev servers under supervision: a site's `npm run dev` / `bun run dev`,
//! started and stopped by devcrate instead of left running in some other
//! terminal.
//!
//! A dev server is not a service: there is one per project, it runs `node.exe`
//! like any other Node program on the machine, and it is not matched by its
//! path the way nginx or MariaDB are. Starting one records its PID **and the
//! time that process started** in `run\dev\<host>.json`; a PID only counts as
//! the dev server while a process with that start time still holds it, so a
//! PID Windows has since handed to something else is never touched. Stopping
//! kills that process and everything descended from it (`npm` runs `node`,
//! which runs Vite), by PID, deepest first.
//!
//! Output goes to `logs\dev-<host>.log`, which the dashboard's Logs pane lists.
//! `devcrate stop` with no service named stops every dev server first.
//!
//! The command runs with the stack's runtimes first on `PATH` -- `node\current`,
//! `bun\current`, `php\current`, `composer\` -- and npm's cache and global
//! prefix pointed into `node\`, so nothing it downloads lands in the profile.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::config::Stack;
use crate::control::{Step, Stopped};
use crate::probe::ProcessTable;
use crate::site::{self, Site};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// No console window, and its own process group so a Ctrl+C in the terminal
/// that started it does not reach it.
#[cfg(windows)]
const DETACHED: u32 = 0x0000_0200 /* CREATE_NEW_PROCESS_GROUP */ | 0x0800_0000 /* CREATE_NO_WINDOW */;

/// How long a fresh dev server is watched before it counts as started: long
/// enough to catch the usual instant failure (a missing package, a port in
/// use), short enough not to be noticed.
const SETTLE: Duration = Duration::from_millis(2500);

/// What is on disk about a running dev server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Record {
    pub host: String,
    pub pid: u32,
    /// The process's start time, seconds since the Unix epoch.
    pub started: u64,
    pub dir: PathBuf,
    pub command: String,
}

/// A dev server as the scan saw it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DevStatus {
    pub host: String,
    /// `false` once its process has gone without being stopped -- it crashed,
    /// or was closed from elsewhere.
    pub running: bool,
    pub pid: u32,
    pub command: String,
    /// The dev server's own address, as it printed it (`http://localhost:5173/`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Root-relative.
    pub log: String,
}

fn records_dir(stack: &Stack) -> PathBuf {
    stack.root.join("run").join("dev")
}

fn record_path(stack: &Stack, host: &str) -> PathBuf {
    records_dir(stack).join(format!("{host}.json"))
}

pub fn log_path(stack: &Stack, host: &str) -> PathBuf {
    stack.root.join("logs").join(format!("dev-{host}.log"))
}

fn read_record(path: &Path) -> Option<Record> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn records(stack: &Stack) -> Vec<Record> {
    let mut found: Vec<Record> = std::fs::read_dir(records_dir(stack))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| read_record(&e.path()))
        .collect();
    found.sort_by(|a, b| a.host.cmp(&b.host));
    found
}

/// Every dev server devcrate started, running or not, from one process table.
pub fn list(stack: &Stack, table: &ProcessTable) -> Vec<DevStatus> {
    records(stack)
        .into_iter()
        .map(|record| {
            let log = log_path(stack, &record.host);
            let running = table.same(record.pid, record.started).is_some();
            DevStatus {
                running,
                // An address from a server that has gone would only mislead.
                url: running
                    .then(|| std::fs::read(&log).ok())
                    .flatten()
                    .and_then(|bytes| url_from_log(&String::from_utf8_lossy(&bytes))),
                log: stack.rel(&log),
                host: record.host,
                pid: record.pid,
                command: record.command,
            }
        })
        .collect()
}

/// The address a dev server printed last: Vite's `Local:   http://localhost:5173/`
/// line, colour codes and all.
pub fn url_from_log(text: &str) -> Option<String> {
    let plain = strip_ansi(text);
    plain
        .lines()
        .rev()
        .filter(|line| line.contains("Local:") || line.contains("APP_URL:") || line.contains("http://"))
        .flat_map(|line| line.split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .find(|word| {
            ["http://localhost:", "http://127.0.0.1:", "http://[::1]:", "https://localhost:"]
                .iter()
                .any(|prefix| word.starts_with(prefix))
        })
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------------------
// Which command, in what environment
// ---------------------------------------------------------------------------

/// The package manager a project runs `dev` with.
#[derive(Debug, PartialEq, Eq)]
pub enum Runner {
    Npm(PathBuf),
    Bun(PathBuf),
}

impl Runner {
    fn program(&self) -> &Path {
        match self {
            Runner::Npm(path) | Runner::Bun(path) => path,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Runner::Npm(_) => "npm",
            Runner::Bun(_) => "bun",
        }
    }
}

/// A project with a `dev` script runs it with Bun when it has a Bun lockfile,
/// npm otherwise -- through the stack's own Node or Bun, never one elsewhere.
pub fn runner_for(stack: &Stack, dir: &Path) -> Result<Runner> {
    let manifest = dir.join("package.json");
    let text = std::fs::read_to_string(&manifest)
        .map_err(|_| anyhow!("{} has no package.json, so there is no dev server to run", dir.display()))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("reading {}", manifest.display()))?;
    if json.get("scripts").and_then(|s| s.get("dev")).is_none() {
        bail!("{} has no \"dev\" script", manifest.display());
    }
    if dir.join("bun.lock").is_file() || dir.join("bun.lockb").is_file() {
        let bun = stack.root.join("bun").join("current").join("bun.exe");
        if !bun.is_file() {
            bail!("the project uses Bun (it has a bun lockfile), and Bun is not installed: `devcrate install bun`");
        }
        return Ok(Runner::Bun(bun));
    }
    let npm = stack.root.join("node").join("current").join("npm.cmd");
    if !npm.is_file() {
        bail!("Node.js is not installed, or no version is active: `devcrate install node 22`");
    }
    Ok(Runner::Npm(npm))
}

/// The stack's runtimes ahead of everything else on `PATH`, and npm kept
/// inside `node\`.
fn environment(stack: &Stack, command: &mut Command) {
    let mut path: Vec<PathBuf> = [
        stack.root.join("node").join("current"),
        stack.root.join("node").join("npm-global"),
        stack.root.join("bun").join("current"),
        stack.php_dir.join("current"),
        stack.root.join("composer"),
    ]
    .into_iter()
    .filter(|dir| dir.is_dir())
    .collect();
    if let Some(existing) = std::env::var_os("PATH") {
        path.extend(std::env::split_paths(&existing));
    }
    if let Ok(joined) = std::env::join_paths(path) {
        command.env("PATH", joined);
    }
    let node = stack.root.join("node");
    if node.is_dir() {
        command.env("npm_config_cache", node.join("npm-cache"));
        command.env("npm_config_prefix", node.join("npm-global"));
    }
}

// ---------------------------------------------------------------------------
// Start and stop
// ---------------------------------------------------------------------------

/// What a start came to.
#[derive(Debug)]
pub struct Started {
    pub host: String,
    pub pid: u32,
    pub command: String,
    /// Root-relative.
    pub log: String,
    /// `npm install` (or `bun install`) ran first, because there was no
    /// `node_modules`.
    pub installed: bool,
    pub url: Option<String>,
}

/// The site whose project a dev server runs in: `shop`, `shop.test`, either.
fn site_folder(stack: &Stack, name: &str) -> Result<(String, PathBuf)> {
    let host = crate::autosite::host_for(name).ok_or_else(|| anyhow!("{name:?} is not a site name"))?;
    let conf = stack.sites_dir().join(format!("{host}.conf"));
    if !conf.is_file() {
        bail!("no site {host}: {} does not exist", stack.rel(&conf));
    }
    let dir = site::folder(stack, &Site::read(&conf))?;
    Ok((host, dir))
}

pub fn start(stack: &Stack, name: &str, progress: &mut dyn FnMut(&str)) -> Result<Started> {
    let (host, dir) = site_folder(stack, name)?;
    if let Some(record) = read_record(&record_path(stack, &host))
        && ProcessTable::scan().same(record.pid, record.started).is_some()
    {
        bail!("the dev server for {host} is already running (pid {}); `devcrate dev stop {host}` first", record.pid);
    }
    let runner = runner_for(stack, &dir)?;

    let log = log_path(stack, &host);
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    File::create(&log).with_context(|| format!("creating {}", log.display()))?;

    let installed = !dir.join("node_modules").is_dir();
    if installed {
        progress(&format!("{} install", runner.name()));
        let mut install = Command::new(runner.program());
        install.arg("install").current_dir(&dir).stdin(Stdio::null());
        environment(stack, &mut install);
        to_log(&mut install, &log)?;
        #[cfg(windows)]
        install.creation_flags(0x0800_0000 /* CREATE_NO_WINDOW */);
        let status = install.status().with_context(|| format!("running {} install", runner.name()))?;
        if !status.success() {
            bail!(
                "{} install failed (exit code {}); its output is in {}",
                runner.name(),
                status.code().unwrap_or(-1),
                stack.rel(&log)
            );
        }
    }

    progress(&format!("{} run dev", runner.name()));
    let mut dev = Command::new(runner.program());
    dev.args(["run", "dev"]).current_dir(&dir).stdin(Stdio::null());
    environment(stack, &mut dev);
    to_log(&mut dev, &log)?;
    #[cfg(windows)]
    dev.creation_flags(DETACHED);
    keep_std_private();
    let child = dev.spawn().with_context(|| format!("running {} run dev", runner.name()))?;
    let pid = child.id();
    // Not waited on: it outlives this process. Dropping the handle does not
    // stop it.
    drop(child);

    let started = ProcessTable::scan()
        .started_of(pid)
        .ok_or_else(|| anyhow!("{} run dev exited at once; {}", runner.name(), tail(&log)))?;
    let command = format!("{} run dev", runner.name());
    let record = Record { host: host.clone(), pid, started, dir: dir.clone(), command: command.clone() };
    let path = record_path(stack, &host);
    std::fs::create_dir_all(records_dir(stack))?;
    std::fs::write(&path, serde_json::to_string_pretty(&record)? + "\n")
        .with_context(|| format!("writing {}", path.display()))?;

    // Watch it settle: the common failures are immediate.
    let deadline = Instant::now() + SETTLE;
    let mut url = None;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
        if ProcessTable::scan().same(pid, started).is_none() {
            forget(&path, &record);
            bail!("{command} exited at once; {}", tail(&log));
        }
        url = std::fs::read(&log).ok().and_then(|b| url_from_log(&String::from_utf8_lossy(&b)));
        if url.is_some() {
            break;
        }
    }
    Ok(Started { host, pid, command, log: stack.rel(&log), installed, url })
}

/// Mark devcrate's own stdout and stderr as not inheritable. Windows hands a
/// child every inheritable handle, not just the ones it is given as its own
/// output, so without this a dev server -- which outlives devcrate -- would hold
/// the other end of `devcrate dev start x | tee log.txt` open, and the pipe
/// would never close. Its output goes to its log file either way.
fn keep_std_private() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
        use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};
        for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: a standard handle of this process, or null/invalid, which
            // SetHandleInformation rejects without effect.
            unsafe {
                let handle = GetStdHandle(which);
                if !handle.is_null() {
                    SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
                }
            }
        }
    }
}

fn to_log(command: &mut Command, log: &Path) -> Result<()> {
    let file = std::fs::OpenOptions::new()
        .append(true)
        .open(log)
        .with_context(|| format!("opening {}", log.display()))?;
    command.stdout(file.try_clone()?).stderr(file);
    Ok(())
}

/// The last few lines of a log, for an error message.
fn tail(log: &Path) -> String {
    let text = std::fs::read(log).map(|b| strip_ansi(&String::from_utf8_lossy(&b))).unwrap_or_default();
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    match lines.len() {
        0 => "it printed nothing".to_string(),
        n => format!("it said:\n  {}", lines[n.saturating_sub(5)..].join("\n  ")),
    }
}

/// Stop one dev server: its process and everything under it.
pub fn stop(stack: &Stack, name: &str) -> Result<Stopped> {
    let host = crate::autosite::host_for(name).ok_or_else(|| anyhow!("{name:?} is not a site name"))?;
    let path = record_path(stack, &host);
    let Some(record) = read_record(&path) else {
        return Ok(Stopped::NotRunning);
    };
    Ok(stop_record(&path, &record))
}

/// Remove the record only if it is still this one: a `start` that ran while
/// this stop waited has written its own record under the same name, and
/// deleting that would leave its dev server running with nothing to stop it by.
fn forget(path: &Path, record: &Record) {
    if read_record(path).is_some_and(|now| now.pid == record.pid && now.started == record.started) {
        let _ = std::fs::remove_file(path);
    }
}

fn stop_record(path: &Path, record: &Record) -> Stopped {
    let table = ProcessTable::scan();
    if table.same(record.pid, record.started).is_none() {
        forget(path, record);
        return Stopped::NotRunning;
    }
    let mut pids = table.descendants(record.pid);
    pids.push(record.pid);
    let count = pids.iter().filter(|pid| table.kill(**pid)).count();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if ProcessTable::scan().same(record.pid, record.started).is_none() {
            forget(path, record);
            return Stopped::Terminated { count, reason: None };
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Stopped::Failed(format!("pid {} is still running", record.pid))
}

/// Every dev server, as steps of a whole-stack stop.
pub fn stop_all(stack: &Stack) -> Vec<Step<Stopped>> {
    records(stack)
        .into_iter()
        .filter_map(|record| {
            let path = record_path(stack, &record.host);
            let outcome = stop_record(&path, &record);
            // One that had already gone is not worth a line in a stop.
            (!matches!(outcome, Stopped::NotRunning)).then(|| Step {
                id: format!("dev:{}", record.host),
                name: format!("dev {}", record.host),
                outcome,
                helpers: 0,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The CLI
// ---------------------------------------------------------------------------

pub fn print_start(stack: &Stack, name: &str) -> Result<u8> {
    let started = start(stack, name, &mut |step| println!("  ...      {step}"))?;
    if started.installed {
        println!("  installed node_modules (no node_modules was there)");
    }
    println!("  running  {} for {} (pid {})", started.command, started.host, started.pid);
    match &started.url {
        Some(url) => println!("  serving  {url}"),
        None => println!("  serving  (no address printed yet -- see the log)"),
    }
    println!("  log      {}", started.log);
    Ok(crate::exit::OK)
}

pub fn print_stop(stack: &Stack, name: &str) -> Result<u8> {
    match stop(stack, name)? {
        Stopped::NotRunning => println!("no dev server running for {name}"),
        Stopped::Failed(why) => {
            println!("FAILED to stop the dev server for {name}: {why}");
            return Ok(crate::exit::ERROR);
        }
        Stopped::Terminated { count, .. } => println!("stopped the dev server for {name} ({count} process(es))"),
        Stopped::Graceful => println!("stopped the dev server for {name}"),
    }
    Ok(crate::exit::OK)
}

pub fn print_list(stack: &Stack) -> Result<u8> {
    let all = list(stack, &ProcessTable::scan());
    if all.is_empty() {
        println!("No dev servers. `devcrate dev start <site>` runs a project's `npm run dev`.");
        return Ok(crate::exit::OK);
    }
    for dev in all {
        let state = if dev.running { "running" } else { "exited " };
        println!(
            "  {:<24} {state}  pid {:<6} {:<14} {}  {}",
            dev.host,
            dev.pid,
            dev.command,
            dev.url.as_deref().unwrap_or("-"),
            dev.log
        );
    }
    Ok(crate::exit::OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_address_is_read_from_vites_output() {
        let log = "\u{1b}[32m  VITE v6.0.3\u{1b}[39m  ready in 312 ms\n\n  \u{1b}[32m➜\u{1b}[39m  \u{1b}[1mLocal\u{1b}[22m:   \u{1b}[36mhttp://localhost:\u{1b}[1m5173\u{1b}[22m/\u{1b}[39m\n  ➜  Network: use --host to expose\n";
        assert_eq!(url_from_log(log).as_deref(), Some("http://localhost:5173/"));
        assert_eq!(url_from_log("  LARAVEL v12  plugin v1.2.0\n  ➜  APP_URL: https://shop.test\n"), None);
        assert_eq!(url_from_log("nothing yet"), None);
    }

    #[test]
    fn the_runner_follows_the_lockfile_and_needs_a_dev_script() {
        use crate::root::{RootSource, StackRoot};
        let root = std::env::temp_dir().join(format!("devcrate-devserver-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("projects").join("shop");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(root.join("nginx").join("conf")).unwrap();
        std::fs::write(root.join("nginx").join("conf").join("nginx.conf"), "").unwrap();
        let stack = Stack::open(StackRoot { path: root.clone(), source: RootSource::Flag }).unwrap();

        let err = runner_for(&stack, &project).unwrap_err().to_string();
        assert!(err.contains("no package.json"), "{err}");
        std::fs::write(project.join("package.json"), r#"{"scripts":{"build":"vite build"}}"#).unwrap();
        assert!(runner_for(&stack, &project).unwrap_err().to_string().contains("no \"dev\" script"));

        std::fs::write(project.join("package.json"), r#"{"scripts":{"dev":"vite"}}"#).unwrap();
        assert!(runner_for(&stack, &project).unwrap_err().to_string().contains("devcrate install node"));
        let node = root.join("node").join("current");
        std::fs::create_dir_all(&node).unwrap();
        std::fs::write(node.join("npm.cmd"), "").unwrap();
        assert_eq!(runner_for(&stack, &project).unwrap(), Runner::Npm(node.join("npm.cmd")));

        std::fs::write(project.join("bun.lock"), "").unwrap();
        assert!(runner_for(&stack, &project).unwrap_err().to_string().contains("devcrate install bun"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
