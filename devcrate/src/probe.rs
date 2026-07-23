//! Inspection: which ports answer, and which of our processes run.
//!
//! The only thing here that acts rather than observes is [`ProcessTable::kill`],
//! and it can only reach a process the caller has already matched by path.

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, UpdateKind};

/// Long enough for a loopback accept, short enough that probing a dozen closed
/// ports stays instant.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(250);

/// Is something accepting connections on `127.0.0.1:port`?
///
/// This opens and immediately drops a real TCP connection, which is the only
/// check that needs no privileges. A server that logs aborted connections
/// (MariaDB does) will note it.
pub fn is_listening(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).is_ok()
}

#[derive(Debug, Clone)]
pub struct ProcessEntry {
    pub pid: u32,
    pub parent: Option<u32>,
    pub name: String,
    pub exe: PathBuf,
}

/// A snapshot of the running processes that have a resolvable executable path.
///
/// The snapshot owns the `sysinfo` handle it was taken from, so a PID read out
/// of it can be signalled without a second scan -- and without the PID having
/// been recycled in between.
#[derive(Debug)]
pub struct ProcessTable {
    system: System,
    entries: Vec<ProcessEntry>,
}

impl ProcessTable {
    pub fn scan() -> ProcessTable {
        let mut system = System::new_with_specifics(RefreshKind::nothing().with_processes(
            ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
        ));
        system.refresh_processes(ProcessesToUpdate::All, true);

        let mut entries: Vec<ProcessEntry> = system
            .processes()
            .values()
            .filter_map(|process| {
                let exe = process.exe()?.to_path_buf();
                Some(ProcessEntry {
                    pid: process.pid().as_u32(),
                    parent: process.parent().map(|p| p.as_u32()),
                    name: process.name().to_string_lossy().into_owned(),
                    exe,
                })
            })
            .collect();
        entries.sort_by_key(|e| e.pid);
        ProcessTable { system, entries }
    }

    /// Terminate a process this table matched. `false` means it was already
    /// gone, or the caller lacks the rights to signal it.
    ///
    /// There is no polite signal to send here: Windows has no SIGTERM, and the
    /// services that *do* have a graceful shutdown (nginx, MariaDB, RabbitMQ)
    /// have it as a command of their own. This is the fallback for what is left
    /// afterwards, which is what `stop.bat` reaches for as well.
    pub fn kill(&self, pid: u32) -> bool {
        self.system.process(Pid::from_u32(pid)).is_some_and(|process| process.kill())
    }

    /// PIDs whose executable is `prefix`, or lives under it, supervisors first.
    ///
    /// Matching on the path is deliberate: it means a system-wide nginx or
    /// MariaDB is never mistaken for the one in this stack root.
    pub fn matching(&self, prefix: &Path, exclude_names: &[String]) -> Vec<u32> {
        let matched: Vec<&ProcessEntry> = self
            .entries
            .iter()
            .filter(|entry| {
                entry.exe.starts_with(prefix)
                    && !exclude_names.iter().any(|n| n.eq_ignore_ascii_case(&entry.name))
            })
            .collect();
        supervisors_first(&matched)
    }

    /// The inverse: the helpers under `prefix` that [`matching`](Self::matching)
    /// leaves out.
    ///
    /// They are excluded from the running/stopped decision because they outlive
    /// the service, which is exactly why something has to clean them up
    /// afterwards.
    pub fn helpers(&self, prefix: &Path, exclude_names: &[String]) -> Vec<u32> {
        self.entries
            .iter()
            .filter(|entry| {
                entry.exe.starts_with(prefix)
                    && exclude_names.iter().any(|n| n.eq_ignore_ascii_case(&entry.name))
            })
            .map(|entry| entry.pid)
            .collect()
    }
}

/// Order a service's processes so the ones that own the others come first.
///
/// A supervisor is a matched process whose parent is not itself matched: the
/// nginx master rather than a worker, the `php-cgi.exe` that forked the
/// `PHP_FCGI_CHILDREN` pool, `erl.exe` rather than the `inet_gethost.exe` and
/// `win32sysinfo.exe` port programs the Erlang VM starts. Sorting by PID would
/// pick whichever happened to be numbered lowest, which is a worker more often
/// than not -- a useless number to report, and the wrong one to signal.
fn supervisors_first(matched: &[&ProcessEntry]) -> Vec<u32> {
    let pids: Vec<u32> = matched.iter().map(|e| e.pid).collect();
    let owned = |entry: &ProcessEntry| entry.parent.is_some_and(|p| pids.contains(&p));

    let mut ordered: Vec<u32> =
        matched.iter().filter(|e| !owned(e)).map(|e| e.pid).collect();
    ordered.extend(matched.iter().filter(|e| owned(e)).map(|e| e.pid));
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(pid: u32, parent: u32, name: &str) -> ProcessEntry {
        ProcessEntry {
            pid,
            parent: Some(parent),
            name: name.into(),
            exe: PathBuf::from(name),
        }
    }

    /// The FastCGI pool: `start.bat` launches one `php-cgi.exe`, which forks
    /// PHP_FCGI_CHILDREN workers. Windows hands those workers lower PIDs often
    /// enough that "lowest" is not "master".
    #[test]
    fn the_forking_process_leads_its_pool() {
        let pool = [
            entry(3408, 10716, "php-cgi.exe"),
            entry(4764, 10716, "php-cgi.exe"),
            entry(10716, 8204, "php-cgi.exe"),
        ];
        let matched: Vec<&ProcessEntry> = pool.iter().collect();
        assert_eq!(supervisors_first(&matched), vec![10716, 3408, 4764]);
    }

    /// RabbitMQ is `erl.exe`; the rest are port programs it opened.
    #[test]
    fn erlang_port_programs_follow_the_vm() {
        let node = [
            entry(60, 1068, "inet_gethost.exe"),
            entry(1068, 4728, "erl.exe"),
            entry(15656, 1068, "win32sysinfo.exe"),
        ];
        let matched: Vec<&ProcessEntry> = node.iter().collect();
        assert_eq!(supervisors_first(&matched), vec![1068, 60, 15656]);
    }

    /// Unrelated processes are all supervisors, and keep their PID order.
    #[test]
    fn independent_processes_keep_their_order() {
        let procs = [entry(200, 4, "a.exe"), entry(900, 4, "b.exe")];
        let matched: Vec<&ProcessEntry> = procs.iter().collect();
        assert_eq!(supervisors_first(&matched), vec![200, 900]);
    }
}
