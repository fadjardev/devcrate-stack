//! Which log files this stack has, and what to call them.
//!
//! Discovery rather than a fixed list: every vhost adds two nginx logs, and a
//! version of MariaDB or RabbitMQ names its file after the machine. Anything
//! that is not there is simply not offered.

use std::path::{Path, PathBuf};

use crate::config::{ServiceKind, Stack};

#[derive(Debug, Clone)]
pub struct LogFile {
    /// What the picker shows, e.g. `nginx  myapp.test.error`.
    pub label: String,
    pub path: PathBuf,
    pub size: u64,
}

/// Every log file belonging to this stack, grouped by service and sorted so the
/// error logs of each come before the access logs -- when something is wrong,
/// the error log is what you want.
pub fn discover(stack: &Stack) -> Vec<LogFile> {
    let mut files = Vec::new();

    collect(&mut files, "nginx", &stack.nginx_prefix.join("logs"), &["log"]);

    if let Some(mariadb) = stack.by_kind(ServiceKind::MariaDb)
        && let Some(dir) = mariadb.install_marker.parent().and_then(Path::parent)
    {
        // MariaDB writes <hostname>.err into its data directory by default.
        collect(&mut files, "mariadb", &dir.join("data"), &["err", "log"]);
    }

    if let Some(rabbit) = stack.by_kind(ServiceKind::RabbitMq)
        && let Some(dir) = rabbit.install_marker.parent().and_then(Path::parent)
    {
        collect(&mut files, "rabbitmq", &dir.join("data").join("log"), &["log"]);
    }

    files.sort_by(|a, b| {
        // Error logs first within a group; the group prefix keeps services
        // together because it sorts as part of the label.
        let rank = |file: &LogFile| !file.label.contains("error") && !file.label.contains(".err");
        (rank(a), a.label.clone()).cmp(&(rank(b), b.label.clone()))
    });
    files
}

fn collect(into: &mut Vec<LogFile>, service: &str, dir: &Path, extensions: &[&str]) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let matches = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| extensions.iter().any(|want| ext.eq_ignore_ascii_case(want)));
        if !matches || !path.is_file() {
            continue;
        }
        let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        into.push(LogFile {
            label: format!("{service}  {stem}"),
            size: entry.metadata().map(|meta| meta.len()).unwrap_or(0),
            path,
        });
    }
}

/// `1.2 MB`, `840 B` -- the picker shows this so an empty log is obvious
/// before you select it and wonder why nothing appears.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [(&str, u64); 3] = [("GB", 1 << 30), ("MB", 1 << 20), ("kB", 1 << 10)];
    for (unit, scale) in UNITS {
        if bytes >= scale {
            return format!("{:.1} {unit}", bytes as f64 / scale as f64);
        }
    }
    format!("{bytes} B")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_in_the_largest_unit_that_fits() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(2048), "2.0 kB");
        assert_eq!(human_size(5 * (1 << 20)), "5.0 MB");
    }
}
