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
mod theme;
mod view;
mod worker;

use std::io::{self, IsTerminal, Stdout};
use std::sync::mpsc::channel;
use std::sync::{Arc, RwLock};
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
    let shared: worker::SharedStack = Arc::new(RwLock::new(Arc::clone(&stack)));
    worker::spawn_input(tx.clone());
    let nudge = worker::spawn_scanner(Arc::clone(&shared), tx.clone());
    let actor = worker::spawn_actor(shared, tx.clone(), nudge.clone());
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
                Event::SetupStep(line) => app.on_setup_step(line),
                Event::Restacked(stack) => app.on_restacked(stack),
                Event::AutoSkip(folder) => app.on_auto_skip(folder),
                Event::Unserved(folders) => {
                    if let Some(job) = app.on_unserved(folders)
                        && actor.send(job).is_err()
                    {
                        bail!("the worker thread has gone away");
                    }
                }
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
            auto_sites: true,
            auto_sites_ignore: Vec::new(),
            auto_renew_certs: true,
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
            dev: Vec::new(),
            certs: Vec::new(),
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
            proxy_port: None,
            tls: true,
        }];

        assert!(app.on_key(KeyEvent::from(KeyCode::Char('d'))).is_none(), "no job yet");
        let asking = draw(&app);
        assert!(asking.contains("Confirm"), "{asking}");
        assert!(asking.contains("left alone"), "it says what is kept: {asking}");

        let job = app.on_key(KeyEvent::from(KeyCode::Char('y'))).expect("y confirms");
        assert!(matches!(job, worker::Job::SiteRemove(host) if host == "myapp.test"));
    }

    /// `N` opens the quick-create form: a name that cannot be a host is
    /// refused in the form, and a good one becomes a `devcrate new` job with
    /// the database chosen -- WordPress offering MariaDB alone.
    #[test]
    fn a_new_project_is_chosen_in_a_form() {
        let mut app = App::new(stack());
        app.tab = app::Tab::Sites;
        assert!(draw(&app).contains("new project"), "the footer names N");

        app.on_key(KeyEvent::from(KeyCode::Char('N')));
        let form = draw(&app);
        assert!(form.contains("New project") && form.contains("Laravel"), "{form}");
        assert!(form.contains("MariaDB"), "MariaDB is the default: {form}");

        app.on_key(KeyEvent::from(KeyCode::Tab));
        for c in "my app".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        assert!(app.on_key(KeyEvent::from(KeyCode::Enter)).is_none(), "refused");
        assert!(draw(&app).contains("cannot be a hostname"));

        app.on_key(KeyEvent::from(KeyCode::Backspace));
        app.on_key(KeyEvent::from(KeyCode::Backspace));
        app.on_key(KeyEvent::from(KeyCode::Backspace));
        app.on_key(KeyEvent::from(KeyCode::Backspace));
        app.on_key(KeyEvent::from(KeyCode::Char('-')));
        app.on_key(KeyEvent::from(KeyCode::Char('b')));
        assert!(draw(&app).contains("https://my-b.test"), "the host is previewed");

        // Database: MariaDB -> PostgreSQL -> SQLite.
        app.on_key(KeyEvent::from(KeyCode::Tab));
        app.on_key(KeyEvent::from(KeyCode::Tab));
        app.on_key(KeyEvent::from(KeyCode::Right));
        app.on_key(KeyEvent::from(KeyCode::Right));
        assert!(draw(&app).contains("SQLite"));

        let job = app.on_key(KeyEvent::from(KeyCode::Enter)).expect("a job");
        assert!(matches!(
            job,
            worker::Job::New { kind: crate::create::Kind::Laravel, ref name, php: None, database: crate::create::Database::Sqlite }
                if name == "my-b"
        ));
        assert!(app.modal.is_none());

        // WordPress has one database to offer, whatever was chosen before.
        app.busy = None;
        app.on_key(KeyEvent::from(KeyCode::Char('N')));
        app.on_key(KeyEvent::from(KeyCode::Right));
        for _ in 0..3 {
            app.on_key(KeyEvent::from(KeyCode::Tab));
        }
        app.on_key(KeyEvent::from(KeyCode::Right));
        let form = draw(&app);
        assert!(form.contains("WordPress") && form.contains("MariaDB"), "{form}");
    }

    /// The Sites table has a DEV column from the scan -- the port of a running
    /// dev server, or that it exited -- and `v` asks for a toggle of the
    /// selected site's.
    #[test]
    fn dev_servers_show_in_the_sites_pane() {
        let mut app = App::new(stack());
        let mut status = report(State::Up, State::Up);
        status.dev = vec![
            crate::devserver::DevStatus {
                host: "myapp.test".into(),
                running: true,
                pid: 4242,
                command: "npm run dev".into(),
                url: Some("http://localhost:5173/".into()),
                log: "logs\\dev-myapp.test.log".into(),
            },
            crate::devserver::DevStatus {
                host: "shop.test".into(),
                running: false,
                pid: 4343,
                command: "npm run dev".into(),
                url: None,
                log: "logs\\dev-shop.test.log".into(),
            },
        ];
        app.on_status(status);
        app.tab = app::Tab::Sites;
        app.sites = vec![
            crate::site::Site {
                host: "myapp.test".into(),
                root: Some("projects/myapp/public".into()),
                fastcgi_port: Some(9085),
                proxy_port: None,
                tls: true,
            },
            crate::site::Site {
                host: "shop.test".into(),
                root: Some("projects/shop".into()),
                fastcgi_port: Some(9085),
                proxy_port: None,
                tls: true,
            },
        ];
        let screen = draw(&app);
        assert!(screen.contains("DEV"), "{screen}");
        assert!(screen.contains("● :5173"), "{screen}");
        assert!(screen.contains("✗ exited"), "{screen}");
        assert!(screen.contains("v dev"), "the footer names v: {screen}");
        assert!(screen.contains("CERT"), "{screen}");

        let job = app.on_key(KeyEvent::from(KeyCode::Char('v'))).expect("a job");
        assert!(matches!(job, worker::Job::DevToggle(ref host) if host == "myapp.test"));
    }

    /// The CERT column says how long each site's certificate has, or what is
    /// wrong with it, and `R` asks for the selected site's to be issued again.
    #[test]
    fn certificates_show_in_the_sites_pane() {
        use crate::cert::{CertState, SiteCert};
        let mut app = App::new(stack());
        let mut status = report(State::Up, State::Up);
        let cert = |host: &str, state: CertState, days: i64| SiteCert {
            host: host.into(),
            file: format!("certs/_wildcard.{host}.pem"),
            state,
            days_left: Some(days),
            expires: None,
            names: Vec::new(),
        };
        status.certs = vec![
            cert("myapp.test", CertState::Ok, 800),
            cert("old.test", CertState::Expiring, 12),
            cert("moved.test", CertState::ForeignCa, 700),
        ];
        app.on_status(status);
        app.tab = app::Tab::Sites;
        let site = |host: &str| crate::site::Site {
            host: host.into(),
            root: Some(format!("projects/{host}")),
            fastcgi_port: Some(9085),
            proxy_port: None,
            tls: true,
        };
        app.sites = vec![site("myapp.test"), site("old.test"), site("moved.test")];
        let screen = draw(&app);
        assert!(screen.contains("✓ 2y"), "{screen}");
        assert!(screen.contains("! 12d"), "{screen}");
        assert!(screen.contains("✗ CA"), "{screen}");

        let job = app.on_key(KeyEvent::from(KeyCode::Char('R'))).expect("a job");
        assert!(matches!(job, worker::Job::CertRenew(ref host) if host == "myapp.test"));
    }

    /// A proxy vhost shows where it forwards in the PHP column, and the New
    /// Site form takes a port -- digits only -- that makes the site a proxy.
    #[test]
    fn a_proxy_site_is_shown_and_made_from_the_form() {
        let mut app = App::new(stack());
        app.on_status(report(State::Up, State::Up));
        app.tab = app::Tab::Sites;
        app.sites = vec![crate::site::Site {
            host: "api.test".into(),
            root: Some("projects/api".into()),
            fastcgi_port: None,
            proxy_port: Some(3000),
            tls: true,
        }];
        let screen = draw(&app);
        assert!(screen.contains("→ :3000"), "{screen}");
        assert!(!screen.contains("no fastcgi_pass"), "{screen}");

        app.on_key(KeyEvent::from(KeyCode::Char('n')));
        for c in "front.test".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        for _ in 0..4 {
            app.on_key(KeyEvent::from(KeyCode::Tab));
        }
        for c in "51x73".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        let form = draw(&app);
        assert!(form.contains("localhost:5173"), "letters are not taken: {form}");
        let job = app.on_key(KeyEvent::from(KeyCode::Enter)).expect("a job");
        assert!(matches!(job, worker::Job::SiteAdd { ref host, proxy: Some(5173), .. } if host == "front.test"));
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
            proxy_port: None,
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

    /// `e` on a PHP row lists that version's extensions, offers Xdebug when
    /// it has no DLL for it, and turns a row into the job that switches it.
    #[test]
    fn the_extension_picker_switches_the_selected_version() {
        let root = std::env::temp_dir().join(format!("devcrate-tui-ext-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let php = root.join("php").join("php-8.5");
        std::fs::create_dir_all(php.join("ext")).unwrap();
        for file in ["php-cgi.exe", "php.exe", "ext/php_curl.dll", "ext/php_pgsql.dll"] {
            std::fs::write(php.join(file), "").unwrap();
        }
        std::fs::write(php.join("php.ini"), "extension=curl\n").unwrap();
        let stack = Arc::new(
            Stack::open(crate::root::StackRoot { path: root.clone(), source: RootSource::Flag })
                .unwrap(),
        );
        let mut app = App::new(stack);
        app.on_status(report(State::Up, State::Up));

        app.service_row = 0; // nginx
        app.on_key(KeyEvent::from(KeyCode::Char('e')));
        assert!(app.modal.is_none());
        assert!(app.note.as_ref().is_some_and(|n| n.text.contains("PHP row")));

        app.service_row = 1; // php-8.5
        app.on_key(KeyEvent::from(KeyCode::Char('e')));
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal.draw(|frame| view::render(frame, &app)).unwrap();
        let screen = format!("{:?}", terminal.backend().buffer());
        for want in ["extensions", "xdebug", "curl", "php.ini line 1", "pgsql"] {
            assert!(screen.contains(want), "{want} missing: {screen}");
        }

        let job = app.on_key(KeyEvent::from(KeyCode::Enter)).expect("first row installs");
        assert!(matches!(job, worker::Job::PhpExtInstall { php, ext } if php == "php-8.5" && ext == "xdebug"));
        app.busy = None;

        app.on_key(KeyEvent::from(KeyCode::Char('e')));
        app.on_key(KeyEvent::from(KeyCode::Down));
        app.on_key(KeyEvent::from(KeyCode::Down));
        let job = app.on_key(KeyEvent::from(KeyCode::Char(' '))).expect("pgsql switches on");
        assert!(matches!(job, worker::Job::PhpExt { php, ext, on: true } if php == "php-8.5" && ext == "pgsql"));

        let _ = std::fs::remove_dir_all(&root);
    }

    fn nothing_installed() -> StatusReport {
        let mut report = report(State::Absent, State::Absent);
        for service in &mut report.services {
            service.installed = false;
        }
        report.cli_php = None;
        report
    }

    /// A first scan that finds nothing installed puts up the first-run
    /// screen; `s` runs setup, whose steps tick off as they land. A failure
    /// keeps the screen with the reason; success leaves it for the dashboard.
    #[test]
    fn an_empty_stack_opens_on_the_first_run_screen() {
        let mut app = App::new(stack());
        app.on_status(nothing_installed());
        let screen = draw(&app);
        for want in ["Welcome to devcrate", "Nothing is installed in this stack yet:", "    C:\\devcrate","set up a working stack", "Windows asks once", "s set up"] {
            assert!(screen.contains(want), "{want} missing:\n{screen}");
        }
        assert!(!screen.contains("SERVICE"), "the table is replaced:\n{screen}");

        // Keys of the dashboard proper do nothing here.
        assert!(app.on_key(KeyEvent::from(KeyCode::Char('S'))).is_none());
        let job = app.on_key(KeyEvent::from(KeyCode::Char('s'))).expect("s runs setup");
        assert!(matches!(job, worker::Job::Setup));

        app.on_setup_step("nginx     nginx 1.30.6 installed into nginx\\nginx-1.30.6".into());
        app.on_progress("downloading PHP  42%".into());
        let screen = draw(&app);
        assert!(screen.contains("✓ nginx     nginx 1.30.6 installed"), "{screen}");
        assert!(screen.contains("downloading PHP  42%"), "{screen}");
        assert!(screen.contains("look behind this window"), "{screen}");
        assert!(!screen.contains("Esc skip"), "nothing but q works while it runs:\n{screen}");
        assert!(!screen.contains("set up a working stack"), "the offer gives way to progress:\n{screen}");

        app.on_done(worker::JobResult {
            label: "Setting up the stack".into(),
            lines: vec![
                "nginx     nginx 1.30.6 installed into nginx\\nginx-1.30.6".into(),
                "stopped at PHP: no route to host".into(),
                "What was installed before it stays installed.".into(),
            ],
            failed: true,
            sites_changed: false,
        });
        let screen = draw(&app);
        assert!(screen.contains("✗ stopped at PHP: no route to host"), "{screen}");
        assert!(screen.contains("s carry on"), "{screen}");
        assert!(screen.contains("Setting up this stack:"), "{screen}");
        assert!(app.modal.is_none());

        app.on_key(KeyEvent::from(KeyCode::Char('s'))).expect("s carries on");
        assert!(app.welcome.as_ref().is_some_and(|w| w.steps.is_empty() && w.error.is_empty()));
        app.on_done(worker::JobResult {
            label: "Setting up the stack".into(),
            lines: vec!["nginx     already there: nginx at nginx\\current\\nginx.exe".into(), "start     3 started, 3 up".into()],
            failed: false,
            sites_changed: false,
        });
        assert!(app.welcome.is_none(), "success leaves for the dashboard");
        assert!(app.note.as_ref().is_some_and(|n| n.text.ends_with("start 3 started, 3 up")), "the note says where it ended up");
        assert!(matches!(app.modal, Some(app::Modal::Output { failed: false, .. })));
    }

    /// More steps than fit, with long paths: the newest step, the reason it
    /// stopped, and the key to carry on all stay on screen.
    #[test]
    fn the_first_run_screen_keeps_its_ending_in_view() {
        let mut app = App::new(stack());
        app.on_status(nothing_installed());
        app.on_key(KeyEvent::from(KeyCode::Char('s')));
        let long = "C:\\Users\\someone\\AppData\\Local\\Temp\\a-rather-long-folder-name\\devcrate\\nginx\\nginx.exe";
        for i in 0..14 {
            app.on_setup_step(format!("step{i:02}    already there: {long}"));
        }
        let mut lines: Vec<String> = app.welcome.as_ref().unwrap().steps.clone();
        lines.push("stopped at start: MariaDB FAILED: exited immediately; nginx FAILED: exited immediately".into());
        lines.push("What was installed before it stays installed.".into());
        app.on_done(worker::JobResult { label: "Setting up the stack".into(), lines, failed: true, sites_changed: false });
        let screen = draw(&app);
        for want in ["step13", "✗ stopped at start", "s carry on"] {
            assert!(screen.contains(want), "{want} missing:\n{screen}");
        }
    }

    /// A folder the scanner reports unserved is served once per session, and
    /// never in the middle of something else.
    #[test]
    fn unserved_folders_are_served_once_and_never_mid_action() {
        let mut app = App::new(stack());
        app.on_status(report(State::Up, State::Up));

        app.on_key(KeyEvent::from(KeyCode::Char('?')));
        assert!(app.on_unserved(vec!["shop".into()]).is_none(), "not over an open modal");
        app.modal = None;

        let job = app.on_unserved(vec!["shop".into()]).expect("served");
        assert!(matches!(job, worker::Job::SiteSync));
        assert!(app.on_unserved(vec!["blog".into()]).is_none(), "not while the first runs");
        app.on_done(worker::JobResult {
            label: "Serving new folders under projects".into(),
            lines: vec!["FAILED   shop: no PHP".into()],
            failed: true,
            sites_changed: true,
        });

        assert!(app.on_unserved(vec!["shop".into()]).is_none(), "a failed folder is not retried every scan");
        assert!(app.on_unserved(vec!["shop".into(), "blog".into()]).is_some(), "a new one still is");
    }

    /// Only the first scan decides: a stack with something installed never
    /// shows the screen, even after everything is uninstalled; Esc skips it.
    #[test]
    fn the_first_run_screen_is_decided_once() {
        let mut app = App::new(stack());
        app.on_status(report(State::Up, State::Up));
        app.on_status(nothing_installed());
        assert!(app.welcome.is_none());
        assert!(draw(&app).contains("SERVICE"));

        let mut app = App::new(stack());
        app.on_status(nothing_installed());
        assert!(app.welcome.is_some());
        assert!(app.on_key(KeyEvent::from(KeyCode::Esc)).is_none());
        assert!(app.welcome.is_none());
        assert!(!app.quit, "Esc skips, it does not quit");
        assert!(draw(&app).contains("SERVICE"));
    }

    /// A job that can change what is installed is followed by a fresh read of
    /// the stack, so a version installed from the dashboard is listed without
    /// reopening it -- even when the job itself failed partway.
    #[test]
    fn the_actor_rereads_the_stack_after_an_install() {
        let root = std::env::temp_dir().join(format!("devcrate-tui-restack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("php")).unwrap();
        let open = || Stack::open(crate::root::StackRoot { path: root.clone(), source: RootSource::Flag }).unwrap();
        let shared: worker::SharedStack = Arc::new(RwLock::new(Arc::new(open())));
        assert!(!shared.read().unwrap().services.iter().any(|s| s.id == "php-8.4"));

        std::fs::create_dir_all(root.join("php").join("php-8.4")).unwrap();
        let (tx, rx) = channel();
        let (nudge, _nudged) = channel();
        let actor = worker::spawn_actor(Arc::clone(&shared), tx, nudge);
        actor.send(worker::Job::Install { runtime: "no-such-runtime".into(), version: None }).unwrap();

        let restacked = match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
            Event::Restacked(stack) => stack,
            _ => panic!("the fresh stack comes before Done"),
        };
        assert!(restacked.services.iter().any(|s| s.id == "php-8.4"));
        assert!(shared.read().unwrap().services.iter().any(|s| s.id == "php-8.4"), "the scanner sees it too");
        assert!(matches!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), Event::Done(done) if done.failed));

        let mut app = App::new(Arc::new(open()));
        app.on_restacked(restacked);
        assert!(app.php_versions().iter().any(|(id, _)| id == "php-8.4"));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn draw_at(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| view::render(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_string()))
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A screen with room to spare gets the overview cards and a panel about
    /// the selected row; a small one gets neither, and loses nothing it had.
    #[test]
    fn a_large_screen_adds_the_overview_and_the_detail_panel() {
        let mut app = App::new(stack());
        app.on_status(report(State::Up, State::Stopped));
        app.service_row = 1;

        let large = draw_at(&app, 140, 34);
        for want in ["STACK", "1/2 up", "CERTIFICATES", "COMMAND LINE", "php-8.5", "Selected", "9085 closed", "s starts it."] {
            assert!(large.contains(want), "{want} missing:\n{large}");
        }

        let small = draw(&app);
        assert!(!small.contains("STACK") && !small.contains("Selected"), "{small}");
        assert!(small.contains("SERVICE") && small.contains("PHP 8.5"), "{small}");

        app.tab = app::Tab::Sites;
        app.sites = vec![crate::site::Site {
            host: "myapp.test".into(),
            root: Some("projects/myapp/public".into()),
            fastcgi_port: Some(9085),
            proxy_port: None,
            tls: true,
        }];
        let sites = draw_at(&app, 140, 34);
        for want in ["1 vhost", "https://myapp.test/", "serves", "PHP 8.5", "not running"] {
            assert!(sites.contains(want), "{want} missing:\n{sites}");
        }
    }

    /// The footer drops hints that do not fit rather than cutting the line
    /// off, and help and quit are the last to go.
    #[test]
    fn a_narrow_footer_keeps_help_and_quit() {
        let mut app = App::new(stack());
        app.on_status(report(State::Up, State::Up));
        let narrow = draw_at(&app, 60, 22);
        let footer = narrow.lines().last().unwrap();
        assert!(footer.contains("s start") && footer.contains("? help") && footer.contains("q quit"), "{footer}");
        assert!(!footer.contains("uninstall"), "{footer}");

        let wide = draw_at(&app, 130, 22);
        assert!(wide.lines().last().unwrap().contains("A autostart"), "{wide}");
    }

    /// A result longer than the screen keeps its ending -- where the failure
    /// is -- and says how much came before; a line with a break of its own is
    /// two lines, not one with a stray character in it.
    #[test]
    fn a_long_result_keeps_its_ending_in_view() {
        let mut app = App::new(stack());
        app.on_status(report(State::Up, State::Up));
        let mut lines: Vec<String> = (0..40).map(|i| format!("line {i:02}")).collect();
        lines.push("NOT DONE migrate: exited with code 1\n         finish with: php artisan migrate".into());
        app.on_done(worker::JobResult { label: "Creating the Laravel project shop".into(), lines, failed: true, sites_changed: false });
        let screen = draw(&app);
        for want in ["earlier line(s) not shown", "NOT DONE migrate", "finish with: php artisan migrate", "any key closes"] {
            assert!(screen.contains(want), "{want} missing:\n{screen}");
        }
        assert!(!screen.contains("│ line 00"), "the start gives way: {screen}");
    }

    /// Home shows the top of the log, not an empty pane above it.
    #[test]
    fn home_shows_the_first_page_of_a_log() {
        let mut app = App::new(stack());
        app.tab = app::Tab::Logs;
        let path = PathBuf::from("C:\\devcrate\\nginx\\logs\\error.log");
        app.logs = vec![logs::LogFile { label: "nginx  error".into(), path: path.clone(), size: 1 }];
        app.on_log(worker::LogView {
            path,
            lines: (0..200).map(|i| format!("entry {i:03}")).collect(),
            bytes: 1,
            truncated: false,
        });
        assert!(draw(&app).contains("entry 199"), "it follows the end");
        app.on_key(KeyEvent::from(KeyCode::Home));
        let top = draw(&app);
        assert!(top.contains("entry 000") && !top.contains("(empty)"), "{top}");
    }

    /// Every pane's footer fits a 100-column terminal, `q quit` included.
    #[test]
    fn footers_fit_a_hundred_columns() {
        let mut app = App::new(stack());
        for tab in app::Tab::ALL {
            app.tab = tab;
            let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
            terminal.draw(|frame| view::render(frame, &app)).unwrap();
            let buffer = terminal.backend().buffer().clone();
            let y = buffer.area.height - 1;
            let footer: String = (0..buffer.area.width)
                .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_string()))
                .collect();
            assert!(footer.contains("q quit"), "{}: {footer}", tab.title());
        }
    }

    /// Draws the dashboard for the README: `cargo test screenshots -- --ignored`
    /// writes `docs/img/dashboard-*.svg`. The view code is the real one; the
    /// stack is a demo -- a fixed set of services and sites -- so the picture
    /// shows no one's projects and does not change with whatever is running.
    #[test]
    #[ignore = "writes docs/img; run on purpose"]
    fn screenshots() {
        let root = PathBuf::from("C:\\devcrate");
        let svc = |id: &str, name: &str, kind: ServiceKind, ports: Vec<u16>| Service {
            id: id.into(),
            name: name.into(),
            kind,
            install_marker: root.join(id),
            process_prefix: root.join(id),
            exclude_names: Vec::new(),
            ports,
        };
        let mut stack = Arc::try_unwrap(stack()).unwrap();
        stack.services = vec![
            svc("nginx", "nginx", ServiceKind::Nginx, vec![80, 443]),
            svc("php-7.4", "PHP 7.4", ServiceKind::Php, vec![9074]),
            svc("php-8.2", "PHP 8.2", ServiceKind::Php, vec![9082]),
            svc("php-8.5", "PHP 8.5", ServiceKind::Php, vec![9085]),
            svc("mariadb", "MariaDB", ServiceKind::MariaDb, vec![3306]),
            svc("postgres", "PostgreSQL", ServiceKind::Postgres, vec![5432]),
            svc("rabbitmq", "RabbitMQ", ServiceKind::RabbitMq, vec![5672, 15672]),
            svc("redis", "Redis", ServiceKind::Redis, vec![6379]),
        ];
        let status = |service: &Service, state: State, uptime: u64, pid: u32| ServiceStatus {
            id: service.id.clone(),
            name: service.name.clone(),
            kind: "x".into(),
            state,
            installed: true,
            path: format!("{}.exe", service.id),
            ports: service
                .ports
                .iter()
                .map(|&port| PortStatus { port, listening: state == State::Up, holder: None })
                .collect(),
            uptime_secs: (state == State::Up).then_some(uptime),
            pids: if state == State::Up { vec![pid] } else { Vec::new() },
        };
        let states = [
            (State::Up, 9_240, 4812),
            (State::Up, 9_238, 5120),
            (State::Up, 9_238, 5164),
            (State::Up, 9_237, 5208),
            (State::Up, 9_236, 6020),
            (State::Stopped, 0, 0),
            (State::Up, 9_200, 7344),
            (State::Stopped, 0, 0),
        ];
        let hosts = ["blog.test", "shop.test", "legacy.test", "spa.test"];
        let report = StatusReport {
            root: "C:\\devcrate".into(),
            root_source: "own location".into(),
            config: Some("C:\\devcrate\\devcrate.toml".into()),
            cli_php: Some("php-8.5".into()),
            cli_python: Some("python-3.13".into()),
            services: stack
                .services
                .iter()
                .zip(states)
                .map(|(service, (state, up, pid))| status(service, state, up, pid))
                .collect(),
            sites: hosts.iter().map(|h| h.to_string()).collect(),
            dev: Vec::new(),
            certs: hosts
                .iter()
                .map(|host| crate::cert::SiteCert {
                    host: host.to_string(),
                    file: "certs/_wildcard.test.pem".into(),
                    state: crate::cert::CertState::Ok,
                    days_left: Some(812),
                    expires: Some("2029-02-03".into()),
                    names: vec!["*.test".into()],
                })
                .collect(),
        };
        let site = |host: &str, root: &str, fastcgi: Option<u16>, proxy: Option<u16>| crate::site::Site {
            host: host.into(),
            root: Some(root.into()),
            fastcgi_port: fastcgi,
            proxy_port: proxy,
            tls: true,
        };

        let mut app = App::new(Arc::new(stack));
        app.on_status(report);
        app.sites = vec![
            site("blog.test", "projects/blog/public", Some(9085), None),
            site("shop.test", "projects/shop/public", Some(9082), None),
            site("legacy.test", "projects/legacy", Some(9074), None),
            site("spa.test", "projects/spa/dist", None, Some(5173)),
        ];
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("docs").join("img");
        std::fs::create_dir_all(&out).unwrap();

        app.service_row = 3;
        std::fs::write(out.join("dashboard-services.svg"), svg(&app, 140, 34)).unwrap();
        app.tab = app::Tab::Sites;
        app.site_row = 1;
        std::fs::write(out.join("dashboard-sites.svg"), svg(&app, 140, 34)).unwrap();
    }

    /// One frame as an SVG: a rectangle per run of background, a `<text>` per
    /// run of style, each stretched to exactly its cells so box-drawing lines
    /// up whatever monospace font the viewer has.
    fn svg(app: &App, width: u16, height: u16) -> String {
        use ratatui::buffer::Cell;
        use ratatui::style::{Color, Modifier};

        let t = theme::theme();
        let hex = |color: Color, fallback: Color| match (color, fallback) {
            (Color::Rgb(r, g, b), _) | (Color::Reset, Color::Rgb(r, g, b)) => format!("#{r:02x}{g:02x}{b:02x}"),
            (other, _) => panic!("the screenshot knows RGB only, not {other:?}"),
        };
        let style = |c: &Cell| {
            let (mut fg, mut bg) = (hex(c.fg, t.text), hex(c.bg, t.bg));
            if c.modifier.contains(Modifier::REVERSED) {
                std::mem::swap(&mut fg, &mut bg);
            }
            (fg, bg, c.modifier.contains(Modifier::BOLD))
        };
        let escape = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");

        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| view::render(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let (cw, lh) = (8.4_f32, 18.0_f32);
        let (mut rects, mut texts) = (String::new(), String::new());
        for y in 0..height {
            let cells: Vec<(String, (String, String, bool))> = (0..width)
                .map(|x| {
                    let cell = buffer.cell((x, y)).unwrap();
                    (cell.symbol().to_string(), style(cell))
                })
                .collect();
            let top = y as f32 * lh;
            let mut x = 0;
            while x < cells.len() {
                let (start, bg) = (x, cells[x].1 .1.clone());
                while x < cells.len() && cells[x].1 .1 == bg {
                    x += 1;
                }
                rects.push_str(&format!(
                    "<rect x=\"{:.1}\" y=\"{top:.1}\" width=\"{:.1}\" height=\"{lh:.1}\" fill=\"{bg}\"/>\n",
                    start as f32 * cw,
                    (x - start) as f32 * cw + 0.3,
                ));
            }
            // A word at a time: a run's spaces would be stretched along with it.
            let blank = |s: &str| s.trim().is_empty();
            let mut x = 0;
            while x < cells.len() {
                if blank(&cells[x].0) {
                    x += 1;
                    continue;
                }
                let (start, (fg, _, bold)) = (x, cells[x].1.clone());
                let mut text = String::new();
                while x < cells.len() && !blank(&cells[x].0) && cells[x].1 .0 == fg && cells[x].1 .2 == bold {
                    text.push_str(&cells[x].0);
                    x += 1;
                }
                texts.push_str(&format!(
                    "<text x=\"{:.1}\" y=\"{:.1}\" fill=\"{fg}\"{} textLength=\"{:.1}\" lengthAdjust=\"spacingAndGlyphs\">{}</text>\n",
                    start as f32 * cw,
                    top + 13.5,
                    if bold { " font-weight=\"bold\"" } else { "" },
                    (x - start) as f32 * cw,
                    escape(&text),
                ));
            }
        }
        let (w, h) = (width as f32 * cw, height as f32 * lh);
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.0}\" height=\"{h:.0}\" viewBox=\"0 0 {w:.1} {h:.1}\">\n\
             <rect width=\"100%\" height=\"100%\" fill=\"{}\"/>\n{rects}\
             <g font-family=\"Cascadia Mono, Consolas, DejaVu Sans Mono, Menlo, monospace\" font-size=\"14\" xml:space=\"preserve\">\n{texts}</g>\n</svg>\n",
            hex(t.bg, t.bg),
        )
    }
}
