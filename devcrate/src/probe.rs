//! Read-only inspection: which ports answer, and which of our processes run.
//!
//! Nothing here starts, stops, or writes anything.

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, UpdateKind};

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
    pub name: String,
    pub exe: PathBuf,
}

/// A snapshot of the running processes that have a resolvable executable path.
#[derive(Debug, Default)]
pub struct ProcessTable {
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
                    name: process.name().to_string_lossy().into_owned(),
                    exe,
                })
            })
            .collect();
        entries.sort_by_key(|e| e.pid);
        ProcessTable { entries }
    }

    /// PIDs whose executable is `prefix`, or lives under it.
    ///
    /// Matching on the path is deliberate: it means a system-wide nginx or
    /// MariaDB is never mistaken for the one in this stack root.
    pub fn matching(&self, prefix: &Path, exclude_names: &[String]) -> Vec<u32> {
        self.entries
            .iter()
            .filter(|entry| {
                entry.exe.starts_with(prefix)
                    && !exclude_names.iter().any(|n| n.eq_ignore_ascii_case(&entry.name))
            })
            .map(|entry| entry.pid)
            .collect()
    }
}
