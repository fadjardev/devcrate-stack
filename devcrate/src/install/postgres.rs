//! Turning an unpacked PostgreSQL archive into a cluster the stack can serve.
//!
//! The shape sits between PHP's and nginx's. Like nginx, the *binaries* carry no
//! configuration this stack keeps -- they are a folder of executables. Unlike
//! nginx, a running server needs something the archive does not contain: an
//! initialised data directory. So the work here is [`initialise`], run once
//! after the binaries are in place, which is `initdb` and nothing more.
//!
//! The data directory is why PostgreSQL is a single install rather than the
//! side-by-side versions PHP keeps. A cluster is written in a version-specific
//! on-disk format, so two majors cannot share one `postgres\data`, and the
//! install is careful never to walk over an existing one -- see the swap in
//! [`super::postgres_from_archive`].

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// The default local development superuser and its password.
///
/// A development stack is single-user and localhost-only, so this is a
/// convenience rather than a secret -- and it is printed on install, so it is
/// never a mystery. `ALTER USER postgres PASSWORD ...` changes it for anything
/// that will ever leave the machine.
pub const SUPERUSER: &str = "postgres";
pub const DEFAULT_PASSWORD: &str = "postgres";

/// `postgresql-13.16-1-windows-x64-binaries.zip` -> `13.16`.
///
/// EDB's own name carries `major.minor` and then its packaging build (`-1`); the
/// build is EDB's, not PostgreSQL's, so it is dropped. A bare major is refused
/// for the same reason PHP and nginx refuse one -- it names nothing exact enough
/// to record.
pub fn version_from_file_name(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".zip").unwrap_or(&lower);
    let rest = stem.strip_prefix("postgresql-")?;

    let version = rest.split('-').next()?;
    if !version.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() < 2 || parts.iter().any(|part| part.is_empty()) {
        return None;
    }
    Some(version.to_string())
}

/// Is this unpacked directory a PostgreSQL binaries distribution for Windows?
///
/// Three files settle it, and each is one the stack actually runs: the server,
/// the tool that creates a cluster, and the tool that stops one cleanly.
pub fn check(dir: &Path, archive: &Path) -> Result<()> {
    for required in ["postgres.exe", "initdb.exe", "pg_ctl.exe"] {
        if !dir.join("bin").join(required).is_file() {
            bail!(
                "{} does not contain bin\\{required}; it does not look like a \
                 PostgreSQL binaries distribution for Windows.\n\
                 The zip to install is EDB's \"binaries\" archive \
                 (postgresql-13.x-1-windows-x64-binaries.zip), not the installer.",
                archive.display()
            );
        }
    }
    Ok(())
}

/// What initialising the cluster came to.
#[derive(Debug)]
pub struct Initialised {
    /// Root-relative data directory.
    pub data_dir: String,
    /// `initdb` ran now, as opposed to an existing cluster being left in place.
    pub created: bool,
    pub superuser: &'static str,
    /// The password set on a freshly created cluster. `None` when an existing
    /// cluster was kept -- its password is whatever it was already.
    pub password: Option<&'static str>,
}

/// Create the cluster under `dest\data` if there is not one there already.
///
/// Idempotent by the marker `initdb` itself leaves: a `PG_VERSION` file at the
/// top of the data directory is what says a cluster lives there, so its presence
/// means "keep this" and its absence means "make one". That is what lets a
/// reinstall preserve an existing database -- the binaries are replaced, the
/// data is found intact, and no `initdb` runs.
///
/// The auth choice is the localhost-development one: `trust` for local
/// connections, a real password over TCP (`scram-sha-256`), and the password
/// seeded from a file that is written, read by `initdb`, and deleted here so it
/// never lingers on disk.
pub fn initialise(dest: &Path, rel: impl Fn(&Path) -> String) -> Result<Initialised> {
    let data = dest.join("data");
    if data.join("PG_VERSION").is_file() {
        return Ok(Initialised {
            data_dir: rel(&data),
            created: false,
            superuser: SUPERUSER,
            password: None,
        });
    }

    let initdb = dest.join("bin").join("initdb.exe");
    if !initdb.is_file() {
        bail!("{} not found", rel(&initdb));
    }

    // Read by initdb, then removed. Kept beside the binaries rather than in the
    // system temp so a failure leaves it inside the stack root to be cleared.
    let pwfile = dest.join(".devcrate-initpw");
    std::fs::write(&pwfile, DEFAULT_PASSWORD)
        .with_context(|| format!("writing the initdb password file in {}", rel(dest)))?;

    let result = Command::new(&initdb)
        .arg("-D")
        .arg(&data)
        .arg(format!("--username={SUPERUSER}"))
        .arg("--auth-local=trust")
        .arg("--auth-host=scram-sha-256")
        .arg("--encoding=UTF8")
        .arg(format!("--pwfile={}", pwfile.display()))
        .output();
    let _ = std::fs::remove_file(&pwfile);

    let output = result.with_context(|| format!("running {}", rel(&initdb)))?;
    if !output.status.success() {
        let detail = [output.stderr, output.stdout]
            .iter()
            .map(|stream| String::from_utf8_lossy(stream).trim().to_string())
            .find(|text| !text.is_empty())
            .unwrap_or_else(|| format!("exit code {}", output.status.code().unwrap_or(-1)));
        bail!("initdb failed: {}", detail.lines().last().unwrap_or(&detail));
    }

    Ok(Initialised {
        data_dir: rel(&data),
        created: true,
        superuser: SUPERUSER,
        password: Some(DEFAULT_PASSWORD),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_is_the_major_minor_from_the_vendors_file_name() {
        assert_eq!(
            version_from_file_name("postgresql-13.16-1-windows-x64-binaries.zip"),
            Some("13.16".into())
        );
        // The EDB packaging build (-2) is theirs, not PostgreSQL's, and dropped.
        assert_eq!(
            version_from_file_name("postgresql-13.20-2-windows-x64-binaries.zip"),
            Some("13.20".into())
        );
    }

    #[test]
    fn a_name_without_a_version_is_not_guessed() {
        assert_eq!(version_from_file_name("postgresql.zip"), None);
        assert_eq!(version_from_file_name("postgresql-13-windows-x64-binaries.zip"), None);
        // ...and an nginx archive is not a PostgreSQL one.
        assert_eq!(version_from_file_name("nginx-1.31.3.zip"), None);
    }
}
