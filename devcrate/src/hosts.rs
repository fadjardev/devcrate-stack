//! Reading, updating, and managing the Windows hosts file.
//!
//! All Devcrate entries are isolated within a dedicated block:
//! ```text
//! # --- devcrate begin ---
//! 127.0.0.1   myapp.test
//! 127.0.0.1   api.mygroup.test
//! # --- devcrate end ---
//! ```
//!
//! A backup `hosts.bak` is created before the first edit. A write goes to a
//! temp file beside the hosts file and is renamed over it, so an interrupted
//! write never leaves it truncated. If that is denied (UAC protection), an
//! elevation prompt (via PowerShell `Start-Process -Verb RunAs`) copies the
//! new content into place instead.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

const BEGIN_MARKER: &str = "# --- devcrate begin ---";
const END_MARKER: &str = "# --- devcrate end ---";

#[derive(Debug, PartialEq, Eq)]
pub enum HostsResult {
    Updated,
    Unchanged,
    FallbackManual(String),
}

/// Path to the system hosts file.
pub fn hosts_path() -> PathBuf {
    if let Some(sys_root) = std::env::var_os("SystemRoot") {
        Path::new(&sys_root)
            .join("System32")
            .join("drivers")
            .join("etc")
            .join("hosts")
    } else {
        PathBuf::from(r"C:\Windows\System32\drivers\etc\hosts")
    }
}

/// Backup hosts file to `hosts.bak` if not already backed up.
pub fn backup_hosts() -> Result<()> {
    let target = hosts_path();
    if !target.is_file() {
        return Ok(());
    }
    let backup = target.with_file_name("hosts.bak");
    if !backup.exists() {
        let _ = fs::copy(&target, &backup);
    }
    Ok(())
}

/// Parse hosts content and update the devcrate block.
pub fn update_devcrate_block(content: &str, mut entries: Vec<String>) -> (String, bool) {
    entries.sort();
    entries.dedup();

    let lines: Vec<&str> = content.lines().collect();
    let mut begin_idx = None;
    let mut end_idx = None;

    for (i, line) in lines.iter().enumerate() {
        if line.trim() == BEGIN_MARKER {
            begin_idx = Some(i);
        } else if line.trim() == END_MARKER {
            end_idx = Some(i);
        }
    }

    let new_block_lines: Vec<String> = if entries.is_empty() {
        vec![]
    } else {
        let mut b = vec![BEGIN_MARKER.to_string()];
        for entry in &entries {
            b.push(format!("127.0.0.1   {entry}"));
        }
        b.push(END_MARKER.to_string());
        b
    };

    let mut result_lines = Vec::new();
    if let (Some(start), Some(end)) = (begin_idx, end_idx) {
        if start < end {
            result_lines.extend_from_slice(&lines[..start]);
            for l in &new_block_lines {
                result_lines.push(l.as_str());
            }
            if end + 1 < lines.len() {
                result_lines.extend_from_slice(&lines[end + 1..]);
            }
        } else {
            result_lines.extend_from_slice(&lines);
            if !new_block_lines.is_empty() {
                result_lines.extend(new_block_lines.iter().map(|s| s.as_str()));
            }
        }
    } else {
        result_lines.extend_from_slice(&lines);
        if !new_block_lines.is_empty() {
            if !result_lines.is_empty() && !result_lines.last().unwrap().is_empty() {
                result_lines.push("");
            }
            result_lines.extend(new_block_lines.iter().map(|s| s.as_str()));
        }
    }

    let mut new_content = result_lines.join("\r\n");
    if content.ends_with("\n") || content.ends_with("\r\n") {
        new_content.push_str("\r\n");
    }

    let changed = new_content != content;
    (new_content, changed)
}

/// Extract current entries inside the devcrate block.
pub fn read_devcrate_entries(content: &str) -> Vec<String> {
    let mut in_block = false;
    let mut entries = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == BEGIN_MARKER {
            in_block = true;
            continue;
        }
        if trimmed == END_MARKER {
            in_block = false;
            continue;
        }
        if in_block
            && let Some(rest) = trimmed.strip_prefix("127.0.0.1")
        {
            let h = rest.trim();
            if !h.is_empty() {
                entries.push(h.to_string());
            }
        }
    }
    entries
}

/// Write new hosts content, using UAC elevation fallback if permission denied.
pub fn write_hosts_content(new_content: &str) -> Result<HostsResult> {
    let target = hosts_path();
    let _ = backup_hosts();

    // Atomic where it can be: a reader -- or an interruption -- sees the old
    // file or the new one, never half of it. The rename can still be refused
    // with permission to write (the DNS client holding the file open), and a
    // plain write is better then than an elevation prompt nobody needed.
    let written = replace_atomically(&target, new_content)
        .or_else(|_| fs::write(&target, new_content));

    match written {
        Ok(_) => Ok(HostsResult::Updated),
        Err(_) => {
            // Permission denied -> write to temp file and request elevation
            let temp_file = std::env::temp_dir().join("devcrate_hosts.txt");
            fs::write(&temp_file, new_content)
                .with_context(|| format!("writing temp file {}", temp_file.display()))?;

            let powershell_cmd = format!(
                "Start-Process cmd -Verb RunAs -ArgumentList '/c copy /y \"{}\" \"{}\"' -Wait",
                temp_file.display(),
                target.display()
            );

            // Silenced: this also runs from the dashboard, where anything
            // PowerShell printed would land on the screen it just drew.
            let status = Command::new("powershell")
                .args(["-NoProfile", "-NonInteractive", "-Command", &powershell_cmd])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();

            let _ = fs::remove_file(&temp_file);

            if let Ok(st) = status
                && st.success()
            {
                Ok(HostsResult::Updated)
            } else {
                Ok(HostsResult::FallbackManual(
                    "Elevation declined or failed".to_string(),
                ))
            }
        }
    }
}

/// Write `content` to a temp file beside `target`, then rename it over
/// `target` -- the same directory, so the rename is a replace on one volume
/// rather than a copy. The temp file is removed if the rename fails.
fn replace_atomically(target: &Path, content: &str) -> std::io::Result<()> {
    let temp = target.with_file_name(format!(
        "{}.devcrate-tmp",
        target.file_name().unwrap_or_default().to_string_lossy()
    ));
    fs::write(&temp, content)?;
    fs::rename(&temp, target).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}

/// Add an IP mapping for `host` in the devcrate block.
pub fn add_entry(host: &str) -> Result<HostsResult> {
    let target = hosts_path();
    let content = fs::read_to_string(&target).unwrap_or_default();
    let mut entries = read_devcrate_entries(&content);

    if entries.iter().any(|e| e.eq_ignore_ascii_case(host)) {
        return Ok(HostsResult::Unchanged);
    }

    entries.push(host.to_string());
    let (new_content, changed) = update_devcrate_block(&content, entries);

    if !changed {
        return Ok(HostsResult::Unchanged);
    }

    write_hosts_content(&new_content)
}

/// Remove a host mapping from the devcrate block.
pub fn remove_entry(host: &str) -> Result<HostsResult> {
    let target = hosts_path();
    let content = fs::read_to_string(&target).unwrap_or_default();
    let mut entries = read_devcrate_entries(&content);

    let initial_len = entries.len();
    entries.retain(|e| !e.eq_ignore_ascii_case(host));

    if entries.len() == initial_len {
        return Ok(HostsResult::Unchanged);
    }

    let (new_content, changed) = update_devcrate_block(&content, entries);
    if !changed {
        return Ok(HostsResult::Unchanged);
    }

    write_hosts_content(&new_content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_update_devcrate_block() {
        let initial = "127.0.0.1 localhost\r\n";
        let (updated, changed) = update_devcrate_block(initial, vec!["myapp.test".into()]);
        assert!(changed);
        assert!(updated.contains(BEGIN_MARKER));
        assert!(updated.contains("127.0.0.1   myapp.test"));
        assert!(updated.contains(END_MARKER));

        let entries = read_devcrate_entries(&updated);
        assert_eq!(entries, vec!["myapp.test"]);

        let (removed, changed2) = update_devcrate_block(&updated, vec![]);
        assert!(changed2);
        assert!(!removed.contains(BEGIN_MARKER));
        assert_eq!(removed.trim(), "127.0.0.1 localhost");
    }

    /// The replace lands the whole new content and leaves no temp file behind.
    #[test]
    fn replacing_atomically_swaps_the_whole_file() {
        let dir = std::env::temp_dir()
            .join(format!("devcrate-hosts-replace-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("hosts");
        fs::write(&target, "127.0.0.1 localhost\r\n").unwrap();

        let new = "127.0.0.1 localhost\r\n# --- devcrate begin ---\r\n127.0.0.1   myapp.test\r\n# --- devcrate end ---\r\n";
        replace_atomically(&target, new).unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), new);
        assert!(!dir.join("hosts.devcrate-tmp").exists());

        let _ = fs::remove_dir_all(&dir);
    }
}
