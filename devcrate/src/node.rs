//! Managing installed Node.js versions and `node\current` junction.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::config::Stack;

#[derive(Debug)]
pub struct NodeInstalledVersion {
    pub version: String,
    pub path: PathBuf,
    pub active: bool,
}

pub fn list_installed(stack: &Stack) -> Vec<NodeInstalledVersion> {
    let node_dir = stack.root.join("node");
    let current_target = node_dir.join("current").canonicalize().ok();

    let mut versions = Vec::new();
    let Ok(entries) = std::fs::read_dir(&node_dir) else {
        return versions;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "current" || name == "npm-global" || name == "npm-cache" {
            continue;
        }

        if path.join("node.exe").is_file() || path.join("bin").join("node.exe").is_file() {
            let ver = name.trim_start_matches('v').to_string();
            let active = current_target.as_ref() == Some(&path.canonicalize().unwrap_or_else(|_| path.clone()));
            versions.push(NodeInstalledVersion { version: ver, path, active });
        }
    }

    versions.sort_by(|a, b| b.version.cmp(&a.version));
    versions
}

pub fn find(stack: &Stack, wanted: &str) -> Result<NodeInstalledVersion> {
    let installed = list_installed(stack);
    if installed.is_empty() {
        bail!("no Node.js versions installed under node\\; run `devcrate install node` first");
    }

    let wanted_clean = wanted.trim_start_matches('v');
    for v in &installed {
        if v.version == wanted_clean || v.version.starts_with(wanted_clean) {
            return Ok(NodeInstalledVersion {
                version: v.version.clone(),
                path: v.path.clone(),
                active: v.active,
            });
        }
    }

    bail!("no installed Node.js version matches '{wanted}'")
}

pub fn switch(stack: &Stack, wanted: &str) -> Result<u8> {
    let target = find(stack, wanted)?;
    let node_dir = stack.root.join("node");
    let current_junction = node_dir.join("current");

    if current_junction.exists() {
        let _ = std::fs::remove_file(&current_junction);
        let _ = std::fs::remove_dir_all(&current_junction);
    }

    crate::junction::create(&current_junction, &target.path)
        .with_context(|| format!("creating junction {}", current_junction.display()))?;

    println!("CLI Node.js switched -> v{}", target.version);
    Ok(crate::exit::OK)
}

/// Detect required Node version from .nvmrc or package.json's engines.node.
#[allow(dead_code)]
pub fn detect_node_version(project_dir: &Path) -> Option<String> {
    let nvmrc = project_dir.join(".nvmrc");
    if let Ok(text) = std::fs::read_to_string(&nvmrc) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.trim_start_matches('v').to_string());
        }
    }

    let pkg_json = project_dir.join("package.json");
    if let Ok(text) = std::fs::read_to_string(&pkg_json)
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
        && let Some(engine_node) = json.get("engines").and_then(|e| e.get("node")).and_then(|n| n.as_str())
    {
        let mut digits = String::new();
        for c in engine_node.chars() {
            if c.is_ascii_digit() || c == '.' {
                digits.push(c);
            } else if !digits.is_empty() {
                break;
            }
        }
        if !digits.is_empty() {
            return Some(digits);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_node_version_nvmrc() {
        let dir = std::env::temp_dir().join("devcrate-node-nvmrc");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join(".nvmrc"), "v22.11.0\n").unwrap();
        assert_eq!(detect_node_version(&dir).as_deref(), Some("22.11.0"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_detect_node_version_package_json() {
        let dir = std::env::temp_dir().join("devcrate-node-pkg");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("package.json"), r#"{"engines": {"node": ">=20.0.0"}}"#).unwrap();
        assert_eq!(detect_node_version(&dir).as_deref(), Some("20.0.0"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
