//! Local TLS Certificate Management via mkcert.
//!
//! Automatically issues and updates TLS certificates for local vhosts into
//! `nginx/conf/certs/`, one per domain group: `myapp.test` gets a certificate
//! for `myapp.test` and `*.myapp.test`, and `api.mygroup.test` shares one for
//! `mygroup.test` and `*.mygroup.test` with its siblings.
//!
//! Never `*.test` on its own. Browsers refuse a wildcard that sits directly
//! under a top-level domain, so a `*.test` certificate is rejected for
//! `myapp.test` (`ERR_CERT_COMMON_NAME_INVALID` in Chrome) even though it
//! appears to cover it. An older `_wildcard.test.pem` from `new-vhost.bat` is
//! therefore no longer reused.
//!
//! mkcert's own output is discarded: these run from the dashboard too, where
//! anything printed would land on the screen it just drew.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
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

/// Issue or reuse the certificate for the host's domain group.
/// Returns the name of the cert file used (e.g., `_wildcard.mygroup.test.pem`).
pub fn ensure_cert_for_host(stack: &Stack, host: &str) -> Result<Option<String>> {
    let certs_dir = stack.nginx_prefix.join("conf").join("certs");
    if !certs_dir.is_dir() {
        let _ = std::fs::create_dir_all(&certs_dir);
    }

    let domain = cert_domain(host);

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
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("running mkcert to issue {wildcard_pattern}"))?;

    if status.success() && cert_path.is_file() {
        Ok(Some(cert_file_name))
    } else {
        Ok(None)
    }
}

/// The domain a host's certificate is issued for: the host itself when it is
/// directly under the TLD (`myapp.test`), its parent otherwise
/// (`api.mygroup.test` -> `mygroup.test`). Never the bare TLD, whose wildcard
/// browsers reject.
pub fn cert_domain(host: &str) -> &str {
    parent_domain(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `*.test` is never the answer: browsers refuse a wildcard straight under
    /// a TLD, so a second-level host gets its own name on the certificate.
    #[test]
    fn certificates_are_never_issued_for_the_bare_tld() {
        assert_eq!(cert_domain("db.test"), "db.test");
        assert_eq!(cert_domain("api.mygroup.test"), "mygroup.test");
        assert_eq!(cert_domain("a.b.mygroup.test"), "b.mygroup.test");
    }

    #[test]
    fn test_parent_domain() {
        assert_eq!(parent_domain("api.mygroup.test"), "mygroup.test");
        assert_eq!(parent_domain("myapp.test"), "myapp.test");
    }
}
