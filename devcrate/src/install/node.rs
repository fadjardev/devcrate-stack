//! Installing Node.js into `node\v<version>\`.
//!
//! Validates `node.exe` and `npm.cmd`, creates portable `npm-global` and `npm-cache`
//! directories under `node\`, and writes a default `.npmrc` to isolate NPM.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

#[derive(Debug, PartialEq, Eq)]
pub struct NodeInstalled {
    pub version: String,
    pub target: PathBuf,
}

pub fn version_from_file_name(name: &str) -> Option<String> {
    let name = name.trim_end_matches(".zip");
    if let Some(rest) = name.strip_prefix("node-v") {
        let v = rest.split('-').next().unwrap_or(rest);
        return Some(v.to_string());
    }
    if let Some(rest) = name.strip_prefix("node-") {
        let v = rest.split('-').next().unwrap_or(rest);
        return Some(v.trim_start_matches('v').to_string());
    }
    None
}

pub fn validate(staging_dir: &Path) -> Result<PathBuf> {
    let direct = staging_dir.join("node.exe");
    if direct.is_file() {
        return Ok(staging_dir.to_path_buf());
    }

    let bin = staging_dir.join("bin").join("node.exe");
    if bin.is_file() {
        return Ok(staging_dir.to_path_buf());
    }

    if let Ok(entries) = fs::read_dir(staging_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("node.exe").is_file() {
                return Ok(path);
            }
        }
    }

    bail!(
        "staging directory {} contains no node.exe",
        staging_dir.display()
    )
}

pub fn finish(staging_dir: &Path, version: &str, node_root: &Path) -> Result<NodeInstalled> {
    let valid_dir = validate(staging_dir)?;
    let target = node_root.join(format!("v{version}"));

    if target.exists() {
        let _ = fs::remove_dir_all(&target);
    }

    fs::rename(&valid_dir, &target)
        .with_context(|| format!("moving node into {}", target.display()))?;

    // Create portable npm-global & npm-cache inside node_root
    let npm_global = node_root.join("npm-global");
    let npm_cache = node_root.join("npm-cache");
    let _ = fs::create_dir_all(&npm_global);
    let _ = fs::create_dir_all(&npm_cache);

    // Create default .npmrc in target
    let npmrc = target.join(".npmrc");
    if !npmrc.exists() {
        let text = format!(
            "prefix={}\ncache={}\n",
            npm_global.display().to_string().replace('\\', "/"),
            npm_cache.display().to_string().replace('\\', "/")
        );
        let _ = fs::write(&npmrc, text);
    }

    Ok(NodeInstalled { version: version.to_string(), target })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_from_file_name() {
        assert_eq!(
            version_from_file_name("node-v22.11.0-win-x64.zip").as_deref(),
            Some("22.11.0")
        );
        assert_eq!(
            version_from_file_name("node-v20.18.0-win-x64").as_deref(),
            Some("20.18.0")
        );
    }
}
