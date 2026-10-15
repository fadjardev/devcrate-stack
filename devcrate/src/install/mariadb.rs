//! MariaDB installer helper for devcrate stack.

use std::fs::File;
use std::io::Write;
use std::path::Path;

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
pub fn configure(dir: &Path) -> Result<Configured> {
    let my_ini = dir.join("my.ini");
    let mut config_created = false;

    if !my_ini.is_file() {
        let content = "\
[mysqld]
port=3306
basedir=.
datadir=./data
bind-address=127.0.0.1
default-storage-engine=InnoDB
innodb_file_per_table=1
";
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

    #[test]
    fn configure_writes_my_ini_and_data_dir_only_once() {
        let dir = unique_temp_dir("mariadb-configure");
        std::fs::create_dir_all(&dir).unwrap();

        let first = configure(&dir).unwrap();
        assert!(first.config_created);
        assert!(first.data_dir_created);
        assert!(dir.join("my.ini").is_file());
        assert!(dir.join("data").is_dir());

        let content = std::fs::read_to_string(dir.join("my.ini")).unwrap();
        assert!(content.contains("port=3306"));

        // A hand-edited my.ini and an already-existing data dir are left alone.
        std::fs::write(dir.join("my.ini"), "[mysqld]\nport=3307\n").unwrap();
        let second = configure(&dir).unwrap();
        assert!(!second.config_created);
        assert!(!second.data_dir_created);
        let unchanged = std::fs::read_to_string(dir.join("my.ini")).unwrap();
        assert_eq!(unchanged, "[mysqld]\nport=3307\n");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
