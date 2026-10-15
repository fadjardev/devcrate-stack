//! The dashboard: `devcrate` with no arguments.
//!
//! A controller over the same core the subcommands call -- every action here
//! goes through [`crate::control`], [`crate::php`], or [`crate::site`], and
//! nothing is implemented twice. What the dashboard adds is the one thing a
//! command cannot have: it stays resident, so it can notice that a service
//! which *was* up is now down, and say `crashed` rather than `stopped`.
//!
//! The terminal is restored on the way out of every exit path there is --
//! normal return, error, and panic.

mod app;
mod logs;
mod view;
mod worker;

use std::io::{self, IsTerminal, Stdout};
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

use crate::config::Stack;
use crate::exit;
use crate::tui::app::{App, Launch};
use crate::tui::worker::Event;

/// How long to wait for an event before redrawing anyway. Nothing depends on
/// it, but a clock that never ticks makes "working..." look frozen.
const IDLE: Duration = Duration::from_millis(500);
/// ...and while an action runs, often enough for the spinner to turn.
const BUSY: Duration = Duration::from_millis(100);

pub fn run(stack: Stack) -> Result<u8> {
    if !io::stdout().is_terminal() {
        bail!(
            "the dashboard needs a terminal; stdout here is redirected.\n\
             Use `devcrate status` (or `status --json`) for scriptable output."
        );
    }

    let stack = Arc::new(stack);
    let mut terminal = Screen::enter()?;
    let result = event_loop(&mut terminal.inner, stack);
    // Restoring explicitly rather than leaning on the Drop order, so a failure
    // to restore is reported instead of swallowed.
    terminal.leave()?;
    result
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, stack: Arc<Stack>) -> Result<u8> {
    let (tx, rx) = channel::<Event>();
    worker::spawn_input(tx.clone());
    let nudge = worker::spawn_scanner(Arc::clone(&stack), tx.clone());
    let actor = worker::spawn_actor(Arc::clone(&stack), tx.clone(), nudge.clone());
    let tailer = worker::spawn_tailer(tx.clone());

    let mut app = App::new(stack);

    loop {
        terminal.draw(|frame| view::render(frame, &app)).context("drawing the dashboard")?;

        // Block for the first event, then take whatever else has piled up
        // behind it: a scan and three keystrokes should cost one redraw.
        let reading = app.offerings_requested && !app.offerings_loaded;
        let wait = if app.busy.is_some() || reading { BUSY } else { IDLE };
        let mut events = match rx.recv_timeout(wait) {
            Ok(event) => vec![event],
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Vec::new(),
            // Every sender is gone, which can only mean the threads died.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        events.extend(worker::drain(&rx));

        for event in events {
            match event {
                Event::Input(key) => {
                    if let Some(job) = app.on_key(key)
                        && actor.send(job).is_err()
                    {
                        bail!("the worker thread has gone away");
                    }
                }
                Event::Resize => {}
                Event::Status(report) => app.on_status(*report),
                Event::Log(view) => app.on_log(*view),
                Event::Done(result) => app.on_done(*result),
                Event::Offerings(offerings) => app.on_offerings(offerings),
                Event::Progress(text) => app.on_progress(text),
            }
        }

        // The app asks for these by raising a flag rather than holding the
        // channels itself, so it stays a pure state machine and can be tested
        // without any threads at all.
        if app.rescan_wanted {
            app.rescan_wanted = false;
            let _ = nudge.send(());
        }
        if app.offerings_wanted {
            app.offerings_wanted = false;
            worker::spawn_offerings(tx.clone());
        }
        if let Some((what, site)) = app.launch_wanted.take() {
            let (text, failed) = match launch(&app.stack, what, &site) {
                Ok(text) => (text, false),
                Err(err) => (format!("{err:#}"), true),
            };
            app.on_launched(text, failed);
        }
        if app.log_target_dirty {
            app.log_target_dirty = false;
            let _ = tailer.send(app.selected_log());
        }
        if app.quit {
            break;
        }
    }

    Ok(exit::OK)
}

/// Hand a vhost to the browser, Explorer, or a new terminal. Spawned, never
/// waited on, and with none of our handles -- so it is done here on the UI
/// thread rather than queued behind a running job.
fn launch(stack: &Stack, what: Launch, site: &crate::site::Site) -> Result<String> {
    match what {
        Launch::Browser => {
            let url = site.url();
            crate::launch::browser(&url)?;
            Ok(format!("opened {url} in the browser"))
        }
        Launch::Folder => {
            let dir = crate::site::folder(stack, site)?;
            crate::launch::folder(&dir)?;
            Ok(format!("opened {} in Explorer", dir.display()))
        }
        Launch::Terminal => {
            let dir = crate::site::folder(stack, site)?;
            let opened = crate::launch::terminal(&dir)?;
            let name = match opened {
                crate::launch::Terminal::WindowsTerminal => "Windows Terminal",
                crate::launch::Terminal::Cmd => "a cmd window",
            };
            Ok(format!("opened {name} in {}", dir.display()))
        }
    }
}

/// Owns the alternate screen and raw mode for as long as it lives.
///
/// The panic hook is the point of this type. A panic between `enable_raw_mode`
/// and `disable_raw_mode` leaves the shell with no echo and no line editing --
/// the terminal looks broken and the message that caused it is unreadable. The
/// hook restores first, then lets the original hook print normally.
struct Screen {
    inner: Terminal<CrosstermBackend<Stdout>>,
}

impl Screen {
    fn enter() -> Result<Screen> {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = restore();
            previous(info);
        }));

        enable_raw_mode().context("entering raw mode")?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen).context("entering the alternate screen")?;
        let mut inner =
            Terminal::new(CrosstermBackend::new(stdout)).context("starting the terminal")?;
        inner.hide_cursor().ok();
        Ok(Screen { inner })
    }

    fn leave(&mut self) -> Result<()> {
        restore()?;
        self.inner.show_cursor().ok();
        Ok(())
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        // Belt and braces: `leave` is the normal path, but an early `?` between
        // construction and it must not leave the terminal in raw mode.
        let _ = restore();
    }
}

/// Undo everything [`Screen::enter`] did. Safe to call twice.
fn restore() -> Result<()> {
    execute!(io::stdout(), LeaveAlternateScreen).ok();
    disable_raw_mode().context("leaving raw mode")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent};

    use super::*;
    use crate::config::{Service, ServiceKind};
    use crate::root::RootSource;
    use crate::status::{PortStatus, ServiceStatus, State, StatusReport};

    fn stack() -> Arc<Stack> {
        let root = PathBuf::from("C:\\devcrate");
        Arc::new(Stack {
            root: root.clone(),
            root_source: RootSource::Flag,
            config_path: None,
            nginx_prefix: root.join("nginx"),
            nginx_bin: root.join("nginx").join("nginx-1.31.1").join("nginx.exe"),
            php_dir: root.join("php"),
            python_dir: root.join("python"),
            services: vec![
                Service {
                    id: "nginx".into(),
                    name: "nginx".into(),
                    kind: ServiceKind::Nginx,
                    install_marker: root.join("nginx").join("nginx-1.31.1").join("nginx.exe"),
                    process_prefix: root.join("nginx"),
                    exclude_names: Vec::new(),
                    ports: vec![80, 443],
                },
                Service {
                    id: "php-8.5".into(),
                    name: "PHP 8.5".into(),
                    kind: ServiceKind::Php,
                    install_marker: root.join("php").join("php-8.5").join("php-cgi.exe"),
                    process_prefix: root.join("php").join("php-8.5").join("php-cgi.exe"),
                    exclude_names: Vec::new(),
                    ports: vec![9085],
                },
            ],
        })
    }

    fn report(nginx: State, php: State) -> StatusReport {
        let service = |id: &str, name: &str, state: State, port: u16, pids: Vec<u32>| {
            ServiceStatus {
                id: id.into(),
                name: name.into(),
                kind: "x".into(),
                state,
                installed: true,
                path: format!("{id}.exe"),
                ports: vec![PortStatus {
                    port,
                    listening: state == State::Up,
                    holder: None,
                }],
                uptime_secs: (state == State::Up).then_some(4_000),
                pids,
            }
        };
        StatusReport {
            root: "C:\\devcrate".into(),
            root_source: "--root flag".into(),
            config: None,
            cli_php: Some("php-8.5".into()),
            cli_python: None,
            services: vec![
                service("nginx", "nginx", nginx, 80, vec![100]),
                service("php-8.5", "PHP 8.5", php, 9085, vec![200]),
            ],
            sites: vec!["myapp.test".into()],
        }
    }

    /// Render into a buffer and read it back as text, which is the only way to
    /// check a terminal UI without a terminal.
    fn draw(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(96, 22)).unwrap();
        terminal.draw(|frame| view::render(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let area = buffer.area;
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_string()))
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The reason the dashboard has to be a resident process: a service that
    /// goes down on its own is not the same as one that was never started, and
    /// only something holding the previous scan can tell the difference.
    #[test]
    fn a_service_that_dies_on_its_own_is_reported_as_crashed() {
        let mut app = App::new(stack());
        app.on_status(report(State::Up, State::Up));
        assert!(!app.crashed("php-8.5"));

        app.on_status(report(State::Up, State::Stopped));
        assert!(app.crashed("php-8.5"), "it was up a moment ago and nobody asked it to stop");
        assert!(draw(&app).contains("crashed"));

        // Coming back up clears it.
        app.on_status(report(State::Up, State::Up));
        assert!(!app.crashed("php-8.5"));
    }

    /// ...and a service the user stopped is just stopped.
    #[test]
    fn a_service_the_user_stopped_is_not_a_crash() {
        let mut app = App::new(stack());
        app.on_status(report(State::Up, State::Up));

        let job = app.on_key(KeyEvent::from(KeyCode::Char('X'))).expect("X stops the stack");
        assert!(job.stops());
        // The actor would run it; pretend it finished.
        app.busy = None;

        app.on_status(report(State::Stopped, State::Stopped));
        assert!(!app.crashed("php-8.5"));
        assert!(!app.crashed("nginx"));
        assert!(!draw(&app).contains("crashed"));
    }

    /// A key that starts something must not be accepted twice while the first
    /// one is still running -- two `devcrate start`s racing is exactly the
    /// double-MariaDB situation the restart guard exists to avoid.
    #[test]
    fn only_one_action_runs_at_a_time() {
        let mut app = App::new(stack());
        app.on_status(report(State::Stopped, State::Stopped));
        assert!(app.on_key(KeyEvent::from(KeyCode::Char('S'))).is_some());
        assert!(app.on_key(KeyEvent::from(KeyCode::Char('S'))).is_none());
        assert!(app.on_key(KeyEvent::from(KeyCode::Char('X'))).is_none());
    }

    #[test]
    fn the_dashboard_draws_its_panes() {
        let mut app = App::new(stack());
        app.on_status(report(State::Up, State::Up));

        let services = draw(&app);
        assert!(services.contains("Services"), "{services}");
        assert!(services.contains("PHP 8.5"));
        assert!(services.contains("1h 06m"), "uptime is shown: {services}");
        assert!(services.contains("s start"), "the footer names the keys");

        app.on_key(KeyEvent::from(KeyCode::Char('2')));
        assert!(draw(&app).contains("Sites"));

        app.on_key(KeyEvent::from(KeyCode::Char('?')));
        let help = draw(&app);
        assert!(help.contains("start / stop / restart"), "{help}");
    }

    /// Every entry the install picker offers names a version, so choosing it
    /// installs something -- the picker used to hold five that only listed a
    /// catalogue and reported failure.
    #[test]
    fn every_install_picker_entry_names_a_version() {
        let mut app = App::new(stack());
        app.on_key(KeyEvent::from(KeyCode::Char('i')));
        assert!(app.offerings_wanted, "opening the picker asks for the catalogues");
        app.offerings_wanted = false; // as the event loop does once it has spawned the thread
        let reading = draw(&app);
        assert!(reading.contains("reading catalogues"), "{reading}");
        assert!(reading.contains("PHP (newest branch)"), "{reading}");

        for index in 0..app.install_options.len() {
            app.busy = None;
            app.modal = Some(app::Modal::InstallPicker { index });
            let job = app.on_key(KeyEvent::from(KeyCode::Enter)).expect("Enter installs");
            let worker::Job::Install { version, .. } = job else { panic!("not an install") };
            assert!(version.is_some_and(|v| !v.is_empty()), "entry {index} names no version");
        }

        // The catalogues replace the fallback list, and the title settles.
        app.busy = None;
        app.on_key(KeyEvent::from(KeyCode::Char('i')));
        assert!(!app.offerings_wanted, "asked for once per session");
        app.on_offerings(vec![crate::install::Offering {
            runtime: "php",
            label: "PHP 8.5  (8.5.1)".into(),
            version: "8.5".into(),
        }]);
        let loaded = draw(&app);
        assert!(loaded.contains("PHP 8.5  (8.5.1)"), "{loaded}");
        assert!(!loaded.contains("reading catalogues"), "{loaded}");
    }

    /// An install's progress shows in the footer while it runs, and goes with it.
    #[test]
    fn install_progress_shows_in_the_footer() {
        let mut app = App::new(stack());
        app.on_progress("downloading  42%".into());
        assert!(app.progress.is_none(), "nothing is running, so nothing to show");

        app.busy = Some("Downloading & installing redis latest".into());
        app.on_progress("downloading  42%  12.0 MB / 28.6 MB".into());
        let running = draw(&app);
        assert!(running.contains("downloading  42%"), "{running}");

        app.on_done(worker::JobResult {
            label: "Downloading & installing redis latest".into(),
            lines: vec!["Redis 8.10.2 installed into redis".into()],
            failed: false,
            sites_changed: false,
        });
        assert!(app.progress.is_none());
        assert!(!draw(&app).contains("downloading  42%"));
    }

    /// `U` lists what is installed, asks before removing, and the job it
    /// submits names exactly the version picked.
    #[test]
    fn uninstalling_from_the_dashboard_is_confirmed() {
        let mut app = App::new(stack());
        app.on_key(KeyEvent::from(KeyCode::Char('U')));
        assert!(draw(&app).contains("Nothing is installed"), "the fixture root is empty");

        app.modal = Some(app::Modal::UninstallPicker {
            index: 0,
            items: vec![crate::uninstall::Removable {
                runtime: "php",
                version: Some("php-8.5".into()),
                label: "PHP 8.5".into(),
            }],
        });
        assert!(app.on_key(KeyEvent::from(KeyCode::Enter)).is_none(), "asks first");
        let asking = draw(&app);
        assert!(asking.contains("Remove PHP 8.5?"), "{asking}");

        let job = app.on_key(KeyEvent::from(KeyCode::Char('y'))).expect("y confirms");
        assert!(matches!(
            job,
            worker::Job::Uninstall { runtime, version: Some(v), .. } if runtime == "php" && v == "php-8.5"
        ));
    }

    /// Deleting a vhost asks first.
    #[test]
    fn destructive_site_actions_are_confirmed() {
        let mut app = App::new(stack());
        app.tab = app::Tab::Sites;
        app.sites = vec![crate::site::Site {
            host: "myapp.test".into(),
            root: Some("projects/myapp.test/public".into()),
            fastcgi_port: Some(9085),
            tls: true,
        }];

        assert!(app.on_key(KeyEvent::from(KeyCode::Char('d'))).is_none(), "no job yet");
        let asking = draw(&app);
        assert!(asking.contains("Confirm"), "{asking}");
        assert!(asking.contains("left alone"), "it says what is kept: {asking}");

        let job = app.on_key(KeyEvent::from(KeyCode::Char('y'))).expect("y confirms");
        assert!(matches!(job, worker::Job::SiteRemove(host) if host == "myapp.test"));
    }

    /// `o`, `e`, and `c` name what to open and leave the spawning to the loop,
    /// with no job -- so they work while an install is running.
    #[test]
    fn site_quick_actions_ask_the_loop_to_open_things() {
        let mut app = App::new(stack());
        app.tab = app::Tab::Sites;
        app.sites = vec![crate::site::Site {
            host: "myapp.test".into(),
            root: Some("projects/myapp.test/public".into()),
            fastcgi_port: Some(9085),
            tls: true,
        }];
        app.busy = Some("Downloading & installing php 8.4".into());

        let footer = draw(&app);
        assert!(footer.contains("browser") && footer.contains("terminal"), "{footer}");

        for (key, what) in [('o', Launch::Browser), ('e', Launch::Folder), ('c', Launch::Terminal)] {
            assert!(app.on_key(KeyEvent::from(KeyCode::Char(key))).is_none(), "not a job");
            let (asked, site) = app.launch_wanted.take().expect("asked for");
            assert_eq!(asked, what);
            assert_eq!(site.url(), "https://myapp.test/");
        }

        app.sites.clear();
        assert!(app.on_key(KeyEvent::from(KeyCode::Char('o'))).is_none());
        assert!(app.launch_wanted.is_none(), "nothing selected, nothing opened");
    }

    /// Every row of the help fits, the last pane's included.
    #[test]
    fn the_help_shows_every_key() {
        let mut app = App::new(stack());
        app.on_key(KeyEvent::from(KeyCode::Char('?')));
        let mut terminal = Terminal::new(TestBackend::new(100, 34)).unwrap();
        terminal.draw(|frame| view::render(frame, &app)).unwrap();
        let screen = format!("{:?}", terminal.backend().buffer());
        for row in ["switch pane", "open in the browser", "Home / End"] {
            assert!(screen.contains(row), "{row} missing: {screen}");
        }
    }
}
