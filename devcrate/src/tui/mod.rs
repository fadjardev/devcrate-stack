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
use crate::tui::app::App;
use crate::tui::worker::Event;

/// How long to wait for an event before redrawing anyway. Nothing depends on
/// it, but a clock that never ticks makes "working..." look frozen.
const IDLE: Duration = Duration::from_millis(500);

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
    let tailer = worker::spawn_tailer(tx);

    let mut app = App::new(stack);

    loop {
        terminal.draw(|frame| view::render(frame, &app)).context("drawing the dashboard")?;

        // Block for the first event, then take whatever else has piled up
        // behind it: a scan and three keystrokes should cost one redraw.
        let mut events = match rx.recv_timeout(IDLE) {
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
            }
        }

        // The app asks for these by raising a flag rather than holding the
        // channels itself, so it stays a pure state machine and can be tested
        // without any threads at all.
        if app.rescan_wanted {
            app.rescan_wanted = false;
            let _ = nudge.send(());
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

    /// Deleting a vhost asks first.
    #[test]
    fn destructive_site_actions_are_confirmed() {
        let mut app = App::new(stack());
        app.tab = app::Tab::Sites;
        app.sites = vec![crate::site::Site {
            host: "myapp.test".into(),
            root: Some("projects/myapp.test/public".into()),
            fastcgi_port: Some(9085),
        }];

        assert!(app.on_key(KeyEvent::from(KeyCode::Char('d'))).is_none(), "no job yet");
        let asking = draw(&app);
        assert!(asking.contains("Confirm"), "{asking}");
        assert!(asking.contains("left alone"), "it says what is kept: {asking}");

        let job = app.on_key(KeyEvent::from(KeyCode::Char('y'))).expect("y confirms");
        assert!(matches!(job, worker::Job::SiteRemove(host) if host == "myapp.test"));
    }
}
