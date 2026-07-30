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
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event as TermEvent, KeyEventKind};

use crate::config::Stack;
use crate::status::{self, StatusReport};
use crate::{control, php, site};

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
}

/// Work the UI can ask for. One at a time, in the order requested.
#[derive(Debug, Clone)]
pub enum Job {
    Start(Option<String>),
    Stop(Option<String>),
    Restart(Option<String>),
    PhpUse(String),
    SiteAdd { host: String, php: Option<String> },
    SiteSetPhp { host: String, version: String },
    SiteRemove(String),
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
        }
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
pub fn spawn_scanner(stack: Arc<Stack>, tx: Sender<Event>) -> Sender<()> {
    let (nudge_tx, nudge_rx) = channel::<()>();
    thread::spawn(move || {
        loop {
            let report = status::report(&stack);
            if tx.send(Event::Status(Box::new(report))).is_err() {
                return;
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
/// catches up straight away.
pub fn spawn_actor(stack: Arc<Stack>, tx: Sender<Event>, nudge: Sender<()>) -> Sender<Job> {
    let (job_tx, job_rx) = channel::<Job>();
    thread::spawn(move || {
        for job in job_rx {
            let result = run(&stack, job);
            if tx.send(Event::Done(Box::new(result))).is_err() {
                return;
            }
            let _ = nudge.send(());
        }
    });
    job_tx
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
fn run(stack: &Stack, job: Job) -> JobResult {
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

        Job::SiteAdd { host, php } => {
            match site::create(stack, &host, None, php.as_deref(), false, false, false) {
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
            Ok(gone) => JobResult {
                label,
                lines: vec![
                    format!("removed {}", gone.conf),
                    gone.reload.note(),
                    format!("projects\\{} was left alone", gone.host),
                ],
                failed: false,
                sites_changed: true,
            },
            Err(err) => failure(label, err),
        },
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
