//! MariaDB installer helper for devcrate stack.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

pub struct Configured {
    pub config_created: bool,
    pub data_dir_created: bool,
}

/// Extract version from archive file name e.g. `mariadb-11.4.5-winx64.zip` -> `11.4.5`.
pub fn version_from_file_name(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".zip").unwrap_or(&lower);
    let rest = stem.strip_prefix("mariadb-")?;

    let version = rest.split("-winx64").next().unwrap_or(rest);
    let version = version.split('-').next()?;

    if !version.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() < 2 || parts.iter().any(|part| part.is_empty()) {
        return None;
    }

    Some(version.to_string())
}

/// Verify unpacked directory has MariaDB binaries.
pub fn check(dir: &Path, archive: &Path) -> Result<()> {
    let bin_mariadbd = dir.join("bin").join("mariadbd.exe");
    let bin_mysqld = dir.join("bin").join("mysqld.exe");

    if !bin_mariadbd.is_file() && !bin_mysqld.is_file() {
        bail!(
            "{} does not contain bin/mariadbd.exe or bin/mysqld.exe; it does not look like a MariaDB distribution for Windows",
            archive.display()
        );
    }
    Ok(())
}

/// Generate default my.ini and ensure data directory exists.
///
/// `port` is the stack's (`[mariadb] port`, 3306 by default). `start` passes
/// it to mariadbd as well, so the two cannot disagree; writing it here keeps
/// the file honest for anything else that reads it.
pub fn configure(dir: &Path, port: u16) -> Result<Configured> {
    let my_ini = dir.join("my.ini");
    let mut config_created = false;

    if !my_ini.is_file() {
        let content = format!(
            "\
[mysqld]
port={port}
basedir=.
datadir=./data
bind-address=127.0.0.1
default-storage-engine=InnoDB
innodb_file_per_table=1
"
        );
        let mut f = File::create(&my_ini)
            .with_context(|| format!("creating {}", my_ini.display()))?;
        f.write_all(content.as_bytes())?;
        config_created = true;
    }

    let data_dir = dir.join("data");
    let mut data_dir_created = false;
    if !data_dir.is_dir() {
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("creating {}", data_dir.display()))?;
        data_dir_created = true;
    }

    Ok(Configured {
        config_created,
        data_dir_created,
    })
}

/// Create the system tables in `dest\data` if there are none, with the
/// distribution's own `mariadb-install-db` -- MariaDB's `initdb`. The zip ships
/// no data directory, and `mariadbd` exits at once on an empty one.
///
/// Idempotent by what the tool leaves behind: a `data\mysql` directory means a
/// server has lived here, so it is kept and nothing runs. That is what lets
/// `--force` replace the binaries and keep the databases. A `data\` holding
/// other files but no `mysql\` is refused rather than initialised over.
///
/// `root@localhost` comes out with no password, which is what `devcrate db`
/// and the Adminer login expect.
pub fn initialise(dest: &Path, rel: impl Fn(&Path) -> String) -> Result<bool> {
    let data = dest.join("data");
    if data.join("mysql").is_dir() {
        return Ok(false);
    }
    if std::fs::read_dir(&data).is_ok_and(|mut entries| entries.next().is_some()) {
        bail!(
            "{} holds files but no `mysql` system database; not initialising over it",
            rel(&data)
        );
    }

    let tool = ["mariadb-install-db.exe", "mysql_install_db.exe"]
        .iter()
        .map(|name| dest.join("bin").join(name))
        .find(|path| path.is_file());
    let Some(tool) = tool else {
        bail!("{} has neither mariadb-install-db.exe nor mysql_install_db.exe", rel(&dest.join("bin")));
    };

    // Captured, never inherited: the dashboard can run this, and the
    // terminal is the screen it drew.
    let output = Command::new(&tool)
        .arg(format!("--datadir={}", data.display()))
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running {}", rel(&tool)))?;
    if !output.status.success() {
        let detail = [output.stderr, output.stdout]
            .iter()
            .map(|stream| String::from_utf8_lossy(stream).trim().to_string())
            .find(|text| !text.is_empty())
            .unwrap_or_else(|| format!("exit code {}", output.status.code().unwrap_or(-1)));
        bail!("mariadb-install-db failed: {}", detail.lines().last().unwrap_or(&detail));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_from_file_name() {
        assert_eq!(
            version_from_file_name("mariadb-11.4.5-winx64.zip").as_deref(),
            Some("11.4.5")
        );
        assert_eq!(
            version_from_file_name("mariadb-10.11.11-winx64.zip").as_deref(),
            Some("10.11.11")
        );
        assert_eq!(version_from_file_name("invalid.zip"), None);
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
    fn a_directory_is_recognized_by_either_server_binary_name() {
        let dir = unique_temp_dir("mariadb-check-ok");
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin").join("mariadbd.exe"), b"stub").unwrap();

        assert!(check(&dir, Path::new("mariadb-11.4.5-winx64.zip")).is_ok());

        // The older `mysqld.exe` name is accepted too.
        std::fs::remove_file(dir.join("bin").join("mariadbd.exe")).unwrap();
        std::fs::write(dir.join("bin").join("mysqld.exe"), b"stub").unwrap();
        assert!(check(&dir, Path::new("mariadb-10.11.11-winx64.zip")).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_without_either_server_binary_is_rejected() {
        let dir = unique_temp_dir("mariadb-check-bad");
        std::fs::create_dir_all(&dir).unwrap();

        let archive = Path::new("not-mariadb.zip");
        let err = check(&dir, archive).unwrap_err().to_string();
        assert!(err.contains("not-mariadb.zip"));
        assert!(err.contains("bin/mariadbd.exe"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A data directory a server has lived in is kept, and nothing runs --
    /// which is what lets `--force` replace the binaries under the databases.
    #[test]
    fn initialise_keeps_existing_system_tables() {
        let dir = unique_temp_dir("mariadb-init-keep");
        std::fs::create_dir_all(dir.join("data").join("mysql")).unwrap();
        // No bin\ at all: had it tried to run the tool, this would fail.
        assert!(!initialise(&dir, |p| p.display().to_string()).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn initialise_refuses_a_data_directory_that_is_not_mariadbs() {
        let dir = unique_temp_dir("mariadb-init-foreign");
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(dir.join("data").join("notes.txt"), "mine").unwrap();
        let err = initialise(&dir, |p| p.display().to_string()).unwrap_err().to_string();
        assert!(err.contains("no `mysql` system database"), "{err}");
        assert!(dir.join("data").join("notes.txt").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn initialise_names_the_missing_tool() {
        let dir = unique_temp_dir("mariadb-init-notool");
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let err = initialise(&dir, |p| p.display().to_string()).unwrap_err().to_string();
        assert!(err.contains("mariadb-install-db.exe"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn configure_writes_my_ini_and_data_dir_only_once() {
        let dir = unique_temp_dir("mariadb-configure");
        std::fs::create_dir_all(&dir).unwrap();

        let first = configure(&dir, 3399).unwrap();
        assert!(first.config_created);
        assert!(first.data_dir_created);
        assert!(dir.join("my.ini").is_file());
        assert!(dir.join("data").is_dir());

        let content = std::fs::read_to_string(dir.join("my.ini")).unwrap();
        assert!(content.contains("port=3399"), "the stack's port: {content}");

        // A hand-edited my.ini and an already-existing data dir are left alone.
        std::fs::write(dir.join("my.ini"), "[mysqld]\nport=3307\n").unwrap();
        let second = configure(&dir, 3399).unwrap();
        assert!(!second.config_created);
        assert!(!second.data_dir_created);
        let unchanged = std::fs::read_to_string(dir.join("my.ini")).unwrap();
        assert_eq!(unchanged, "[mysqld]\nport=3307\n");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
