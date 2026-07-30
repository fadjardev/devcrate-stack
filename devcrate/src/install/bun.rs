//! Installing Bun into `bun\v<version>\`.
//!
//! Validates `bun.exe` inside the archive and moves it into place under `bun\`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

pub fn version_from_file_name(name: &str) -> Option<String> {
    let name = name.trim_end_matches(".zip");
    if let Some(rest) = name.strip_prefix("bun-v") {
        return Some(rest.to_string());
    }
    if let Some(rest) = name.strip_prefix("bun-") {
        return Some(rest.trim_start_matches('v').to_string());
    }
    None
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
    }
}
