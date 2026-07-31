//! Local TLS Certificate Management via mkcert.
//!
//! Automatically issues and updates TLS certificates for local vhosts into
//! `nginx/conf/certs/`. Reuses `_wildcard.test.pem` for standard `*.test` domains
//! and issues domain-group wildcards (e.g. `*.mygroup.test`) for 3rd-level domains.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};

use crate::config::Stack;

/// Find mkcert executable in stack root, PATH, or `mkcert/`.
pub fn find_mkcert(stack: &Stack) -> Option<PathBuf> {
    let candidates = [
        stack.root.join("mkcert.exe"),
        stack.root.join("mkcert").join("mkcert.exe"),
    ];

    for candidate in candidates {
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    if let Ok(path_var) = std::env::var("PATH") {
        for p in std::env::split_paths(&path_var) {
            let candidate = p.join("mkcert.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

/// Ensure mkcert Root CA is installed in the local Windows store.
pub fn ensure_ca_installed(mkcert_bin: &Path) -> Result<()> {
    let status = Command::new(mkcert_bin)
        .arg("-install")
        .status()
        .with_context(|| format!("running {} -install", mkcert_bin.display()))?;

    if !status.success() {
        return Err(anyhow!(
            "mkcert -install failed with exit code {:?}",
            status.code()
        ));
    }
    Ok(())
}

/// Parent domain e.g. `api.mygroup.test` -> `mygroup.test`, `myapp.test` -> `myapp.test`.
pub fn parent_domain(host: &str) -> &str {
    if host.matches('.').count() >= 2 {
        host.split_once('.').map(|(_, rest)| rest).unwrap_or(host)
    } else {
        host
    }
}

/// Issue or reuse a TLS wildcard certificate for the target host.
/// Returns the name of the cert file used (e.g., `_wildcard.mygroup.test.pem`).
pub fn ensure_cert_for_host(stack: &Stack, host: &str) -> Result<Option<String>> {
    let certs_dir = stack.nginx_prefix.join("conf").join("certs");
    if !certs_dir.is_dir() {
        let _ = std::fs::create_dir_all(&certs_dir);
    }

    let is_third_level = host.matches('.').count() > 1;
    if !is_third_level {
        // Standard *.test domain is covered by existing default wildcard
        let default_cert = certs_dir.join("_wildcard.test.pem");
        if default_cert.is_file() {
            return Ok(Some("_wildcard.test.pem".to_string()));
        }
    }

    let domain = if is_third_level {
        parent_domain(host)
    } else {
        "test"
    };

    let cert_file_name = format!("_wildcard.{domain}.pem");
    let key_file_name = format!("_wildcard.{domain}-key.pem");
    let cert_path = certs_dir.join(&cert_file_name);
    let key_path = certs_dir.join(&key_file_name);

    if cert_path.is_file() && key_path.is_file() {
        return Ok(Some(cert_file_name));
    }

    let Some(mkcert_bin) = find_mkcert(stack) else {
        return Ok(None);
    };

    let _ = ensure_ca_installed(&mkcert_bin);

    let wildcard_pattern = format!("*.{domain}");
    let status = Command::new(&mkcert_bin)
        .current_dir(&certs_dir)
        .args([
            "-cert-file",
            &cert_file_name,
            "-key-file",
            &key_file_name,
            &wildcard_pattern,
            domain,
        ])
        .status()
        .with_context(|| format!("running mkcert to issue {wildcard_pattern}"))?;

    if status.success() && cert_path.is_file() {
        Ok(Some(cert_file_name))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parent_domain() {
        assert_eq!(parent_domain("api.mygroup.test"), "mygroup.test");
        assert_eq!(parent_domain("myapp.test"), "myapp.test");
    }
}
