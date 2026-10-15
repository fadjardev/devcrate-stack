//! Turning an unpacked Redis-for-Windows archive into a service the stack can
//! serve.
//!
//! Shaped like [`super::mariadb`], not [`super::postgres`]: one directory,
//! not versioned, with `data\` created beside the binaries and a config file
//! seeded on first install and left alone after. There is no `initdb`
//! equivalent -- `redis-server.exe` starts against an empty `data\` with
//! nothing to prepare first -- so there is no separate "initialise" step the
//! way PostgreSQL needs one.
//!
//! The build itself comes from `redis-windows/redis-windows` on GitHub
//! (`Redis-<version>-Windows-x64-msys2.zip`), a community compile of the
//! official Redis source for Windows; there is no vendor-published checksum
//! for it, so the download is length/TLS-checked like nginx and PostgreSQL --
//! see `download::Download::sha256`.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};

pub struct Configured {
    pub conf_created: bool,
    pub data_dir_created: bool,
}

/// `Redis-8.10.2-Windows-x64-msys2.zip` -> `8.10.2`.
pub fn version_from_file_name(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".zip").unwrap_or(&lower);
    let rest = stem.strip_prefix("redis-")?;
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

/// Is this unpacked directory a Redis-for-Windows build? Both the server and
/// the CLI matter: the server is what the stack runs, the CLI is what its own
/// graceful shutdown uses (see `control::ask_nicely`).
pub fn check(dir: &Path, archive: &Path) -> Result<()> {
    for required in ["redis-server.exe", "redis-cli.exe"] {
        if !dir.join(required).is_file() {
            bail!(
                "{} does not contain {required}; it does not look like a \
                 Redis-for-Windows build.\n\
                 The zip to install is the plain msys2 build \
                 (Redis-<version>-Windows-x64-msys2.zip), not the \
                 \"-with-Service\" or cygwin variant.",
                archive.display()
            );
        }
    }
    Ok(())
}

/// Generate a default `devcrate.conf` if there is not one already, and ensure
/// `data\` exists. Both left alone on a reinstall, the same way MariaDB's
/// `my.ini` and `data\` are -- a hand-edited conf survives `install --force`.
///
/// Named `devcrate.conf`, not `redis.conf`: the vendor's zip already ships a
/// full stock `redis.conf` (its own documented example, hundreds of lines) at
/// the top level, so writing under that name would either clobber a file this
/// stack does not own or, worse, silently do nothing on every install because
/// [`redis::configure`](configure) only writes when the file is absent --
/// which is exactly the bug an earlier version of this function had: the
/// vendor's `redis.conf` was already there, so this function's own port/dir/
/// logfile settings never took effect and the server ran on the vendor's
/// defaults instead (`dir ./`, writing `dump.rdb` into the redis root rather
/// than `data\`). A name this stack owns outright has no such collision.
///
/// `dir` and `logfile` are written as relative paths (`./data`,
/// `redis.log`), not absolute ones. This install runs inside a staging
/// directory that is renamed into place afterward (see `swap_into_place`), so
/// an absolute path baked in here would name a location that no longer
/// exists once the swap moves it -- an earlier version of this function had
/// exactly that bug. A relative path sidesteps it entirely: `redis-server.exe`
/// is launched with the *final* directory as its working directory (see
/// `control::launch`), so it resolves correctly regardless of where the
/// install happened to stage.
///
/// `redis-server.exe` `chdir`s into `dir` early at startup, before it opens
/// `logfile` -- so the log actually lands at `data\redis.log`, not beside the
/// binaries, whatever this function's own working directory was when it ran.
/// `tui::logs::discover` looks for it there.
pub fn configure(dir: &Path, port: u16) -> Result<Configured> {
    let conf = dir.join("devcrate.conf");
    let mut conf_created = false;

    if !conf.is_file() {
        let content = format!(
            "port {port}\n\
             bind 127.0.0.1\n\
             protected-mode yes\n\
             daemonize no\n\
             dir ./data\n\
             logfile \"redis.log\"\n\
             save 900 1\n\
             save 300 10\n\
             save 60 10000\n"
        );
        let mut f =
            File::create(&conf).with_context(|| format!("creating {}", conf.display()))?;
        f.write_all(content.as_bytes())?;
        conf_created = true;
    }

    let data_dir = dir.join("data");
    let mut data_dir_created = false;
    if !data_dir.is_dir() {
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("creating {}", data_dir.display()))?;
        data_dir_created = true;
    }

    Ok(Configured { conf_created, data_dir_created })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_is_read_from_the_vendors_file_name() {
        assert_eq!(
            version_from_file_name("Redis-8.10.2-Windows-x64-msys2.zip"),
            Some("8.10.2".into())
        );
        assert_eq!(
            version_from_file_name("redis-8.8.3-windows-x64-msys2.zip"),
            Some("8.8.3".into())
        );
    }

    #[test]
    fn a_name_without_a_version_is_not_guessed() {
        assert_eq!(version_from_file_name("redis.zip"), None);
        assert_eq!(version_from_file_name("redis-windows-x64-msys2.zip"), None);
        // ...and a MariaDB archive is not a Redis one.
        assert_eq!(version_from_file_name("mariadb-11.4.5-winx64.zip"), None);
    }

    fn unique_temp_dir(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "devcrate-test-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn a_directory_with_both_executables_passes() {
        let dir = unique_temp_dir("redis-check-ok");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("redis-server.exe"), b"stub").unwrap();
        std::fs::write(dir.join("redis-cli.exe"), b"stub").unwrap();

        assert!(check(&dir, Path::new("Redis-8.10.2-Windows-x64-msys2.zip")).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_missing_either_executable_is_rejected() {
        let dir = unique_temp_dir("redis-check-bad");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("redis-server.exe"), b"stub").unwrap();

        let archive = Path::new("Redis-8.10.2-Windows-x64-msys2.zip");
        let err = check(&dir, archive).unwrap_err().to_string();
        assert!(err.contains("redis-cli.exe"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn configure_writes_the_conf_and_data_dir_only_once() {
        let dir = unique_temp_dir("redis-configure");
        std::fs::create_dir_all(&dir).unwrap();

        let first = configure(&dir, 6379).unwrap();
        assert!(first.conf_created);
        assert!(first.data_dir_created);
        assert!(dir.join("devcrate.conf").is_file());
        assert!(dir.join("data").is_dir());

        let content = std::fs::read_to_string(dir.join("devcrate.conf")).unwrap();
        assert!(content.contains("port 6379"));
        assert!(content.contains("bind 127.0.0.1"));
        // Relative, not baked from `dir` -- the staging directory this ran in
        // is renamed away before the server ever starts.
        assert!(content.contains("dir ./data"));

        // A hand-edited conf and an already-existing data dir are left alone.
        std::fs::write(dir.join("devcrate.conf"), "port 6380\n").unwrap();
        let second = configure(&dir, 6379).unwrap();
        assert!(!second.conf_created);
        assert!(!second.data_dir_created);
        let unchanged = std::fs::read_to_string(dir.join("devcrate.conf")).unwrap();
        assert_eq!(unchanged, "port 6380\n");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
