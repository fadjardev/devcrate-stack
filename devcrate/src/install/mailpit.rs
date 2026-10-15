//! Placing Mailpit: the binary in `mailpit\`, a `data\` for its message store,
//! and every PHP version's `php.ini` pointed at it.
//!
//! Shaped like Redis: one un-versioned directory, a flat zip, `data\` kept
//! across a reinstall. It listens on loopback only -- SMTP on 1025, the inbox
//! UI on 8025 -- and accepts any SMTP login, so an app configured with a
//! username and password (a Laravel `.env`, say) is caught rather than refused.
//!
//! The `php.ini` change is what makes plain `mail()` land in it: on Windows
//! PHP speaks SMTP itself, to `SMTP` / `smtp_port`. It goes in a marked block
//! at the end of the file, the same discipline the hosts file gets, so a
//! hand-edited `php.ini` is never rewritten -- only the block, which a
//! reinstall replaces and an uninstall removes. Being last, its values win
//! over the `[mail function]` defaults earlier in the file.

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::ini_block;
pub use crate::ini_block::IniEdit;

pub const SMTP_PORT: u16 = 1025;
pub const UI_PORT: u16 = 8025;

/// The name in its `php.ini` markers, `; --- devcrate mailpit begin ---`.
const BLOCK: &str = "mailpit";

/// `mailpit-windows-amd64.zip` carries no version; the download is cached
/// under the release's tag instead, `mailpit-1.31.5-windows-amd64.zip`.
pub fn version_from_file_name(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let rest = lower.strip_prefix("mailpit-")?;
    let version = rest.split('-').next()?;
    let dotted = !version.is_empty()
        && version.split('.').all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    dotted.then(|| version.to_string())
}

/// The unpacked archive has the binary at its top.
pub fn check(dir: &Path, archive: &Path) -> Result<()> {
    if !dir.join("mailpit.exe").is_file() {
        bail!(
            "{} does not contain mailpit.exe; it does not look like Mailpit for Windows",
            archive.display()
        );
    }
    Ok(())
}

/// `data\`, for the message store. Returns whether it had to be made.
pub fn ensure_data(dir: &Path) -> Result<bool> {
    let data = dir.join("data");
    if data.is_dir() {
        return Ok(false);
    }
    std::fs::create_dir_all(&data).with_context(|| format!("creating {}", data.display()))?;
    Ok(true)
}

/// The arguments Mailpit is started with, relative to its own directory.
pub fn launch_args(smtp_port: u16, ui_port: u16) -> Vec<String> {
    vec![
        "--listen".into(),
        format!("127.0.0.1:{ui_port}"),
        "--smtp".into(),
        format!("127.0.0.1:{smtp_port}"),
        "--database".into(),
        r"data\mailpit.db".into(),
        "--log-file".into(),
        r"data\mailpit.log".into(),
        "--smtp-auth-accept-any".into(),
        "--smtp-auth-allow-insecure".into(),
        "--disable-version-check".into(),
    ]
}

/// `php_ini` with the devcrate block pointing `mail()` at Mailpit: replaced
/// where there is one, appended where there is not. Everything outside the
/// block is returned as it was.
pub fn with_block(php_ini: &str, smtp_port: u16) -> String {
    let body = [
        "; Written by `devcrate install mailpit`, removed by its uninstall. mail()".to_string(),
        "; goes to Mailpit; its inbox is http://127.0.0.1:8025".to_string(),
        "[mail function]".to_string(),
        "SMTP = 127.0.0.1".to_string(),
        format!("smtp_port = {smtp_port}"),
        "sendmail_from = devcrate@localhost".to_string(),
    ];
    ini_block::with_block(php_ini, BLOCK, &body)
}

/// `php_ini` with the devcrate block taken out, and nothing else changed.
pub fn without_block(php_ini: &str) -> String {
    ini_block::without_block(php_ini, BLOCK)
}

/// Write the block into one `php.ini`, through a temp file and a rename.
pub fn point_php_at(php_ini: &Path, smtp_port: u16) -> IniEdit {
    ini_block::rewrite(php_ini, |text| with_block(text, smtp_port))
}

/// Take the block out of one `php.ini` again.
pub fn unpoint_php(php_ini: &Path) -> IniEdit {
    ini_block::rewrite(php_ini, without_block)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_comes_from_the_cached_name() {
        assert_eq!(version_from_file_name("mailpit-1.31.5-windows-amd64.zip").as_deref(), Some("1.31.5"));
        assert_eq!(version_from_file_name("mailpit-windows-amd64.zip"), None);
    }

    /// Appended once, replaced in place after, removed cleanly -- and the
    /// rest of the file, line endings included, is never touched.
    #[test]
    fn the_block_is_added_replaced_and_removed_without_touching_the_rest() {
        let ini = "[PHP]\r\nmemory_limit = 512M\r\n[mail function]\r\nSMTP = localhost\r\nsmtp_port = 25\r\n";

        let once = with_block(ini, 1025);
        assert!(once.starts_with(ini.trim_end()));
        assert!(once.contains("smtp_port = 1025\r\n"));
        assert!(!once.contains("\r\r") && once.matches('\n').count() == once.matches("\r\n").count());

        let twice = with_block(&once, 1026);
        assert_eq!(twice.matches("; --- devcrate mailpit begin ---").count(), 1, "replaced, not appended again");
        assert!(twice.contains("smtp_port = 1026") && !twice.contains("smtp_port = 1025"));

        assert_eq!(with_block(&once, 1025), once, "rewriting the same block changes nothing");
        assert_eq!(without_block(&once), ini, "removal gives back the original file");
        assert_eq!(without_block(ini), ini, "nothing to remove is no change");
    }

    /// Trailing blank lines are the file's own, and survive the round trip.
    #[test]
    fn a_round_trip_restores_the_file_exactly() {
        for ini in ["a = 1\n", "a = 1\n\n", "a = 1\r\n\r\n\r\n", "a = 1", ""] {
            let back = without_block(&with_block(ini, 1025));
            let expected = if ini == "a = 1" { "a = 1\n" } else { ini };
            assert_eq!(back, expected, "round trip of {ini:?}");
        }
    }

    /// The block's last word wins in PHP's ini parsing, and the section header
    /// keeps it out of a `[PATH=...]` section the file may end with.
    #[test]
    fn the_block_opens_its_own_section() {
        let block = with_block("[PATH=C:/x]\nmemory_limit = 1G\n", 1025);
        let header = block.find("[mail function]").unwrap();
        let smtp = block.find("SMTP = 127.0.0.1").unwrap();
        assert!(block.find("[PATH=").unwrap() < header && header < smtp);
    }

    #[test]
    fn mailpit_listens_on_loopback_only() {
        let args = launch_args(SMTP_PORT, UI_PORT).join(" ");
        assert!(args.contains("--listen 127.0.0.1:8025") && args.contains("--smtp 127.0.0.1:1025"));
        assert!(!args.contains("0.0.0.0") && !args.contains("[::]"));
    }
}
