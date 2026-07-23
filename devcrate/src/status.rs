//! `devcrate status` -- what is installed, what is running, what holds a port.

use std::collections::BTreeMap;

use anyhow::Result;
use serde::Serialize;

use crate::config::{Service, Stack};
use crate::probe::{self, ProcessTable};
use crate::term::{self, Color};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    /// Our process is running and at least one of its ports answers.
    Up,
    /// Our process is running but nothing answers yet.
    Starting,
    /// A port answers, but no process of ours is behind it -- something else
    /// holds it, and starting the stack will fail on that port.
    PortBusy,
    /// Installed, not running.
    Stopped,
    /// Not installed in this stack root.
    Absent,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Up => "up",
            State::Starting => "starting",
            State::PortBusy => "port busy",
            State::Stopped => "stopped",
            State::Absent => "absent",
        }
    }

    /// Colour as a second channel for the same word, never the only one: the
    /// table has to read identically with colour off.
    fn color(self) -> Color {
        match self {
            State::Up => Color::Green,
            State::Starting => Color::Yellow,
            State::PortBusy => Color::Red,
            State::Stopped | State::Absent => Color::Dim,
        }
    }
}

/// Whoever holds a port that is not ours.
#[derive(Debug, Serialize)]
pub struct PortHolder {
    pub pid: u32,
    /// Image name, when the process is still there to ask.
    pub name: Option<String>,
}

impl std::fmt::Display for PortHolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.name {
            Some(name) => write!(f, "{name} (pid {})", self.pid),
            None => write!(f, "pid {}", self.pid),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PortStatus {
    pub port: u16,
    pub listening: bool,
    /// Set only when something *other* than this service is on the port --
    /// the difference between "up" and "start will fail here".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub holder: Option<PortHolder>,
}

#[derive(Debug, Serialize)]
pub struct ServiceStatus {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub state: State,
    pub installed: bool,
    pub path: String,
    pub ports: Vec<PortStatus>,
    pub pids: Vec<u32>,
    /// How long the leading process has been up, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_secs: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct StatusReport {
    pub root: String,
    pub root_source: String,
    pub config: Option<String>,
    pub cli_php: Option<String>,
    pub services: Vec<ServiceStatus>,
    pub sites: Vec<String>,
}

pub fn report(stack: &Stack) -> StatusReport {
    let processes = ProcessTable::scan();
    // One kernel table for the whole report rather than one lookup per port.
    let listeners = probe::listeners();
    let services =
        stack.services.iter().map(|s| inspect(s, &processes, &listeners, stack)).collect();

    StatusReport {
        root: stack.root.display().to_string(),
        root_source: stack.root_source.label().to_string(),
        config: stack.config_path.as_ref().map(|p| p.display().to_string()),
        cli_php: stack.current_php().as_deref().map(|p| {
            p.file_name().unwrap_or(p.as_os_str()).to_string_lossy().into_owned()
        }),
        services,
        sites: stack
            .sites()
            .iter()
            .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .collect(),
    }
}

fn inspect(
    service: &Service,
    processes: &ProcessTable,
    listeners: &BTreeMap<u16, u32>,
    stack: &Stack,
) -> ServiceStatus {
    let installed = service.is_installed();
    let pids = processes.matching(&service.process_prefix, &service.exclude_names);
    let ports: Vec<PortStatus> = service
        .ports
        .iter()
        .map(|&port| {
            let listening = probe::is_listening(port);
            // A port held by one of our own processes is not "held by
            // somebody"; it is the service working.
            let holder = listeners
                .get(&port)
                .filter(|_| listening)
                .filter(|pid| !pids.contains(pid))
                .map(|&pid| PortHolder {
                    pid,
                    name: processes.name_of(pid).map(str::to_string),
                });
            PortStatus { port, listening, holder }
        })
        .collect();
    let any_listening = ports.iter().any(|p| p.listening);

    let state = match (pids.is_empty(), any_listening, installed) {
        (false, true, _) => State::Up,
        (false, false, _) => State::Starting,
        (true, true, _) => State::PortBusy,
        (true, false, true) => State::Stopped,
        (true, false, false) => State::Absent,
    };

    ServiceStatus {
        id: service.id.clone(),
        name: service.name.clone(),
        kind: service.kind.as_str().to_string(),
        state,
        installed,
        path: stack.rel(&service.install_marker),
        // The leader's age is the service's age: its workers may have been
        // recycled under it (PHP_FCGI_MAX_REQUESTS does exactly that).
        uptime_secs: pids.first().and_then(|&pid| processes.uptime(pid)).map(|d| d.as_secs()),
        ports,
        pids,
    }
}

pub fn print_text(report: &StatusReport) {
    println!("Devcrate  {}", report.root);
    println!("  root from  {}", report.root_source);
    match &report.config {
        Some(path) => println!("  config     {path}"),
        None => println!("  config     built-in defaults (no devcrate.toml)"),
    }
    match &report.cli_php {
        Some(version) => println!("  CLI PHP    {version}  (via php\\current)"),
        None => println!("  CLI PHP    not set  (run: phpuse 85)"),
    }
    println!();

    let rows: Vec<[String; 6]> = report
        .services
        .iter()
        .map(|s| {
            [
                s.name.clone(),
                s.state.as_str().to_string(),
                format_ports(&s.ports),
                format_uptime(s.uptime_secs),
                format_pids(&s.pids),
                s.path.clone(),
            ]
        })
        .collect();

    let headers = ["SERVICE", "STATE", "PORTS", "UPTIME", "PIDS", "PATH"];
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }

    print_row(&headers.map(String::from), &widths, None);
    for (row, service) in rows.iter().zip(&report.services) {
        print_row(row, &widths, Some(service.state.color()));
    }

    if report.services.iter().any(|s| s.ports.iter().any(|p| !p.listening)) {
        println!("\n  (port) = not answering");
    }

    // Naming the holder is the difference between "start will fail" and
    // knowing what to close.
    let taken: Vec<String> = report
        .services
        .iter()
        .flat_map(|s| s.ports.iter())
        .filter_map(|p| p.holder.as_ref().map(|h| format!("port {} is held by {h}", p.port)))
        .collect();
    if !taken.is_empty() {
        println!();
        for line in taken {
            println!("  {}", term::paint(&line, Color::Red));
        }
    }

    println!();
    match report.sites.len() {
        0 => println!("  no vhosts configured"),
        n => {
            let list = report.sites.join(", ");
            let heading = format!("{n} vhost{}: {list}", if n == 1 { "" } else { "s" });
            for line in term::wrap(&heading, 2) {
                println!("{line}");
            }
        }
    }
}

/// Print one table row, padding on the plain text and colouring afterwards so
/// the escape sequences never count towards a column width.
fn print_row(cells: &[String; 6], widths: &[usize], state: Option<Color>) {
    let mut line = String::from("  ");
    for (i, cell) in cells.iter().enumerate() {
        let last = i + 1 == cells.len();
        let padded = if last {
            elide(cell, line.chars().count())
        } else {
            format!("{:width$}  ", cell, width = widths[i])
        };
        match state {
            // Column 1 is STATE.
            Some(color) if i == 1 => line.push_str(&term::paint(&padded, color)),
            _ => line.push_str(&padded),
        }
    }
    println!("{}", line.trim_end());
}

/// Keep the last column inside the terminal, from the left: the tail of a path
/// identifies it, the head is the part every row has in common.
fn elide(text: &str, used: usize) -> String {
    let Some(room) = term::width().map(|w| w.saturating_sub(used)) else {
        return text.to_string();
    };
    let length = text.chars().count();
    if length <= room || room < 4 {
        return text.to_string();
    }
    let keep = room - 1;
    format!("\u{2026}{}", text.chars().skip(length - keep).collect::<String>())
}

fn format_ports(ports: &[PortStatus]) -> String {
    if ports.is_empty() {
        return "-".into();
    }
    ports
        .iter()
        .map(|p| if p.listening { p.port.to_string() } else { format!("({})", p.port) })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Two units, largest first: enough to tell "just restarted" from "up since
/// Monday" at a glance, without a column that jitters every second.
fn format_uptime(secs: Option<u64>) -> String {
    let Some(secs) = secs else {
        return "-".into();
    };
    let (days, hours, minutes) = (secs / 86_400, (secs % 86_400) / 3600, (secs % 3600) / 60);
    match (days, hours, minutes) {
        (0, 0, 0) => format!("{secs}s"),
        (0, 0, m) => format!("{m}m {:02}s", secs % 60),
        (0, h, m) => format!("{h}h {m:02}m"),
        (d, h, _) => format!("{d}d {h:02}h"),
    }
}

fn format_pids(pids: &[u32]) -> String {
    match pids.split_first() {
        None => "-".into(),
        Some((first, [])) => first.to_string(),
        Some((first, rest)) => format!("{first} +{}", rest.len()),
    }
}

pub fn print_json(report: &StatusReport) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(report)?);
    Ok(())
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_that_do_not_answer_are_parenthesised() {
        let ports = vec![
            PortStatus { port: 80, listening: true, holder: None },
            PortStatus { port: 443, listening: false, holder: None },
        ];
        assert_eq!(format_ports(&ports), "80 (443)");
    }

    #[test]
    fn pid_lists_collapse_to_leader_plus_count() {
        assert_eq!(format_pids(&[]), "-");
        assert_eq!(format_pids(&[42]), "42");
        assert_eq!(format_pids(&[42, 43, 44]), "42 +2");
    }

    #[test]
    fn uptime_reads_in_two_units() {
        assert_eq!(format_uptime(None), "-");
        assert_eq!(format_uptime(Some(9)), "9s");
        assert_eq!(format_uptime(Some(125)), "2m 05s");
        assert_eq!(format_uptime(Some(3600 * 3 + 240)), "3h 04m");
        assert_eq!(format_uptime(Some(86_400 * 2 + 3600 * 5)), "2d 05h");
    }

    #[test]
    fn a_named_holder_reads_better_than_a_bare_pid() {
        let named = PortHolder { pid: 1234, name: Some("Skype.exe".into()) };
        assert_eq!(named.to_string(), "Skype.exe (pid 1234)");
        let anonymous = PortHolder { pid: 1234, name: None };
        assert_eq!(anonymous.to_string(), "pid 1234");
    }
}
