//! Dashboard state, and what each keystroke does to it.
//!
//! The app owns no threads and does no I/O beyond reading the vhost confs. It
//! turns keys into [`Job`]s for the actor and folds incoming [`Event`]s into
//! something drawable.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{Stack, ServiceKind};
use crate::site::Site;
use crate::status::{State, StatusReport};
use crate::tui::logs::{self, LogFile};
use crate::tui::worker::{Job, JobResult, LogView};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Services,
    Sites,
    Logs,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Services, Tab::Sites, Tab::Logs];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Services => "Services",
            Tab::Sites => "Sites",
            Tab::Logs => "Logs",
        }
    }
}

/// What is on top of the dashboard, if anything.
pub enum Modal {
    Help,
    /// Pick a PHP version. The choice is applied to whatever asked for it.
    PhpPicker { purpose: PhpPurpose, index: usize },
    /// Choose a runtime/service to install from the TUI.
    InstallPicker { index: usize },
    /// Type details for a new vhost.
    NewSite(NewSiteForm),
    /// Confirm something that cannot be undone.
    Confirm { question: String, job: Job },
    /// The full output of the last job, when it was more than one line.
    Output { title: String, lines: Vec<String>, failed: bool },
}

#[derive(Debug, Clone)]
pub struct NewSiteForm {
    pub host: String,
    pub path: String,
    pub php_index: usize, // 0 = Auto-detect, 1.. = PHP versions
    pub update_hosts: bool,
    pub issue_tls: bool,
    pub active_field: usize, // 0: host, 1: path, 2: browse button, 3: php, 4: hosts, 5: tls, 6: submit button
}

impl NewSiteForm {
    pub fn new() -> Self {
        Self {
            host: String::new(),
            path: String::new(),
            php_index: 0,
            update_hosts: true,
            issue_tls: true,
            active_field: 0,
        }
    }
}

pub fn pick_project_folder() -> Option<String> {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "$f = (New-Object -ComObject Shell.Application).BrowseForFolder(0, 'Select Project Directory', 0, 0); if ($f) { [Console]::Write($f.Self.Path) }",
            ])
            .output();
        if let Ok(out) = output {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(path);
            }
        }
    }
    None
}

pub const INSTALL_OPTIONS: [(&'static str, &'static str, Option<&'static str>); 8] = [
    ("node", "Node.js (v22.11.0)", Some("22.11.0")),
    ("bun", "Bun (Latest)", None),
    ("php", "PHP 8.4", Some("8.4")),
    ("php", "PHP 8.3", Some("8.3")),
    ("php", "PHP 8.2", Some("8.2")),
    ("mariadb", "MariaDB Server", None),
    ("rabbitmq", "RabbitMQ + Erlang", None),
    ("composer", "Composer (Latest)", None),
];

pub enum PhpPurpose {
    /// Repoint `php\current`.
    Cli,
    /// Repoint a vhost's `fastcgi_pass`.
    Site(String),
    /// Choose the version for a vhost about to be created.
    #[allow(dead_code)]
    NewSite(String),
}

/// What was last seen of one service, so a change can be noticed.
struct Watch {
    state: State,
    /// Set when the service went down without having been asked to.
    crashed_at: Option<Instant>,
}

/// A one-line note under the table: the result of the last action.
pub struct Note {
    pub text: String,
    pub failed: bool,
    at: Instant,
}

impl Note {
    /// A result is worth reading for a while and then worth forgetting -- a
    /// line saying the stack started is misleading ten minutes later, once
    /// something has stopped.
    pub fn is_stale(&self) -> bool {
        self.at.elapsed() > std::time::Duration::from_secs(20)
    }
}

pub struct App {
    pub stack: Arc<Stack>,
    pub tab: Tab,
    pub report: Option<StatusReport>,
    pub sites: Vec<Site>,
    pub logs: Vec<LogFile>,
    pub log: Option<LogView>,

    pub service_row: usize,
    pub site_row: usize,
    pub log_row: usize,
    /// Lines scrolled up from the bottom of the log pane. 0 is "following".
    pub log_scroll: usize,
    pub follow: bool,

    pub modal: Option<Modal>,
    /// Set while the actor is working. Nothing else may be submitted.
    pub busy: Option<String>,
    pub note: Option<Note>,
    pub quit: bool,

    watches: HashMap<String, Watch>,
    /// Services the user took down on purpose. Not crashes.
    deliberate: HashSet<String>,
    /// Set when the selected log file changes, for the caller to pick up.
    pub log_target_dirty: bool,
    /// Set by `r`, cleared by the caller once the scanner has been nudged.
    pub rescan_wanted: bool,
}

impl App {
    pub fn new(stack: Arc<Stack>) -> App {
        let sites = read_sites(&stack);
        let logs = logs::discover(&stack);
        App {
            stack,
            tab: Tab::Services,
            report: None,
            sites,
            logs,
            log: None,
            service_row: 0,
            site_row: 0,
            log_row: 0,
            log_scroll: 0,
            follow: true,
            modal: None,
            busy: None,
            note: None,
            quit: false,
            watches: HashMap::new(),
            deliberate: HashSet::new(),
            log_target_dirty: true,
            rescan_wanted: false,
        }
    }

    pub fn selected_log(&self) -> Option<PathBuf> {
        self.logs.get(self.log_row).map(|file| file.path.clone())
    }

    /// Service id under the cursor on the Services tab.
    pub fn selected_service(&self) -> Option<&str> {
        self.report
            .as_ref()?
            .services
            .get(self.service_row)
            .map(|service| service.id.as_str())
    }

    pub fn selected_site(&self) -> Option<&Site> {
        self.sites.get(self.site_row)
    }

    /// Has this service gone down without being asked to?
    ///
    /// This is the whole of the "report a crashed child" requirement, and it is
    /// why the dashboard has to be a resident process: a scan on its own cannot
    /// tell a service that was never started from one that died a minute ago.
    /// Remembering the previous state can.
    pub fn crashed(&self, id: &str) -> bool {
        self.watches.get(id).is_some_and(|watch| watch.crashed_at.is_some())
    }

    pub fn php_versions(&self) -> Vec<(String, String)> {
        self.stack
            .php_services()
            .map(|service| (service.id.clone(), service.name.clone()))
            .collect()
    }

    // -----------------------------------------------------------------------
    // Folding events in
    // -----------------------------------------------------------------------

    pub fn on_status(&mut self, report: StatusReport) {
        for service in &report.services {
            let previously = self.watches.get(&service.id).map(|watch| watch.state);
            let watch = self
                .watches
                .entry(service.id.clone())
                .or_insert(Watch { state: service.state, crashed_at: None });

            let went_down = previously == Some(State::Up)
                && matches!(service.state, State::Stopped | State::Absent);
            // An action in flight is taking things down on purpose, whatever
            // the user asked for.
            let asked_for = self.deliberate.contains(&service.id) || self.busy.is_some();
            if went_down && !asked_for {
                watch.crashed_at = Some(Instant::now());
            }
            if service.state == State::Up {
                watch.crashed_at = None;
                self.deliberate.remove(&service.id);
            }
            watch.state = service.state;
        }

        // Keep the cursor on a real row if the service list ever shrinks.
        self.service_row = self.service_row.min(report.services.len().saturating_sub(1));
        self.report = Some(report);
    }

    pub fn on_done(&mut self, result: JobResult) {
        self.busy = None;
        if result.sites_changed {
            self.sites = read_sites(&self.stack);
            self.site_row = self.site_row.min(self.sites.len().saturating_sub(1));
        }
        let summary = result.lines.first().cloned().unwrap_or_else(|| "done".into());
        self.note = Some(Note {
            text: format!("{}: {summary}", result.label),
            failed: result.failed,
            at: Instant::now(),
        });
        // More than one line is worth reading in full -- a stack start has six.
        if result.lines.len() > 1 {
            self.modal = Some(Modal::Output {
                title: result.label,
                lines: result.lines,
                failed: result.failed,
            });
        }
    }

    pub fn on_log(&mut self, view: LogView) {
        // Ignore a report for a file we have since moved off.
        if self.selected_log().as_deref() != Some(view.path.as_path()) {
            return;
        }
        // The picker's sizes were read when the list was built; this one is
        // current, so a log growing under you is visible in the list too.
        if let Some(file) = self.logs.get_mut(self.log_row) {
            file.size = view.bytes;
        }
        self.log = Some(view);
        if self.follow {
            self.log_scroll = 0;
        }
    }

    // -----------------------------------------------------------------------
    // Keys
    // -----------------------------------------------------------------------

    /// Returns a job to run, if this keystroke asked for one.
    pub fn on_key(&mut self, key: KeyEvent) -> Option<Job> {
        if self.modal.is_some() {
            return self.modal_key(key);
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.ask_to_quit(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Char('?') => self.modal = Some(Modal::Help),
            KeyCode::Char('r') => self.rescan_wanted = true,
            KeyCode::Tab => self.cycle_tab(1),
            KeyCode::BackTab => self.cycle_tab(-1),
            KeyCode::Char('1') => self.tab = Tab::Services,
            KeyCode::Char('2') => self.tab = Tab::Sites,
            KeyCode::Char('3') => self.set_tab_logs(),
            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(1),
            _ => return self.tab_key(key),
        }
        None
    }

    /// Leaving in the middle of a shutdown would abandon it half-done, so the
    /// first `q` says so and the second is taken at its word.
    fn ask_to_quit(&mut self) {
        let Some(busy) = self.busy.clone() else {
            self.quit = true;
            return;
        };
        match &self.note {
            Some(note) if note.text.starts_with("Still ") => self.quit = true,
            _ => {
                self.note = Some(Note {
                    text: format!("Still {}. Press q again to quit anyway.", busy.to_lowercase()),
                    failed: true,
                    at: Instant::now(),
                })
            }
        }
    }

    fn tab_key(&mut self, key: KeyEvent) -> Option<Job> {
        match self.tab {
            Tab::Services => self.services_key(key),
            Tab::Sites => self.sites_key(key),
            Tab::Logs => {
                self.logs_key(key);
                None
            }
        }
    }

    fn services_key(&mut self, key: KeyEvent) -> Option<Job> {
        let selected = self.selected_service().map(str::to_string);
        match key.code {
            KeyCode::Char('s') => self.submit(Job::Start(selected)),
            KeyCode::Char('x') => self.submit(Job::Stop(selected)),
            KeyCode::Char('t') => self.submit(Job::Restart(selected)),
            KeyCode::Char('S') => self.submit(Job::Start(None)),
            KeyCode::Char('X') => self.submit(Job::Stop(None)),
            KeyCode::Char('T') => self.submit(Job::Restart(None)),
            KeyCode::Char('u') => {
                self.modal = Some(Modal::PhpPicker { purpose: PhpPurpose::Cli, index: 0 });
                None
            }
            KeyCode::Char('i') => {
                self.modal = Some(Modal::InstallPicker { index: 0 });
                None
            }
            _ => None,
        }
    }

    fn sites_key(&mut self, key: KeyEvent) -> Option<Job> {
        match key.code {
            KeyCode::Char('n') => {
                self.modal = Some(Modal::NewSite(NewSiteForm::new()));
                None
            }
            KeyCode::Char('p') => {
                let host = self.selected_site()?.host.clone();
                self.modal =
                    Some(Modal::PhpPicker { purpose: PhpPurpose::Site(host), index: 0 });
                None
            }
            KeyCode::Char('d') => {
                let host = self.selected_site()?.host.clone();
                self.modal = Some(Modal::Confirm {
                    question: format!(
                        "Delete the vhost conf for {host}?\n\nprojects\\{host} and the certificate are left alone."
                    ),
                    job: Job::SiteRemove(host),
                });
                None
            }
            _ => None,
        }
    }

    fn logs_key(&mut self, key: KeyEvent) {
        let page = 20;
        match key.code {
            KeyCode::Char('f') => {
                self.follow = !self.follow;
                if self.follow {
                    self.log_scroll = 0;
                }
            }
            KeyCode::PageUp => self.scroll_log(page as isize),
            KeyCode::PageDown => self.scroll_log(-(page as isize)),
            KeyCode::Home => {
                self.follow = false;
                self.log_scroll = self.log.as_ref().map(|l| l.lines.len()).unwrap_or(0);
            }
            KeyCode::End => {
                self.follow = true;
                self.log_scroll = 0;
            }
            _ => {}
        }
    }

    fn modal_key(&mut self, key: KeyEvent) -> Option<Job> {
        let versions = self.php_versions();
        if let Some(Modal::NewSite(form)) = &mut self.modal {
            match key.code {
                KeyCode::Esc => self.modal = None,
                KeyCode::Tab | KeyCode::Down => form.active_field = (form.active_field + 1) % 7,
                KeyCode::BackTab | KeyCode::Up => {
                    form.active_field = if form.active_field == 0 { 6 } else { form.active_field - 1 };
                }
                KeyCode::Char('b') if form.active_field != 0 && form.active_field != 1 => {
                    if let Some(picked) = pick_project_folder() {
                        form.path = picked.clone();
                        if form.host.trim().is_empty() {
                            if let Some(name) = std::path::Path::new(&picked).file_name() {
                                form.host = format!("{}.test", name.to_string_lossy());
                            }
                        }
                    }
                }
                KeyCode::Enter => {
                    if form.active_field == 2 {
                        if let Some(picked) = pick_project_folder() {
                            form.path = picked.clone();
                            if form.host.trim().is_empty() {
                                if let Some(name) = std::path::Path::new(&picked).file_name() {
                                    form.host = format!("{}.test", name.to_string_lossy());
                                }
                            }
                        }
                    } else if form.active_field == 6 || (!form.host.trim().is_empty() && (form.active_field == 0 || form.active_field == 1)) {
                        let host = form.host.trim().to_string();
                        if !host.is_empty() {
                            let path = if form.path.trim().is_empty() { None } else { Some(form.path.trim().to_string()) };
                            let php = if form.php_index == 0 { None } else { versions.get(form.php_index - 1).map(|v| v.0.clone()) };
                            let job = Job::SiteAdd {
                                host,
                                path,
                                php,
                                no_hosts: !form.update_hosts,
                                no_tls: !form.issue_tls,
                            };
                            self.modal = None;
                            return self.submit(job);
                        }
                    } else {
                        form.active_field = (form.active_field + 1) % 7;
                    }
                }
                KeyCode::Backspace => match form.active_field {
                    0 => { form.host.pop(); }
                    1 => { form.path.pop(); }
                    _ => {}
                },
                KeyCode::Left => match form.active_field {
                    3 => form.php_index = form.php_index.saturating_sub(1),
                    4 => form.update_hosts = !form.update_hosts,
                    5 => form.issue_tls = !form.issue_tls,
                    _ => {}
                },
                KeyCode::Right | KeyCode::Char(' ') => match form.active_field {
                    2 => {
                        if let Some(picked) = pick_project_folder() {
                            form.path = picked.clone();
                            if form.host.trim().is_empty() {
                                if let Some(name) = std::path::Path::new(&picked).file_name() {
                                    form.host = format!("{}.test", name.to_string_lossy());
                                }
                            }
                        }
                    }
                    3 => {
                        if form.php_index < versions.len() {
                            form.php_index += 1;
                        }
                    }
                    4 => form.update_hosts = !form.update_hosts,
                    5 => form.issue_tls = !form.issue_tls,
                    _ => {}
                },
                KeyCode::Char(c) => match form.active_field {
                    0 => form.host.push(c),
                    1 => form.path.push(c),
                    _ => {}
                },
                _ => {}
            }
            return None;
        }

        let versions = self.php_versions();
        match &mut self.modal {
            Some(Modal::PhpPicker { index, .. }) => match key.code {
                KeyCode::Esc => self.modal = None,
                KeyCode::Up | KeyCode::Char('k') => *index = index.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => {
                    *index = (*index + 1).min(versions.len().saturating_sub(1))
                }
                KeyCode::Enter => {
                    let chosen = versions.get(*index)?.0.clone();
                    let Some(Modal::PhpPicker { purpose, .. }) = self.modal.take() else {
                        return None;
                    };
                    return match purpose {
                        PhpPurpose::Cli => self.submit(Job::PhpUse(chosen)),
                        PhpPurpose::Site(host) => {
                            self.submit(Job::SiteSetPhp { host, version: chosen })
                        }
                        PhpPurpose::NewSite(host) => {
                            self.submit(Job::SiteAdd {
                                host,
                                path: None,
                                php: Some(chosen),
                                no_hosts: false,
                                no_tls: false,
                            })
                        }
                    };
                }
                _ => {}
            },
            Some(Modal::InstallPicker { index }) => match key.code {
                KeyCode::Esc => self.modal = None,
                KeyCode::Up | KeyCode::Char('k') => *index = index.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => {
                    *index = (*index + 1).min(INSTALL_OPTIONS.len().saturating_sub(1));
                }
                KeyCode::Enter => {
                    let (runtime, _label, version) = INSTALL_OPTIONS[*index];
                    let job = Job::Install {
                        runtime: runtime.to_string(),
                        version: version.map(|v| v.to_string()),
                    };
                    self.modal = None;
                    return self.submit(job);
                }
                _ => {}
            },
            Some(Modal::Confirm { .. }) => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    let Some(Modal::Confirm { job, .. }) = self.modal.take() else {
                        return None;
                    };
                    return self.submit(job);
                }
                _ => self.modal = None,
            },
            // Help and Output: any key closes them.
            _ => self.modal = None,
        }
        None
    }

    // -----------------------------------------------------------------------

    /// Hand a job to the actor, unless one is already running. Records what is
    /// being taken down on purpose, so it is not then reported as a crash.
    fn submit(&mut self, job: Job) -> Option<Job> {
        if self.busy.is_some() {
            return None;
        }
        let ids = self.targets_of(&job);
        if job.stops() {
            self.deliberate.extend(ids);
        } else {
            for id in ids {
                self.deliberate.remove(&id);
                if let Some(watch) = self.watches.get_mut(&id) {
                    watch.crashed_at = None;
                }
            }
        }
        self.busy = Some(job.label());
        self.modal = None;
        Some(job)
    }

    /// Which service ids a start/stop actually covers. `php` means all of them,
    /// and no target at all means the whole stack.
    fn targets_of(&self, job: &Job) -> Vec<String> {
        let all = || self.stack.services.iter().map(|s| s.id.clone()).collect::<Vec<_>>();
        match job.target() {
            None => all(),
            Some("php") => self
                .stack
                .php_services()
                .map(|service| service.id.clone())
                .collect(),
            Some(id) => vec![id.to_string()],
        }
    }

    fn cycle_tab(&mut self, by: isize) {
        let count = Tab::ALL.len() as isize;
        let now = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0) as isize;
        let next = Tab::ALL[((now + by).rem_euclid(count)) as usize];
        if next == Tab::Logs {
            self.set_tab_logs();
        } else {
            self.tab = next;
        }
    }

    fn set_tab_logs(&mut self) {
        self.tab = Tab::Logs;
        // Files come and go -- a vhost added this session has no log until it
        // is first requested.
        self.logs = logs::discover(&self.stack);
        self.log_row = self.log_row.min(self.logs.len().saturating_sub(1));
        self.log_target_dirty = true;
    }

    fn move_cursor(&mut self, by: isize) {
        let step = |row: &mut usize, len: usize| {
            if len == 0 {
                *row = 0;
            } else {
                *row = (*row as isize + by).clamp(0, len as isize - 1) as usize;
            }
        };
        match self.tab {
            Tab::Services => {
                let len = self.report.as_ref().map(|r| r.services.len()).unwrap_or(0);
                step(&mut self.service_row, len);
            }
            Tab::Sites => step(&mut self.site_row, self.sites.len()),
            Tab::Logs => {
                let before = self.log_row;
                step(&mut self.log_row, self.logs.len());
                if self.log_row != before {
                    self.log = None;
                    self.log_scroll = 0;
                    self.log_target_dirty = true;
                }
            }
        }
    }

    fn scroll_log(&mut self, by: isize) {
        let len = self.log.as_ref().map(|view| view.lines.len()).unwrap_or(0) as isize;
        let next = (self.log_scroll as isize + by).clamp(0, len);
        self.log_scroll = next as usize;
        // Scrolling up is an explicit statement that you want to read
        // something, so stop yanking the view back to the bottom.
        if self.log_scroll > 0 {
            self.follow = false;
        }
    }

    /// Which service a vhost's FastCGI port belongs to, for the Sites table.
    pub fn php_for_port(&self, port: Option<u16>) -> Option<&str> {
        let port = port?;
        self.stack
            .services
            .iter()
            .find(|service| service.kind == ServiceKind::Php && service.ports.contains(&port))
            .map(|service| service.name.as_str())
    }
}

fn read_sites(stack: &Stack) -> Vec<Site> {
    stack.sites().iter().map(|conf| Site::read(conf)).collect()
}
