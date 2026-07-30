//! Managing installed Bun versions and `bun\current` junction.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::config::Stack;

#[derive(Debug)]
pub struct BunInstalledVersion {
    pub version: String,
    pub path: PathBuf,
    pub active: bool,
}

pub fn list_installed(stack: &Stack) -> Vec<BunInstalledVersion> {
    let bun_dir = stack.root.join("bun");
    let current_target = bun_dir.join("current").canonicalize().ok();

    let mut versions = Vec::new();
    let Ok(entries) = std::fs::read_dir(&bun_dir) else {
        return versions;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "current" {
            continue;
        }

        if path.join("bun.exe").is_file() {
            let ver = name.trim_start_matches('v').to_string();
            let active = current_target.as_ref() == Some(&path.canonicalize().unwrap_or_else(|_| path.clone()));
            versions.push(BunInstalledVersion { version: ver, path, active });
        }
    }

    versions.sort_by(|a, b| b.version.cmp(&a.version));
    versions
}

pub fn find(stack: &Stack, wanted: &str) -> Result<BunInstalledVersion> {
    let installed = list_installed(stack);
    if installed.is_empty() {
        bail!("no Bun versions installed under bun\\; run `devcrate install bun` first");
    }

    let wanted_clean = wanted.trim_start_matches('v');
    for v in &installed {
        if v.version == wanted_clean || v.version.starts_with(wanted_clean) {
            return Ok(BunInstalledVersion {
                version: v.version.clone(),
                path: v.path.clone(),
                active: v.active,
            });
        }
    }

    bail!("no installed Bun version matches '{wanted}'")
}

pub fn switch(stack: &Stack, wanted: &str) -> Result<u8> {
    let target = find(stack, wanted)?;
    let bun_dir = stack.root.join("bun");
    let current_junction = bun_dir.join("current");

    if current_junction.exists() {
        let _ = std::fs::remove_file(&current_junction);
        let _ = std::fs::remove_dir_all(&current_junction);
    }

    crate::junction::create(&current_junction, &target.path)
        .with_context(|| format!("creating junction {}", current_junction.display()))?;

    println!("CLI Bun switched -> v{}", target.version);
    Ok(crate::exit::OK)
}
