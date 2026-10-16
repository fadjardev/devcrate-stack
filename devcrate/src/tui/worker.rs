//! The threads that do everything slow, so the drawing thread does not.
//!
//! Three of them, each with one job:
//!
//! - the **scanner** re-reads the stack's state on a timer. A scan is a full
//!   process-table walk plus a TCP connect per port, and a connect to a dead
//!   port costs the full 250 ms timeout -- roughly two seconds when the stack
//!   is down. That is far too long to spend inside a redraw.
//! - the **actor** runs one start/stop/site/php action at a time. These take
//!   seconds by nature: a cold RabbitMQ node is allowed ninety of them.
//! - the **tailer** follows one log file and reports it when it grows.
//!
//! They only ever send [`Event`]s. Nothing here draws, and nothing here writes
//! to stdout -- the terminal belongs to the UI thread alone.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event as TermEvent, KeyEventKind};

use crate::config::Stack;
use crate::status::{self, StatusReport};
use crate::{control, php, site};

/// The stack the scanner and the actor work on. Shared and swappable rather
/// than fixed at startup: which services exist and where nginx lives are read
/// from disk by [`Stack::open`], so after an install the old value is out of
/// date -- a newly installed PHP version would not even be listed.
pub type SharedStack = Arc<RwLock<Arc<Stack>>>;

fn current(shared: &SharedStack) -> Arc<Stack> {
    Arc::clone(&shared.read().unwrap_or_else(|poisoned| poisoned.into_inner()))
}

/// How often the dashboard re-reads the stack when nothing else prompts it.
pub const REFRESH: Duration = Duration::from_secs(2);

/// How often a followed log file is checked for new bytes.
const TAIL_POLL: Duration = Duration::from_millis(400);

/// Lines kept in memory per log file. Enough to scroll back through a failed
/// start; not enough to matter.
const TAIL_LINES: usize = 2_000;

/// Bytes read from the end of a log file. nginx access logs reach hundreds of
/// megabytes; only the tail is ever wanted.
const TAIL_BYTES: u64 = 256 * 1024;

/// Everything the UI thread reacts to.
pub enum Event {
    Input(event::KeyEvent),
    Resize,
    Status(Box<StatusReport>),
    Log(Box<LogView>),
    Done(Box<JobResult>),
    /// Where a running install is up to, for the footer.
    Progress(String),
    /// A step of `setup` finished, as the line the command would print.
    SetupStep(String),
    /// The stack as read from disk again after a job that could have changed
    /// what is installed. Sent just before that job's `Done`.
    Restacked(Arc<Stack>),
    /// Folders under `projects\` no vhost serves, found by the scanner while
    /// auto-vhost is on. Sent only when there are some.
    Unserved(Vec<String>),
    /// A folder whose vhost was just deleted, not to be served again this
    /// session.
    AutoSkip(String),
    /// The install picker's entries, read from the vendors' catalogues.
    Offerings(Vec<crate::install::Offering>),
}

/// Work the UI can ask for. One at a time, in the order requested.
#[derive(Debug, Clone)]
pub enum Job {
    Start(Option<String>),
    Stop(Option<String>),
    Restart(Option<String>),
    PhpUse(String),
    SiteAdd {
        host: String,
        path: Option<String>,
        php: Option<String>,
        no_hosts: bool,
        no_tls: bool,
    },
    SiteSetPhp { host: String, version: String },
    SiteRemove(String),
    Install { runtime: String, version: Option<String> },
    Uninstall { runtime: String, version: Option<String>, label: String },
    /// Switch one extension of one PHP version on or off.
    PhpExt { php: String, ext: String, on: bool },
    /// Download a PECL extension for one PHP version and switch it on.
    PhpExtInstall { php: String, ext: String },
    /// `devcrate setup`: install what is missing and start the stack.
    Setup,
    /// `devcrate site sync`: serve the folders no vhost serves yet.
    SiteSync,
    /// `devcrate dev start` / `stop` for a site, whichever it is not.
    DevToggle(String),
    /// `devcrate autostart on` / `off`, whichever it is not.
    AutostartToggle,
    /// `devcrate new`: a Laravel or WordPress project, start to finish.
    New { kind: crate::create::Kind, name: String, php: Option<String>, database: crate::create::Database },
}

impl Job {
    /// What the UI shows while this is in flight.
    pub fn label(&self) -> String {
        let whole = |only: &Option<String>| match only {
            Some(id) => id.clone(),
            None => "the stack".to_string(),
        };
        match self {
            Job::Start(only) => format!("Starting {}", whole(only)),
            Job::Stop(only) => format!("Stopping {}", whole(only)),
            Job::Restart(only) => format!("Restarting {}", whole(only)),
            Job::PhpUse(version) => format!("Switching the CLI to PHP {version}"),
            Job::SiteAdd { host, .. } => format!("Creating {host}"),
            Job::SiteSetPhp { host, version } => format!("Pointing {host} at PHP {version}"),
            Job::SiteRemove(host) => format!("Removing {host}"),
            Job::Install { runtime, version } => {
                format!(
                    "Downloading & installing {runtime}{}",
                    version.as_ref().map(|v| format!(" {v}")).unwrap_or_default()
                )
            }
            Job::Uninstall { label, .. } => format!("Removing {label}"),
            Job::PhpExt { php, ext, on: true } => format!("Enabling {ext} on {php}"),
            Job::PhpExt { php, ext, on: false } => format!("Disabling {ext} on {php}"),
            Job::PhpExtInstall { php, ext } => format!("Downloading & installing {ext} for {php}"),
            Job::Setup => "Setting up the stack".to_string(),
            Job::SiteSync => "Serving new folders under projects".to_string(),
            Job::New { kind, name, .. } => format!("Creating the {} project {name}", kind.label()),
            Job::AutostartToggle => "Switching autostart".to_string(),
            Job::DevToggle(host) => format!("Switching the dev server for {host}"),
        }
    }

    /// Whether this job can change what is installed, so the stack has to be
    /// read from disk again once it is done.
    pub fn changes_stack(&self) -> bool {
        matches!(self, Job::Install { .. } | Job::Uninstall { .. } | Job::Setup)
    }

    /// Service ids this job is deliberately taking down, so the dashboard does
    /// not then report them as having crashed.
    pub fn stops(&self) -> bool {
        matches!(self, Job::Stop(_) | Job::Restart(_))
    }

    pub fn target(&self) -> Option<&str> {
        match self {
            Job::Start(only) | Job::Stop(only) | Job::Restart(only) => only.as_deref(),
            _ => None,
        }
    }
}

/// The outcome of one job, in the same words the command line would use.
pub struct JobResult {
    pub label: String,
    pub lines: Vec<String>,
    pub failed: bool,
    /// Whether the vhost list needs re-reading.
    pub sites_changed: bool,
}

/// A log file as last read.
pub struct LogView {
    pub path: PathBuf,
    pub lines: Vec<String>,
    /// File size at the time of reading, so an unchanged file is not resent.
    pub bytes: u64,
    pub truncated: bool,
}

// ---------------------------------------------------------------------------

/// Read keystrokes and hand them over. Blocking reads live here so the UI loop
/// can wait on one channel for everything.
pub fn spawn_input(tx: Sender<Event>) {
    thread::spawn(move || {
        loop {
            let Ok(term_event) = event::read() else { return };
            let sent = match term_event {
                // Windows reports both press and release; acting on both would
                // run every command twice.
                TermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                    tx.send(Event::Input(key))
                }
                TermEvent::Resize(_, _) => tx.send(Event::Resize),
                _ => Ok(()),
            };
            if sent.is_err() {
                return;
            }
        }
    });
}

/// Re-scan on a timer, and immediately whenever nudged. Returns the nudge
/// channel: sending on it makes the next scan happen now rather than in two
/// seconds, which is what an action does when it finishes.
pub fn spawn_scanner(stack: SharedStack, tx: Sender<Event>) -> Sender<()> {
    let (nudge_tx, nudge_rx) = channel::<()>();
    thread::spawn(move || {
        loop {
            let now = current(&stack);
            let report = status::report(&now);
            if tx.send(Event::Status(Box::new(report))).is_err() {
                return;
            }
            // Auto-vhost's look is a directory listing and a canonicalize
            // per folder and vhost -- cheap beside the port probes above.
            if now.auto_sites {
                let unserved: Vec<String> =
                    crate::autosite::plan(&now).new.into_iter().map(|candidate| candidate.folder).collect();
                if !unserved.is_empty() && tx.send(Event::Unserved(unserved)).is_err() {
                    return;
                }
            }
            match nudge_rx.recv_timeout(REFRESH) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    nudge_tx
}

/// Run jobs, one after another, nudging the scanner after each so the display
/// catches up straight away. A job that can change what is installed is
/// followed by a fresh read of the stack, swapped in before the nudge so the
/// next scan already covers what was installed.
pub fn spawn_actor(stack: SharedStack, tx: Sender<Event>, nudge: Sender<()>) -> Sender<Job> {
    let (job_tx, job_rx) = channel::<Job>();
    thread::spawn(move || {
        for job in job_rx {
            let before = current(&stack);
            let reread = job.changes_stack();
            let result = run(&before, job, &tx);
            if reread {
                let root = crate::root::StackRoot { path: before.root.clone(), source: before.root_source };
                // A stack that no longer opens (its devcrate.toml broken in
                // the meantime) keeps the old view rather than none.
                if let Ok(fresh) = Stack::open(root) {
                    let fresh = Arc::new(fresh);
                    *stack.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::clone(&fresh);
                    let _ = tx.send(Event::Restacked(fresh));
                }
            }
            if tx.send(Event::Done(Box::new(result))).is_err() {
                return;
            }
            let _ = nudge.send(());
        }
    });
    job_tx
}

/// Read the vendors' catalogues once, for the install picker. A thread of its
/// own rather than a job, so a slow catalogue never holds up a start or stop.
pub fn spawn_offerings(tx: Sender<Event>) {
    thread::spawn(move || {
        let _ = tx.send(Event::Offerings(crate::install::offerings()));
    });
}

/// Follow one file at a time. Send `Some(path)` to switch, `None` to stop.
pub fn spawn_tailer(tx: Sender<Event>) -> Sender<Option<PathBuf>> {
    let (watch_tx, watch_rx) = channel::<Option<PathBuf>>();
    thread::spawn(move || {
        let mut current: Option<PathBuf> = None;
        let mut last_size = u64::MAX;
        loop {
            match watch_rx.recv_timeout(TAIL_POLL) {
                Ok(next) => {
                    current = next;
                    // Force a read even if the new file happens to be the same
                    // size as the old one.
                    last_size = u64::MAX;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }

            let Some(path) = current.clone() else { continue };
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if size == last_size {
                continue;
            }
            last_size = size;
            if tx.send(Event::Log(Box::new(read_tail(&path, size)))).is_err() {
                return;
            }
        }
    });
    watch_tx
}

/// Read the last [`TAIL_BYTES`] of a file as lines.
///
/// Seeking rather than reading the whole file is what keeps this usable on an
/// access log that has been growing for a month. The first line after a seek is
/// dropped: landing mid-line is the normal case, and half a line is worse than
/// no line.
fn read_tail(path: &Path, size: u64) -> LogView {
    use std::io::{Read, Seek, SeekFrom};

    let mut view =
        LogView { path: path.to_path_buf(), lines: Vec::new(), bytes: size, truncated: false };

    let Ok(mut file) = std::fs::File::open(path) else {
        view.lines.push(format!("(cannot open {})", path.display()));
        return view;
    };

    let from = size.saturating_sub(TAIL_BYTES);
    view.truncated = from > 0;
    if file.seek(SeekFrom::Start(from)).is_err() {
        return view;
    }

    let mut buffer = Vec::new();
    if file.take(TAIL_BYTES + 1).read_to_end(&mut buffer).is_err() {
        return view;
    }

    // Logs are written by nginx, MariaDB and Erlang in whatever encoding the
    // machine uses; lossy conversion keeps a stray byte from losing the file.
    let text = String::from_utf8_lossy(&buffer);
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if view.truncated && !lines.is_empty() {
        lines.remove(0);
    }
    if lines.len() > TAIL_LINES {
        lines.drain(..lines.len() - TAIL_LINES);
        view.truncated = true;
    }
    view.lines = lines;
    view
}

// ---------------------------------------------------------------------------

/// Every job goes through the same core the subcommands call. Nothing here
/// reimplements an action; it only turns the structured result into lines.
fn run(stack: &Stack, job: Job, tx: &Sender<Event>) -> JobResult {
    let label = job.label();
    match job {
        Job::Start(only) => steps(label, control::run_start(stack, only.as_deref()), |step| {
            control::describe_start(kind_of(stack, &step.id), &step.outcome)
        }),
        Job::Stop(only) => steps(label, control::run_stop(stack, only.as_deref()), |step| {
            control::describe(&step.outcome, step.helpers)
        }),
        Job::Restart(only) => {
            let stopped = control::run_stop(stack, only.as_deref());
            let mut result = steps(label.clone(), stopped, |step| {
                control::describe(&step.outcome, step.helpers)
            });
            if result.failed {
                // Same rule as `devcrate restart`: launching a second MariaDB
                // onto a datadir the first one still holds is worse than
                // reporting the failure.
                result.lines.push("not started again -- the stop did not fully succeed".into());
                return result;
            }
            let started = steps(label, control::run_start(stack, only.as_deref()), |step| {
                control::describe_start(kind_of(stack, &step.id), &step.outcome)
            });
            result.lines.extend(started.lines);
            result.failed = started.failed;
            result
        }

        Job::PhpUse(version) => match php::use_version(stack, &version) {
            Ok(switched) => {
                let mut lines = vec![format!("CLI PHP -> {} ({})", switched.name, switched.dir)];
                lines.extend(switched.banner);
                JobResult { label, lines, failed: false, sites_changed: false }
            }
            Err(err) => failure(label, err),
        },

        Job::SiteAdd { host, path, php, no_hosts, no_tls } => {
            let p = path.as_ref().map(Path::new);
            match site::create(stack, &host, p, php.as_deref(), no_hosts, no_tls, false) {
                Ok(made) => {
                    let mut lines = vec![
                        format!("wrote {}", made.conf),
                        format!("https://{} -> {} (fastcgi {})", made.host, made.php_name, made.port),
                        made.reload.note(),
                    ];
                    if made.hosts_updated {
                        lines.push("hosts updated C:\\Windows\\System32\\drivers\\etc\\hosts".to_string());
                    }
                    lines.push(format!("SSL cert: {}", made.cert_name));
                    JobResult { label, lines, failed: false, sites_changed: true }
                }
                Err(err) => failure(label, err),
            }
        },

        Job::SiteSetPhp { host, version } => {
            match site::repoint_site(stack, &host, &version) {
                Ok(done) if done.unchanged => JobResult {
                    label,
                    lines: vec![format!(
                        "{} already serves through {} (fastcgi {})",
                        done.host, done.php_name, done.port
                    )],
                    failed: false,
                    sites_changed: false,
                },
                Ok(done) => {
                    let was = done.was.map(|p| p.to_string()).unwrap_or_else(|| "none".into());
                    JobResult {
                        label,
                        lines: vec![
                            format!("{} : fastcgi {was} -> {}", done.conf, done.port),
                            done.reload.note(),
                            format!("{} has to be running to serve it", done.php_id),
                        ],
                        failed: matches!(done.reload, site::Reload::Failed(_)),
                        sites_changed: true,
                    }
                }
                Err(err) => failure(label, err),
            }
        }

        Job::SiteRemove(host) => match site::delete(stack, &host) {
            Ok(gone) => {
                let mut lines = vec![format!("removed {}", gone.conf), gone.reload.note()];
                if gone.hosts_removed {
                    lines.push(format!("removed {} from the hosts file", gone.host));
                } else if let Some(note) = &gone.hosts_note {
                    lines.push(format!(
                        "hosts file not updated ({note}) -- remove `127.0.0.1 {}` by hand",
                        gone.host
                    ));
                }
                lines.push(format!("{} was left alone", gone.folder));
                // Not again this session: the scanner would otherwise serve
                // it straight back, two seconds after it was deleted.
                if let Some(folder) = &gone.auto_returns {
                    let _ = tx.send(Event::AutoSkip(folder.clone()));
                    lines.push("auto-vhost serves it again next session -- add it to [sites] ignore in devcrate.toml".into());
                }
                JobResult { label, lines, failed: false, sites_changed: true }
            }
            Err(err) => failure(label, err),
        },

        Job::Install { runtime, version } => {
            // `install_quiet` is the pure core `install` wraps for the CLI --
            // it never writes to stdout, unlike `install` itself, which this
            // job used to call directly. See
            // docs/issue/tui-install-stdout-corruption-2026-09-19.md.
            //
            // Not forced: replacing an installed version would also replace
            // whatever was hand-edited inside it (a PHP version's php.ini, for
            // one) without asking. That stays a deliberate CLI `--force`.
            //
            // Progress arrives per chunk read; ten updates a second is plenty
            // for a footer and keeps the channel from flooding.
            let mut last = None::<std::time::Instant>;
            let mut on_progress = |text: &str| {
                if last.is_none_or(|at| at.elapsed() >= Duration::from_millis(100)) {
                    last = Some(std::time::Instant::now());
                    let _ = tx.send(Event::Progress(text.to_string()));
                }
            };
            match crate::install::install_quiet(stack, &runtime, version.as_deref(), false, &mut on_progress) {
                Ok(crate::install::InstallOutcome::Installed(done)) => JobResult {
                    label,
                    lines: vec![format!(
                        "{} {} installed into {}",
                        done.runtime.label(),
                        done.release,
                        done.dir
                    )],
                    failed: false,
                    sites_changed: false,
                },
                // No version was named, so there was nothing to pick a
                // default from (see the runtimes with `None` in
                // `INSTALL_OPTIONS`) and nothing installed -- reported as a
                // failure so it does not read as a false success.
                Ok(crate::install::InstallOutcome::Listed) => JobResult {
                    label,
                    lines: vec![format!(
                        "No version named for {runtime} -- nothing installed. \
                         Run `devcrate install {runtime} <version>` from a terminal."
                    )],
                    failed: true,
                    sites_changed: false,
                },
                // `--force` cannot be passed from here, so name the command
                // that can, rather than an instruction the user cannot follow.
                Err(err) if format!("{err:#}").contains("pass --force") => JobResult {
                    label,
                    lines: vec![format!(
                        "{err:#} -- already installed. To replace it, run \
                         `devcrate install {runtime} {} --force` from a terminal.",
                        version.as_deref().unwrap_or("<version>")
                    )],
                    failed: true,
                    sites_changed: false,
                },
                Err(err) => failure(label, err),
            }
        }

        // Never `--data`, never `--force`: deleting a database's data, the
        // version `current` names, or Erlang under an installed RabbitMQ stays
        // a deliberate command, so those refusals are passed on with it named.
        Job::Uninstall { runtime, version, .. } => {
            match crate::uninstall::uninstall_quiet(stack, &runtime, version.as_deref(), false, false) {
                Ok(done) => {
                    let what = match done.version.is_empty() {
                        true => done.runtime.label().to_string(),
                        false => format!("{} {}", done.runtime.label(), done.version),
                    };
                    let mut lines = vec![format!("{what} removed from {}", done.dir)];
                    if let Some(data) = &done.data_dir {
                        lines.push(format!(
                            "{data} was left in place -- `devcrate uninstall {runtime} --data --force` deletes it"
                        ));
                    }
                    if done.current_cleared {
                        lines.push("it was the active version; `current` has been cleared".into());
                    }
                    JobResult { label, lines, failed: false, sites_changed: false }
                }
                Err(err) if format!("{err:#}").contains("--force") => JobResult {
                    label,
                    lines: vec![format!(
                        "{err:#} -- the dashboard never forces; run `devcrate uninstall {runtime}{} --force` from a terminal.",
                        version.as_deref().map(|v| format!(" {v}")).unwrap_or_default()
                    )],
                    failed: true,
                    sites_changed: false,
                },
                Err(err) => failure(label, err),
            }
        }

        Job::PhpExt { php, ext, on } => {
            let names = [ext.clone()];
            let done = if on {
                crate::php_ext::enable(stack, Some(&php), &names)
            } else {
                crate::php_ext::disable(stack, Some(&php), &names)
            };
            match done {
                Ok(done) => {
                    let state = if on { "on" } else { "off" };
                    let mut lines = vec![match done.changed.is_empty() {
                        true => format!("{ext} was already {state} in {}", done.php_name),
                        false => format!("{ext} switched {state} in {}", done.ini),
                    }];
                    let mut failed = false;
                    if let Some((line, bad)) = restart_line(&done.php_id, &done.restart) {
                        lines.push(line);
                        failed = bad;
                    }
                    JobResult { label, lines, failed, sites_changed: false }
                }
                Err(err) => failure(label, err),
            }
        }

        Job::PhpExtInstall { php, ext } => {
            // Same throttling as an install: a footer needs ten updates a
            // second at most.
            let mut last = None::<std::time::Instant>;
            let mut on_progress = |done: u64, total: Option<u64>| {
                if last.is_none_or(|at| at.elapsed() >= Duration::from_millis(100)) {
                    last = Some(std::time::Instant::now());
                    let text = match total {
                        Some(total) if total > 0 => format!("{ext}: {}%", done * 100 / total),
                        _ => format!("{ext}: {} KB", done / 1024),
                    };
                    let _ = tx.send(Event::Progress(text));
                }
            };
            // Never forced from here, as with installs: replacing a DLL that
            // is already there stays a deliberate `--force` in a terminal.
            match crate::php_ext::install(stack, Some(&php), &ext, None, false, &mut on_progress) {
                Ok(done) => {
                    let mut lines = vec![
                        format!("{} {} {} installed: {}", done.php_name, done.extension, done.version, done.dll),
                        format!("{}, built for {}", done.file_name, done.build),
                        "length-checked over TLS only -- the PECL host publishes no checksum".into(),
                    ];
                    let mut failed = false;
                    if let Some((line, bad)) = restart_line(&done.php_id, &done.restart) {
                        lines.push(line);
                        failed = bad;
                    }
                    JobResult { label, lines, failed, sites_changed: false }
                }
                Err(err) => failure(label, err),
            }
        }

        Job::DevToggle(host) => {
            let running = crate::devserver::list(stack, &crate::probe::ProcessTable::scan())
                .iter()
                .any(|dev| dev.host == host && dev.running);
            let result = match running {
                true => crate::devserver::stop(stack, &host).map(|stopped| match stopped {
                    crate::control::Stopped::Failed(why) => (vec![format!("FAILED to stop it: {why}")], true),
                    _ => (vec![format!("dev server for {host} stopped")], false),
                }),
                false => crate::devserver::start(stack, &host, &mut |step| {
                    let _ = tx.send(Event::Progress(format!("{host}: {step}")));
                })
                .map(|started| {
                    let mut lines = Vec::new();
                    if started.installed {
                        lines.push("installed node_modules first".to_string());
                    }
                    lines.push(format!("{} running (pid {})", started.command, started.pid));
                    lines.push(match started.url {
                        Some(url) => format!("serving {url}"),
                        None => "no address printed yet".to_string(),
                    });
                    lines.push(format!("log {}", started.log));
                    (lines, false)
                }),
            };
            match result {
                Ok((lines, failed)) => JobResult { label, lines, failed, sites_changed: false },
                Err(err) => failure(label, err),
            }
        }

        Job::AutostartToggle => {
            let switched = crate::integration::autostart_status(stack).and_then(|state| match state {
                crate::integration::Autostart::Off => crate::integration::autostart_on(stack).map(|_| {
                    "autostart on: `devcrate start` runs at logon; logs\\autostart.log says what it did"
                }),
                crate::integration::Autostart::On { .. } => {
                    crate::integration::autostart_off(stack).map(|_| "autostart off: the logon task is removed")
                }
            });
            match switched {
                Ok(line) => JobResult { label, lines: vec![line.to_string()], failed: false, sites_changed: false },
                Err(err) => failure(label, err),
            }
        }

        // Composer and artisan write to a log file, never to this screen; the
        // steps go to the footer as they start.
        Job::New { kind, name, php, database } => {
            let log = crate::create::log_path(stack, &name);
            let request = crate::create::Request {
                kind,
                name: &name,
                php: php.as_deref(),
                database: Some(database),
                no_hosts: false,
                no_tls: false,
                output: crate::create::Output::Log(log.clone()),
            };
            let mut last = None::<std::time::Instant>;
            let mut on_progress = |progress: crate::create::Progress| {
                let text = match progress {
                    crate::create::Progress::Step(step) => format!("{name}: {step}"),
                    crate::create::Progress::Bytes(done, total) => {
                        if last.is_some_and(|at| at.elapsed() < Duration::from_millis(100)) {
                            return;
                        }
                        last = Some(std::time::Instant::now());
                        let mb = |b: u64| b as f64 / 1_048_576.0;
                        match total {
                            Some(total) => format!("{name}: downloading {:.1} / {:.1} MB", mb(done), mb(total)),
                            None => format!("{name}: downloading {:.1} MB", mb(done)),
                        }
                    }
                };
                let _ = tx.send(Event::Progress(text));
            };
            match crate::create::run(stack, &request, &mut on_progress) {
                Ok(created) => {
                    let failed = !created.unfinished.is_empty();
                    let mut lines = created.lines;
                    lines.extend(created.unfinished.iter().map(|line| format!("NOT DONE {line}")));
                    if !failed {
                        lines.push(format!("Ready: https://{}", created.host));
                    }
                    JobResult { label, lines, failed, sites_changed: true }
                }
                Err(err) => {
                    let mut result = failure(label, err);
                    if log.is_file() {
                        result.lines.push(format!("log: {}", stack.rel(&log)));
                    }
                    result
                }
            }
        }

        Job::SiteSync => match crate::autosite::sync(stack, crate::autosite::Options::FULL) {
            Ok(synced) => {
                let mut lines = crate::autosite::describe(&synced);
                if lines.is_empty() {
                    lines.push("nothing new to serve".into());
                }
                let failed = lines.iter().any(|line| line.starts_with("FAILED") || line.starts_with("hosts "));
                JobResult { label, lines, failed, sites_changed: true }
            }
            Err(err) => failure(label, err),
        },

        // The same `setup::run` the command calls. Each finished step is sent
        // as it lands, for the first-run screen to tick off; download progress
        // goes to the footer like an install's.
        Job::Setup => {
            let root = crate::root::StackRoot { path: stack.root.clone(), source: stack.root_source };
            let mut lines = Vec::new();
            let mut on_step = |step: &crate::setup::Step| {
                let line = crate::setup::describe(step);
                let _ = tx.send(Event::SetupStep(line.clone()));
                lines.push(line);
            };
            let mut last = None::<std::time::Instant>;
            let mut on_progress = |text: &str| {
                if last.is_none_or(|at| at.elapsed() >= Duration::from_millis(100)) {
                    last = Some(std::time::Instant::now());
                    let _ = tx.send(Event::Progress(text.to_string()));
                }
            };
            match crate::setup::run(&root, true, true, &mut on_step, &mut on_progress) {
                Ok(_) => JobResult { label, lines, failed: false, sites_changed: false },
                Err(err) => {
                    lines.push(format!("stopped at {err:#}"));
                    lines.push("What was installed before it stays installed.".into());
                    JobResult { label, lines, failed: true, sites_changed: false }
                }
            }
        }
    }
}

/// What a restart after an extension change came to, as a line, and whether
/// it failed.
fn restart_line(php: &str, restart: &crate::php_ext::Restart) -> Option<(String, bool)> {
    use crate::php_ext::Restart;
    match restart {
        Restart::NotNeeded => None,
        Restart::NotRunning => Some((format!("{php} is not running; it reads php.ini when it starts"), false)),
        Restart::Done => Some((format!("{php} restarted"), false)),
        Restart::Failed(why) => Some((format!("{php} did not restart cleanly: {why}"), true)),
    }
}

fn steps<T>(
    label: String,
    result: anyhow::Result<Vec<control::Step<T>>>,
    describe: impl Fn(&control::Step<T>) -> String,
) -> JobResult
where
    T: HasFailure,
{
    match result {
        Err(err) => failure(label, err),
        Ok(steps) => JobResult {
            label,
            failed: steps.iter().any(|step| step.outcome.is_failure()),
            lines: steps
                .iter()
                .map(|step| format!("{:<9} {}", step.name, describe(step)))
                .collect(),
            sites_changed: false,
        },
    }
}

fn failure(label: String, err: anyhow::Error) -> JobResult {
    JobResult { label, lines: vec![format!("{err:#}")], failed: true, sites_changed: false }
}

fn kind_of(stack: &Stack, id: &str) -> crate::config::ServiceKind {
    stack
        .services
        .iter()
        .find(|service| service.id == id)
        .map(|service| service.kind)
        .unwrap_or(crate::config::ServiceKind::Nginx)
}

/// Lets [`steps`] ask either outcome type the one question it cares about.
pub trait HasFailure {
    fn is_failure(&self) -> bool;
}

impl HasFailure for control::Started {
    fn is_failure(&self) -> bool {
        control::Started::is_failure(self)
    }
}

impl HasFailure for control::Stopped {
    fn is_failure(&self) -> bool {
        control::Stopped::is_failure(self)
    }
}

/// Drain everything already queued, so a burst of scans does not make the UI
/// redraw once per scan.
pub fn drain(rx: &Receiver<Event>) -> Vec<Event> {
    rx.try_iter().collect()
}
