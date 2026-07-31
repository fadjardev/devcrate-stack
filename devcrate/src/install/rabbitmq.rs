//! RabbitMQ installer helper for devcrate stack.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};

pub struct Configured {
    pub data_dir_created: bool,
    pub plugins_file_created: bool,
}

/// Extract version from archive file name e.g. `rabbitmq-server-windows-4.0.5.zip` -> `4.0.5`.
pub fn version_from_file_name(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".zip").unwrap_or(&lower);
    let rest = stem
        .strip_prefix("rabbitmq-server-windows-")
        .or_else(|| stem.strip_prefix("rabbitmq-server-"))?;

    let version = rest.split('-').next()?;
    if !version.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    Some(version.to_string())
}

/// Verify unpacked directory has RabbitMQ binaries.
pub fn check(dir: &Path, archive: &Path) -> Result<()> {
    let sbin_server = dir.join("sbin").join("rabbitmq-server.bat");
    if !sbin_server.is_file() {
        bail!(
            "{} does not contain sbin/rabbitmq-server.bat; it does not look like a RabbitMQ distribution for Windows",
            archive.display()
        );
    }
    Ok(())
}

/// Generate data directory and enable management plugin.
pub fn configure(dir: &Path) -> Result<Configured> {
    let data_dir = dir.join("data");
    let mut data_dir_created = false;
    if !data_dir.is_dir() {
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("creating {}", data_dir.display()))?;
        data_dir_created = true;
    }

    let plugins_file = data_dir.join("enabled_plugins");
    let mut plugins_file_created = false;
    if !plugins_file.is_file() {
        let mut f = File::create(&plugins_file)
            .with_context(|| format!("creating {}", plugins_file.display()))?;
        f.write_all(b"[rabbitmq_management].\n")?;
        plugins_file_created = true;
    }

    Ok(Configured {
        data_dir_created,
        plugins_file_created,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_from_file_name() {
        assert_eq!(
            version_from_file_name("rabbitmq-server-windows-4.0.5.zip").as_deref(),
            Some("4.0.5")
        );
        assert_eq!(
            version_from_file_name("rabbitmq-server-4.0.5.zip").as_deref(),
            Some("4.0.5")
        );
        assert_eq!(version_from_file_name("invalid.zip"), None);
    }
}
