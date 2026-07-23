//! Inspection: which ports answer, which process holds one, and which of our
//! processes run.
//!
//! The only thing here that acts rather than observes is [`ProcessTable::kill`],
//! and it can only reach a process the caller has already matched by path.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    /// When the process started, in seconds since the Unix epoch.
    ///
    /// Stored as an absolute point rather than an elapsed time so it does not
    /// go stale while a snapshot is held -- which matters for a dashboard that
    /// refreshes on its own schedule.
    pub started: u64,
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
                    started: process.start_time(),
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

    fn entry(&self, pid: u32) -> Option<&ProcessEntry> {
        self.entries.iter().find(|entry| entry.pid == pid)
    }

    /// How long a process has been running, or `None` if it is not in this
    /// snapshot or its clock reads as being in the future.
    pub fn uptime(&self, pid: u32) -> Option<Duration> {
        let started = self.entry(pid)?.started;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        now.checked_sub(started).map(Duration::from_secs)
    }

    /// The image name of any process, ours or not -- for saying *who* holds a
    /// port rather than only that it is taken.
    pub fn name_of(&self, pid: u32) -> Option<&str> {
        self.entry(pid).map(|entry| entry.name.as_str())
    }
}

/// Which process is listening on each local TCP port.
///
/// A connect probe answers "is this port taken"; only the kernel's own table
/// answers "by whom". `sysinfo` does not expose it -- it has no socket support
/// at all -- so this is the one place that has to go to the Win32 API directly.
/// Nothing here needs elevation: `TCP_TABLE_OWNER_PID_LISTENER` returns the
/// owning PID for every listener on the machine, including other users'.
#[cfg(windows)]
pub fn listeners() -> BTreeMap<u16, u32> {
    use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
    };

    /// `AF_INET`. Spelled out rather than pulling in the whole WinSock module
    /// for one constant.
    const IPV4: u32 = 2;

    let mut ports = BTreeMap::new();
    // Backed by u32 rather than u8 so the buffer is aligned for the table
    // struct it is about to be read as.
    let mut buffer: Vec<u32> = vec![0; 1024];

    // The table can grow between the sizing call and the read, so ask again
    // with the size it reported. Twice is enough in practice; the loop bound
    // is there so a pathological machine cannot spin here forever.
    for _ in 0..4 {
        let mut size = (buffer.len() * size_of::<u32>()) as u32;
        // Safety: `buffer` is at least `size` bytes and aligned for the table;
        // the call only writes into it, and reports the size it needed when it
        // does not fit.
        let result = unsafe {
            GetExtendedTcpTable(
                buffer.as_mut_ptr().cast(),
                &mut size,
                0, // no need to sort
                IPV4,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };

        if result == ERROR_INSUFFICIENT_BUFFER {
            buffer.resize(size as usize / size_of::<u32>() + 1, 0);
            continue;
        }
        if result != NO_ERROR {
            return ports;
        }

        // Safety: the call succeeded, so the buffer holds a MIB_TCPTABLE_OWNER_PID
        // whose `dwNumEntries` rows follow it contiguously.
        unsafe {
            let table = &*buffer.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
            let rows = std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize);
            for row in rows {
                // The port sits in the low word, in network byte order.
                let port = u16::from_be((row.dwLocalPort & 0xffff) as u16);
                ports.entry(port).or_insert(row.dwOwningPid);
            }
        }
        break;
    }

    ports
}

#[cfg(not(windows))]
pub fn listeners() -> BTreeMap<u16, u32> {
    BTreeMap::new()
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
            started: 0,
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

    /// This process is listening on nothing in particular, but it is running,
    /// so the table must contain it and report a plausible age.
    #[test]
    fn a_process_in_the_table_has_an_age() {
        let table = ProcessTable::scan();
        let me = std::process::id();
        assert!(table.name_of(me).is_some(), "the test runner should be in its own scan");
        assert!(table.uptime(me).is_some(), "and it started at some point in the past");
        assert!(table.uptime(u32::MAX).is_none(), "an unknown PID has no age");
    }

    /// Whatever else is on the machine, something is listening somewhere, and
    /// every port it reports has to be a real port owned by a real PID.
    #[test]
    fn the_listener_table_reads_as_ports_not_byte_pairs() {
        for (port, pid) in listeners() {
            assert!(port > 0, "port 0 means the byte order came out wrong");
            assert!(pid > 0, "every listener has an owning process");
        }
    }
}
