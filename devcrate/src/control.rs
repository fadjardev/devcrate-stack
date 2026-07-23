//! Bringing services up and down.
//!
//! This is `start.bat` and `stop.bat`, with the differences that come from
//! knowing which processes are ours. Where the stop script reaches for
//! `taskkill /F /IM php-cgi.exe`, which kills *every* `php-cgi.exe` on the
//! machine, this stops only the processes whose executable lives under this
//! stack root. Two stacks can sit side by side.
//!
//! Both orders are the scripts' own. Down: nginx first so no new request
//! reaches a backend that is about to disappear, then the PHP pools, then
//! RabbitMQ, then MariaDB last because it has the most to flush. Up: the
//! reverse, so nginx only starts once the backends it proxies to are answering.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};

use crate::config::{Service, ServiceKind, Stack};
use crate::probe::{self, ProcessTable};
use crate::{exit, php};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Keep a Ctrl-C in the terminal that ran `devcrate start` from taking the
/// stack down with it, and don't flash a console window per service.
#[cfg(windows)]
const BACKGROUND: u32 = 0x0000_0200 /* CREATE_NEW_PROCESS_GROUP */ | 0x0800_0000 /* CREATE_NO_WINDOW */;

/// No console at all, as opposed to a redirected one. See the RabbitMQ arm of
/// [`launch`].
#[cfg(windows)]
const DETACHED: u32 = 0x0000_0008 /* DETACHED_PROCESS */;

/// The order things may safely be brought down in.
const SHUTDOWN: [ServiceKind; 4] =
    [ServiceKind::Nginx, ServiceKind::Php, ServiceKind::RabbitMq, ServiceKind::MariaDb];

/// ...and up in.
const STARTUP: [ServiceKind; 4] =
    [ServiceKind::MariaDb, ServiceKind::Php, ServiceKind::RabbitMq, ServiceKind::Nginx];

/// How often to re-check whether a service has finished going away.
const POLL: Duration = Duration::from_millis(200);

/// How long each kind gets to shut itself down before it is terminated.
///
/// These are ceilings, not waits: the poll returns as soon as the processes are
/// gone, so a clean nginx quit costs a few hundred milliseconds rather than the
/// flat four-second `timeout` the batch script always pays.
fn grace(kind: ServiceKind) -> Duration {
    match kind {
        ServiceKind::Nginx => Duration::from_secs(10),
        // A FastCGI pool has no shutdown command; it is terminated outright,
        // exactly as stop.bat does. No point waiting for something to happen.
        ServiceKind::Php => Duration::ZERO,
        ServiceKind::RabbitMq => Duration::from_secs(30),
        ServiceKind::MariaDb => Duration::from_secs(30),
    }
}

#[derive(Debug)]
pub enum Stopped {
    /// Nothing of ours was running.
    NotRunning,
    /// It shut itself down when asked.
    Graceful,
    /// It had to be terminated. Carries how many processes went, and why the
    /// graceful attempt did not do the job.
    Terminated { count: usize, reason: Option<String> },
    /// Still there afterwards.
    Failed(String),
}

impl Stopped {
    pub fn is_failure(&self) -> bool {
        matches!(self, Stopped::Failed(_))
    }
}

/// One service's share of a `start` or `stop`, ready to be rendered by whoever
/// asked for it.
///
/// The action functions return these rather than printing, because the same
/// call has to serve a command line writing to stdout and a dashboard that
/// must not have anything written underneath it.
#[derive(Debug)]
pub struct Step<T> {
    pub id: String,
    pub name: String,
    pub outcome: T,
    /// Helper processes cleared afterwards -- `epmd`, in practice.
    pub helpers: usize,
}

/// Stop the whole stack, or the one service named, and report what happened to
/// each. Ordering is the shutdown order; the caller sees them in the order they
/// were acted on.
pub fn run_stop(stack: &Stack, only: Option<&str>) -> Result<Vec<Step<Stopped>>> {
    let targets = select(stack, only, SHUTDOWN)?;
    Ok(targets
        .iter()
        .map(|service| {
            let outcome = stop_service(stack, service);
            // Only once the service itself is down: epmd is the thing RabbitMQ
            // registers with, so clearing it first would be pulling the rug out.
            let helpers = if outcome.is_failure() { 0 } else { reap_helpers(service) };
            Step { id: service.id.clone(), name: service.name.clone(), outcome, helpers }
        })
        .collect())
}

/// Stop the whole stack, or the one service named.
pub fn stop(stack: &Stack, only: Option<&str>) -> Result<u8> {
    let targets = select(stack, only, SHUTDOWN)?;
    println!("Stopping {} in {}", subject(&targets, only), stack.root.display());
    println!();

    let width = targets.iter().map(|s| s.name.chars().count()).max().unwrap_or(0);
    let steps = run_stop(stack, only)?;
    let failed = steps.iter().filter(|step| step.outcome.is_failure()).count();

    for step in &steps {
        println!("  {:<width$}  {}", step.name, describe(&step.outcome, step.helpers));
    }

    println!();
    if failed == 0 {
        println!("Stopped.");
        Ok(exit::OK)
    } else {
        println!("{failed} service(s) could not be stopped.");
        Ok(exit::ERROR)
    }
}

fn subject(targets: &[&Service], only: Option<&str>) -> String {
    if only.is_none() {
        return "the Devcrate stack".to_string();
    }
    targets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
}

/// The services to act on, in the given order.
///
/// `stack.services` is in *display* order (nginx, PHP, MariaDB, RabbitMQ),
/// which is neither the order things may be safely started nor the order they
/// may be stopped.
fn select<'a>(
    stack: &'a Stack,
    only: Option<&str>,
    order: [ServiceKind; 4],
) -> Result<Vec<&'a Service>> {
    let ordered: Vec<&Service> = order
        .into_iter()
        .flat_map(|kind| stack.services.iter().filter(move |s| s.kind == kind))
        .collect();

    let Some(want) = only else {
        return Ok(ordered);
    };

    let want = want.to_ascii_lowercase();
    // `php` on its own means every version, which is what someone typing it
    // after seeing three php* rows in `devcrate status` almost certainly means.
    if want == "php" {
        return Ok(ordered.into_iter().filter(|s| s.kind == ServiceKind::Php).collect());
    }

    let matched: Vec<&Service> = ordered.iter().copied().filter(|s| s.id == want).collect();
    if !matched.is_empty() {
        return Ok(matched);
    }

    // A PHP version may be named by its digits in any spelling, the way
    // `devcrate php use` accepts them -- so `stop php85`, `stop php-8.5`, and
    // `stop 8.5` are one command, and renaming the directories does not
    // invalidate anybody's script.
    let wanted_digits = php::digits(&want);
    if !wanted_digits.is_empty() {
        let by_version: Vec<&Service> = ordered
            .iter()
            .copied()
            .filter(|s| s.kind == ServiceKind::Php && php::digits(&s.id) == wanted_digits)
            .collect();
        if !by_version.is_empty() {
            return Ok(by_version);
        }
    }

    let known: Vec<&str> = ordered.iter().map(|s| s.id.as_str()).collect();
    Err(anyhow!("unknown service {want:?}; known: {}, php", known.join(", ")))
}

fn stop_service(stack: &Stack, service: &Service) -> Stopped {
    if running(service).is_empty() {
        return Stopped::NotRunning;
    }

    let reason = ask_nicely(stack, service).err().map(|err| format!("{err:#}"));

    if wait_until_gone(service, grace(service.kind)) {
        return Stopped::Graceful;
    }

    let count = terminate(service);
    // Termination is not instant: the handles have to close before the process
    // leaves the table.
    if wait_until_gone(service, Duration::from_secs(5)) {
        Stopped::Terminated { count, reason }
    } else {
        Stopped::Failed(format!("{} process(es) still running after terminate", running(service).len()))
    }
}

fn running(service: &Service) -> Vec<u32> {
    ProcessTable::scan().matching(&service.process_prefix, &service.exclude_names)
}

/// Who is on a port we wanted. Worth a full process scan: this only runs on
/// the way to reporting a failure, and "port 3306 is taken" without a name
/// leaves the reader nowhere to go.
fn port_holder(port: u16) -> Option<String> {
    let pid = probe::listeners().get(&port).copied()?;
    let table = ProcessTable::scan();
    Some(match table.name_of(pid) {
        Some(name) => format!("{name} (pid {pid})"),
        None => format!("pid {pid}"),
    })
}

fn wait_until_gone(service: &Service, grace: Duration) -> bool {
    let deadline = Instant::now() + grace;
    loop {
        if running(service).is_empty() {
            return true;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        sleep(POLL.min(left));
    }
}

/// Terminate what is left, supervisors first so a pool manager cannot respawn
/// the workers being killed underneath it.
///
/// Returns how many processes were there to remove, not how many `kill` calls
/// succeeded: PHP puts its FastCGI pool in a job object, so terminating the
/// master takes the four workers with it and the follow-up kills report failure
/// against processes that have already gone. Counting attempts would say
/// "terminated 1" where five processes ended.
fn terminate(service: &Service) -> usize {
    let table = ProcessTable::scan();
    let pids = table.matching(&service.process_prefix, &service.exclude_names);
    for pid in &pids {
        table.kill(*pid);
    }
    pids.len()
}

/// Clear the excluded helpers once the service they serve is down -- `epmd`,
/// which `stop.bat` also kills separately. Path matching keeps this to the port
/// mapper inside this stack root; an Erlang install elsewhere is untouched.
fn reap_helpers(service: &Service) -> usize {
    if service.exclude_names.is_empty() {
        return 0;
    }
    let table = ProcessTable::scan();
    let pids = table.helpers(&service.process_prefix, &service.exclude_names);
    pids.iter().filter(|&&pid| table.kill(pid)).count()
}

/// The service's own shutdown command, where it has one.
fn ask_nicely(stack: &Stack, service: &Service) -> Result<()> {
    match service.kind {
        ServiceKind::Nginx => run(Command::new(&stack.nginx_bin)
            .arg("-p")
            .arg(&stack.nginx_prefix)
            .args(["-s", "quit"])),
        // No shutdown command exists for a php-cgi FastCGI listener.
        ServiceKind::Php => Ok(()),
        ServiceKind::RabbitMq => {
            let sbin = parent(&service.install_marker)?;
            let ctl = sbin.join("rabbitmqctl.bat");
            if !ctl.is_file() {
                return Err(anyhow!("{} not found", stack.rel(&ctl)));
            }
            let rabbit_dir = ancestor(&service.install_marker, 2)?;
            let erlang_bin = service.process_prefix.join("bin");
            let path = match std::env::var("PATH") {
                Ok(existing) => format!("{};{}", erlang_bin.display(), existing),
                Err(_) => erlang_bin.display().to_string(),
            };
            // The same three variables start.bat and stop.bat export; without
            // them rabbitmqctl talks to the wrong node, or to none.
            run(Command::new("cmd")
                .args(["/c", "call"])
                .arg(&ctl)
                .arg("stop")
                .env("ERLANG_HOME", &service.process_prefix)
                .env("RABBITMQ_BASE", rabbit_dir.join("data"))
                .env("PATH", path))
        }
        ServiceKind::MariaDb => {
            let bin = parent(&service.install_marker)?;
            let admin = bin.join("mariadb-admin.exe");
            if !admin.is_file() {
                return Err(anyhow!("{} not found", stack.rel(&admin)));
            }
            let mut command = Command::new(&admin);
            let defaults = ancestor(&service.install_marker, 2)?.join("my.ini");
            if defaults.is_file() {
                command.arg(format!("--defaults-file={}", defaults.display()));
            }
            run(command.args(["-u", "root", "shutdown"]))
        }
    }
}

/// Run a shutdown command and turn a non-zero exit into an error carrying
/// whatever the tool said, so a failed `rabbitmqctl stop` is visible rather
/// than silently escalating to a kill.
fn run(command: &mut Command) -> Result<()> {
    let Output { status, stdout, stderr } = command.output()?;
    if status.success() {
        return Ok(());
    }
    let message = [stderr, stdout]
        .iter()
        .map(|stream| String::from_utf8_lossy(stream).trim().to_string())
        .find(|text| !text.is_empty())
        .unwrap_or_else(|| format!("exit code {}", status.code().unwrap_or(-1)));
    Err(anyhow!("{}", message.lines().next().unwrap_or(&message).to_string()))
}

// ---------------------------------------------------------------------------
// Starting
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum Started {
    /// Ours was already up; nothing to do.
    AlreadyRunning,
    /// Missing from this stack root. `start.bat` prints SKIPPED and carries on,
    /// and so does this: a stack without PHP 8.5 is not a broken stack.
    NotInstalled(String),
    /// Up and answering, in this long.
    Listening(Duration),
    /// Process is alive but no port answered before the deadline.
    Silent(Duration),
    /// The process went away again.
    Exited,
    /// Somebody else is on the port, so it was not started at all. `holder`
    /// names them where the kernel's table will say.
    PortBusy { port: u16, holder: Option<String> },
    Failed(String),
}

impl Started {
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            Started::Silent(_) | Started::Exited | Started::PortBusy { .. } | Started::Failed(_)
        )
    }
}

/// How long each kind gets to answer on its port before it counts as failed.
/// Ceilings again -- the poll returns as soon as the port answers.
fn boot(kind: ServiceKind) -> Duration {
    match kind {
        ServiceKind::Nginx => Duration::from_secs(15),
        ServiceKind::Php => Duration::from_secs(15),
        ServiceKind::MariaDb => Duration::from_secs(45),
        // A cold Erlang node with the management plugin is the slow one here.
        ServiceKind::RabbitMq => Duration::from_secs(90),
    }
}

/// Stop the handles this process was *given* from reaching the services it
/// launches.
///
/// Rust spawns with `bInheritHandles = TRUE`, so every inheritable handle in
/// the process is handed to the child whatever the child's own stdio is set to
/// -- and when `devcrate start` is piped or redirected, the write end of that
/// pipe is one of them. A detached MariaDB then holds `devcrate start > log`
/// open for as long as it runs, and the caller never sees the command return.
///
/// `start.bat` has the same bug for the same reason: `start /B` inherits too.
/// Clearing the inherit flag on our own three standard handles is enough, and
/// costs nothing -- no service here is meant to write to our console anyway.
#[cfg(windows)]
fn seal_stdio() {
    use std::io::{stderr, stdin, stdout};
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};

    let handles =
        [stdin().as_raw_handle(), stdout().as_raw_handle(), stderr().as_raw_handle()];
    for handle in handles {
        // Safety: the handles belong to this process and stay open for its
        // lifetime; clearing an inheritance flag cannot invalidate them.
        unsafe {
            SetHandleInformation(handle as _, HANDLE_FLAG_INHERIT, 0);
        }
    }
}

#[cfg(not(windows))]
fn seal_stdio() {}

/// Start the whole stack, or the one service named, and report what happened to
/// each. In startup order.
pub fn run_start(stack: &Stack, only: Option<&str>) -> Result<Vec<Step<Started>>> {
    let targets = select(stack, only, STARTUP)?;
    seal_stdio();
    Ok(targets
        .iter()
        .map(|service| Step {
            id: service.id.clone(),
            name: service.name.clone(),
            outcome: start_service(stack, service),
            helpers: 0,
        })
        .collect())
}

/// Start the whole stack, or the one service named.
pub fn start(stack: &Stack, only: Option<&str>) -> Result<u8> {
    let targets = select(stack, only, STARTUP)?;
    let kinds: Vec<ServiceKind> = targets.iter().map(|s| s.kind).collect();
    println!("Starting {} in {}", subject(&targets, only), stack.root.display());
    println!();

    let width = targets.iter().map(|s| s.name.chars().count()).max().unwrap_or(0);
    let steps = run_start(stack, only)?;
    let failed = steps.iter().filter(|step| step.outcome.is_failure()).count();

    for (step, kind) in steps.iter().zip(kinds) {
        println!("  {:<width$}  {}", step.name, describe_start(kind, &step.outcome));
    }

    println!();
    if failed == 0 {
        println!("Started.");
        Ok(exit::OK)
    } else {
        println!("{failed} service(s) did not start. `devcrate status` has the detail.");
        Ok(exit::ERROR)
    }
}

fn start_service(stack: &Stack, service: &Service) -> Started {
    if !service.is_installed() {
        return Started::NotInstalled(stack.rel(&service.install_marker));
    }
    if !running(service).is_empty() {
        return Started::AlreadyRunning;
    }
    // Preflight: ours is not running, so anything already on one of its ports
    // belongs to somebody else and starting would just fail to bind.
    if let Some(&port) = service.ports.iter().find(|&&port| probe::is_listening(port)) {
        return Started::PortBusy { port, holder: port_holder(port) };
    }

    if let Err(err) = launch(stack, service) {
        return Started::Failed(format!("{err:#}"));
    }
    wait_for_port(service, boot(service.kind))
}

fn wait_for_port(service: &Service, grace: Duration) -> Started {
    let started = Instant::now();
    let mut seen = false;

    loop {
        let alive = !running(service).is_empty();
        seen |= alive;

        if alive && service.ports.iter().any(|&port| probe::is_listening(port)) {
            return Started::Listening(started.elapsed());
        }
        // Nothing to probe: being alive is the whole of the news.
        if alive && service.ports.is_empty() {
            return Started::Listening(started.elapsed());
        }
        // Gone after having been there, or never there at all.
        if seen && !alive {
            return Started::Exited;
        }
        if started.elapsed() >= grace {
            return if seen { Started::Silent(grace) } else { Started::Exited };
        }
        sleep(POLL);
    }
}

fn launch(stack: &Stack, service: &Service) -> Result<()> {
    match service.kind {
        ServiceKind::MariaDb => {
            let mut command = Command::new(&service.install_marker);
            let defaults = ancestor(&service.install_marker, 2)?.join("my.ini");
            if defaults.is_file() {
                command.arg(format!("--defaults-file={}", defaults.display()));
            }
            background(&mut command)
        }

        ServiceKind::Php => {
            let dir = parent(&service.install_marker)?;
            let &port = service
                .ports
                .first()
                .ok_or_else(|| anyhow!("no FastCGI port configured for {}", service.id))?;
            let mut command = Command::new(&service.install_marker);
            command
                .arg("-b")
                .arg(format!("127.0.0.1:{port}"))
                // php.ini for 7.4 and 8.2 uses a relative extension_dir and
                // error_log, which resolve against the working directory. This
                // is what `start /D` is doing in start.bat.
                .current_dir(&dir)
                .env("PHP_FCGI_CHILDREN", "4")
                .env("PHP_FCGI_MAX_REQUESTS", "500");
            background(&mut command)
        }

        ServiceKind::RabbitMq => {
            let server = parent(&service.install_marker)?.join("rabbitmq-server.bat");
            if !server.is_file() {
                return Err(anyhow!("{} not found", stack.rel(&server)));
            }
            let rabbit_dir = ancestor(&service.install_marker, 2)?;
            let mut command = Command::new("cmd");
            command
                .args(["/c", "call"])
                .arg(&server)
                .arg("-detached")
                .env("ERLANG_HOME", &service.process_prefix)
                .env("RABBITMQ_BASE", rabbit_dir.join("data"))
                .env("PATH", with_erlang_bin(&service.process_prefix));
            // Detached rather than redirected. OTP's terminal driver aborts
            // with "nouser" when handed a console that is not a tty, which is
            // what a pipe or a file redirect looks like -- that is the warning
            // in start.bat. DETACHED_PROCESS gives it no console instead of a
            // fake one, which `-detached` is designed for.
            //
            // It also fixes a hang start.bat has: with our handles inherited,
            // the detached Erlang node keeps the parent's stdout open for its
            // whole lifetime, so `devcrate start | tee` or any redirect never
            // returns until the broker stops. The broker's own log is under
            // RABBITMQ_BASE\log either way.
            command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            #[cfg(windows)]
            command.creation_flags(DETACHED);
            let status = command.status()?;
            if !status.success() {
                return Err(anyhow!("rabbitmq-server exited {}", status.code().unwrap_or(-1)));
            }
            Ok(())
        }

        ServiceKind::Nginx => {
            ensure_projects_junction(stack)?;
            let mut command = Command::new(&stack.nginx_bin);
            // -p is the prefix, which is *not* where the binary lives once
            // versions sit inside it; -c is resolved against the prefix, so
            // conf/nginx.conf is the stack's config whichever version runs.
            command.arg("-p").arg(&stack.nginx_prefix).args(["-c", "conf/nginx.conf"]);
            background(&mut command)
        }
    }
}

/// Spawn and walk away. The child keeps running after `devcrate` exits; the
/// handle is dropped without being waited on, which on Windows detaches it
/// rather than orphaning it.
fn background(command: &mut Command) -> Result<()> {
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(BACKGROUND);
    command.spawn()?;
    Ok(())
}

fn with_erlang_bin(erlang_dir: &Path) -> String {
    let bin = erlang_dir.join("bin");
    match std::env::var("PATH") {
        Ok(existing) => format!("{};{}", bin.display(), existing),
        Err(_) => bin.display().to_string(),
    }
}

/// Recreate `<nginx>\projects -> <root>\projects` if it is missing.
///
/// The vhost confs say `root projects/<domain>/public`, resolved against the
/// nginx prefix. It has to be a junction rather than a relative path because
/// PHP-CGI on Windows rejects any `SCRIPT_FILENAME` containing `..` with "No
/// input file specified" -- `$realpath_root` is a no-op on win32, so document
/// roots must stay dot-free. `start.bat` and `new-vhost.bat` self-heal it the
/// same way.
pub fn ensure_projects_junction(stack: &Stack) -> Result<()> {
    let projects = stack.root.join("projects");
    if !projects.is_dir() {
        std::fs::create_dir_all(&projects)?;
    }
    let link = stack.nginx_prefix.join("projects");
    if link.exists() {
        return Ok(());
    }
    crate::junction::create(&link, &projects)
}

/// Ask a running nginx to re-read its configuration. `Ok(false)` means nginx
/// was not running, which is not an error: the new conf is picked up whenever
/// it next starts.
///
/// The configuration is tested first. nginx loads every conf in `sites\` as one
/// document, so reloading with a broken one in there does not fail politely for
/// that site -- it fails the reload, and every other vhost keeps serving the old
/// config with no indication why the new one never appeared. Testing first turns
/// that into an error message.
pub fn reload_nginx(stack: &Stack) -> Result<bool> {
    let Some(nginx) = stack.by_kind(ServiceKind::Nginx) else {
        return Ok(false);
    };
    if running(nginx).is_empty() {
        return Ok(false);
    }

    let test =
        Command::new(&stack.nginx_bin).arg("-p").arg(&stack.nginx_prefix).arg("-t").output()?;
    if !test.status.success() {
        // nginx -t writes its verdict, including the offending file and line,
        // to stderr.
        let detail = String::from_utf8_lossy(&test.stderr);
        let detail = detail.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>();
        return Err(anyhow!("nginx configuration test failed: {}", detail.join("; ")));
    }

    run(Command::new(&stack.nginx_bin)
        .arg("-p")
        .arg(&stack.nginx_prefix)
        .args(["-s", "reload"]))?;
    Ok(true)
}

pub fn describe_start(service_kind: ServiceKind, outcome: &Started) -> String {
    match outcome {
        Started::AlreadyRunning => "already running".into(),
        Started::NotInstalled(path) => format!("skipped, not installed ({path})"),
        Started::Listening(took) => format!("listening in {:.1}s", took.as_secs_f32()),
        Started::Silent(grace) => {
            format!("FAILED: running, but no port answered within {}s", grace.as_secs())
        }
        Started::Exited if service_kind == ServiceKind::Php => {
            "FAILED: exited immediately (usually the missing Visual C++ Redistributable \
             -- see docs/troubleshooting.md)"
                .into()
        }
        Started::Exited => "FAILED: exited immediately".into(),
        Started::PortBusy { port, holder: Some(who) } => {
            format!("FAILED: port {port} is held by {who}; not started")
        }
        Started::PortBusy { port, holder: None } => {
            format!("FAILED: port {port} is held by another process; not started")
        }
        Started::Failed(why) => format!("FAILED: {why}"),
    }
}

// ---------------------------------------------------------------------------

/// Stop, then start. Nothing is started if the stop did not fully succeed --
/// launching a second MariaDB onto a datadir the first one still holds is worse
/// than reporting the failure.
pub fn restart(stack: &Stack, only: Option<&str>) -> Result<u8> {
    let code = stop(stack, only)?;
    if code != exit::OK {
        return Ok(code);
    }
    println!();
    start(stack, only)
}

fn parent(path: &Path) -> Result<PathBuf> {
    path.parent().map(Path::to_path_buf).ok_or_else(|| anyhow!("{} has no parent", path.display()))
}

fn ancestor(path: &Path, levels: usize) -> Result<PathBuf> {
    path.ancestors()
        .nth(levels)
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("{} has no ancestor {levels} levels up", path.display()))
}

pub fn describe(outcome: &Stopped, helpers: usize) -> String {
    let main = match outcome {
        Stopped::NotRunning => "not running".to_string(),
        Stopped::Graceful => "stopped".to_string(),
        Stopped::Terminated { count, reason: None } => {
            format!("terminated ({count} process(es))")
        }
        Stopped::Terminated { count, reason: Some(why) } => {
            format!("terminated ({count} process(es)) after: {why}")
        }
        Stopped::Failed(why) => format!("FAILED: {why}"),
    };
    match helpers {
        0 => main,
        n => format!("{main}, {n} helper process(es) cleared"),
    }
}
