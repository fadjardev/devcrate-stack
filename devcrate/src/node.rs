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

/// Keep npm's cache and global packages inside `node\`: a `prefix` and `cache`
/// in `<version>\etc\npmrc`, which is the global config npm reads on Windows
/// (`<prefix>\etc\npmrc`, the prefix defaulting to node.exe's folder). Written
/// only when missing, so an edited one stays. Returns whether it was written.
pub fn ensure_npmrc(node_root: &Path, version_dir: &Path) -> std::io::Result<bool> {
    let npmrc = version_dir.join("etc").join("npmrc");
    if npmrc.exists() {
        return Ok(false);
    }
    let npm_global = node_root.join("npm-global");
    let npm_cache = node_root.join("npm-cache");
    std::fs::create_dir_all(&npm_global)?;
    std::fs::create_dir_all(&npm_cache)?;
    std::fs::create_dir_all(version_dir.join("etc"))?;
    let slashed = |p: &Path| p.display().to_string().replace('\\', "/");
    std::fs::write(&npmrc, format!("prefix={}\ncache={}\n", slashed(&npm_global), slashed(&npm_cache)))?;
    Ok(true)
}

pub fn switch(stack: &Stack, wanted: &str) -> Result<u8> {
    let target = find(stack, wanted)?;
    let node_dir = stack.root.join("node");
    // Installs before this was fixed wrote a .npmrc npm never read.
    if ensure_npmrc(&node_dir, &target.path).unwrap_or(false) {
        println!("npm's cache and global prefix now kept in node\\ (wrote etc\\npmrc)");
    }
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


