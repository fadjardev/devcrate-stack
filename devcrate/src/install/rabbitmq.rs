//! RabbitMQ installer helper for devcrate stack.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};

pub struct Configured {
    pub data_dir_created: bool,
    pub plugins_file_created: bool,
    /// `data\rabbitmq.conf` was written now (it binds to loopback).
    pub conf_created: bool,
}

/// RabbitMQ's own defaults listen on every interface. Like Redis and Mailpit,
/// the stack's broker is for this machine only, so AMQP and the management UI
/// are bound to loopback. The node's Erlang distribution port and epmd are
/// left as RabbitMQ sets them.
pub const LOOPBACK_CONF: &str = "\
# Written by devcrate: the broker answers this machine only.
listeners.tcp.default = 127.0.0.1:5672
management.tcp.ip = 127.0.0.1
";

/// Write `data\rabbitmq.conf` -- where `RABBITMQ_BASE` makes RabbitMQ look --
/// unless one is there already, which is then left exactly as it is. True when
/// it was written.
pub fn ensure_conf(data_dir: &Path) -> Result<bool> {
    let conf = data_dir.join("rabbitmq.conf");
    if conf.exists() {
        return Ok(false);
    }
    std::fs::create_dir_all(data_dir).with_context(|| format!("creating {}", data_dir.display()))?;
    std::fs::write(&conf, LOOPBACK_CONF).with_context(|| format!("writing {}", conf.display()))?;
    Ok(true)
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

    let conf_created = ensure_conf(&data_dir)?;

    Ok(Configured {
        data_dir_created,
        plugins_file_created,
        conf_created,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The loopback conf is written once and never over somebody's own.
    #[test]
    fn the_loopback_conf_is_written_once_and_never_over_one_there() {
        let data = std::env::temp_dir().join(format!("devcrate-rabbit-conf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);

        assert!(ensure_conf(&data).unwrap());
        let text = std::fs::read_to_string(data.join("rabbitmq.conf")).unwrap();
        assert!(text.contains("listeners.tcp.default = 127.0.0.1:5672"), "{text}");
        assert!(text.contains("management.tcp.ip = 127.0.0.1"), "{text}");

        std::fs::write(data.join("rabbitmq.conf"), "listeners.tcp.default = 5672\n").unwrap();
        assert!(!ensure_conf(&data).unwrap());
        assert_eq!(std::fs::read_to_string(data.join("rabbitmq.conf")).unwrap(), "listeners.tcp.default = 5672\n");
        let _ = std::fs::remove_dir_all(&data);
    }

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
