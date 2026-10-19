//! Installing Bun into `bun\v<version>\`.
//!
//! Validates `bun.exe` inside the archive and moves it into place under `bun\`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

/// `bun-windows-x64.zip` carries no version; the download is cached under the
/// release's tag instead, `bun-v1.4.3-windows-x64.zip`.
pub fn version_from_file_name(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let rest = lower.strip_prefix("bun-v").or_else(|| lower.strip_prefix("bun-"))?;
    let version = rest.trim_end_matches(".zip").split('-').next()?;
    let dotted = !version.is_empty()
        && version.split('.').all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    dotted.then(|| version.to_string())
}

pub fn validate(staging_dir: &Path) -> Result<PathBuf> {
    let direct = staging_dir.join("bun.exe");
    if direct.is_file() {
        return Ok(staging_dir.to_path_buf());
    }

    if let Ok(entries) = fs::read_dir(staging_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("bun.exe").is_file() {
                return Ok(path);
            }
        }
    }

    bail!(
        "staging directory {} contains no bun.exe",
        staging_dir.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_from_file_name() {
        assert_eq!(
            version_from_file_name("bun-v1.2.2.zip").as_deref(),
            Some("1.2.2")
        );
        assert_eq!(
            version_from_file_name("bun-v1.4.3-windows-x64.zip").as_deref(),
            Some("1.4.3")
        );
        assert_eq!(version_from_file_name("bun-1.4.3-windows-x64.zip").as_deref(), Some("1.4.3"));
        // The asset as GitHub names it: no version to read.
        assert_eq!(version_from_file_name("bun-windows-x64.zip"), None);
        assert_eq!(version_from_file_name("bun-windows-x64-baseline.zip"), None);
    }
}
