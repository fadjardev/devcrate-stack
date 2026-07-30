//! Installing Bun into `bun\v<version>\`.
//!
//! Validates `bun.exe` inside the archive and moves it into place under `bun\`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

#[derive(Debug, PartialEq, Eq)]
pub struct BunInstalled {
    pub version: String,
    pub target: PathBuf,
}

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

pub fn finish(staging_dir: &Path, version: &str, bun_root: &Path) -> Result<BunInstalled> {
    let valid_dir = validate(staging_dir)?;
    let target = bun_root.join(format!("v{version}"));

    if target.exists() {
        let _ = fs::remove_dir_all(&target);
    }

    fs::rename(&valid_dir, &target)
        .with_context(|| format!("moving bun into {}", target.display()))?;

    Ok(BunInstalled { version: version.to_string(), target })
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
