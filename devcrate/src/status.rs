//! `devcrate status` -- what is installed, what is running, what holds a port.

use anyhow::Result;
use serde::Serialize;

use crate::config::{Service, Stack};
use crate::probe::{self, ProcessTable};

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
}

#[derive(Debug, Serialize)]
pub struct PortStatus {
    pub port: u16,
    pub listening: bool,
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
    let services = stack.services.iter().map(|s| inspect(s, &processes, stack)).collect();

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

fn inspect(service: &Service, processes: &ProcessTable, stack: &Stack) -> ServiceStatus {
    let installed = service.is_installed();
    let pids = processes.matching(&service.process_prefix, &service.exclude_names);
    let ports: Vec<PortStatus> = service
        .ports
        .iter()
        .map(|&port| PortStatus { port, listening: probe::is_listening(port) })
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

    let rows: Vec<[String; 5]> = report
        .services
        .iter()
        .map(|s| {
            [
                s.name.clone(),
                s.state.as_str().to_string(),
                format_ports(&s.ports),
                format_pids(&s.pids),
                s.path.clone(),
            ]
        })
        .collect();

    let headers = ["SERVICE", "STATE", "PORTS", "PIDS", "PATH"];
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }

    print_row(&headers.map(String::from), &widths);
    for row in &rows {
        print_row(row, &widths);
    }

    if report.services.iter().any(|s| s.ports.iter().any(|p| !p.listening)) {
        println!("\n  (port) = not answering");
    }

    println!();
    match report.sites.len() {
        0 => println!("  no vhosts configured"),
        n => println!("  {n} vhost{}: {}", if n == 1 { "" } else { "s" }, report.sites.join(", ")),
    }
}

fn print_row(cells: &[String; 5], widths: &[usize]) {
    let mut line = String::from("  ");
    for (i, cell) in cells.iter().enumerate() {
        if i + 1 == cells.len() {
            line.push_str(cell);
        } else {
            line.push_str(&format!("{:width$}  ", cell, width = widths[i]));
        }
    }
    println!("{}", line.trim_end());
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
            PortStatus { port: 80, listening: true },
            PortStatus { port: 443, listening: false },
        ];
        assert_eq!(format_ports(&ports), "80 (443)");
    }

    #[test]
    fn pid_lists_collapse_to_leader_plus_count() {
        assert_eq!(format_pids(&[]), "-");
        assert_eq!(format_pids(&[42]), "42");
        assert_eq!(format_pids(&[42, 43, 44]), "42 +2");
    }
}
