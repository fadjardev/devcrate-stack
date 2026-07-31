//! Erlang installer helper for devcrate stack.

use std::path::Path;

use anyhow::{Result, bail};

/// Extract version from file name e.g. `otp_win64_27.2.exe` -> `27.2` or `erlang-27.2.zip` -> `27.2`.
pub fn version_from_file_name(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let stem = lower
        .strip_suffix(".zip")
        .or_else(|| lower.strip_suffix(".exe"))
        .unwrap_or(&lower);

    let rest = stem
        .strip_prefix("otp_win64_")
        .or_else(|| stem.strip_prefix("erlang-"))
        .or_else(|| stem.strip_prefix("otp_"))?;

    let version = rest.split('-').next()?;
    if !version.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    Some(version.to_string())
}

/// Verify unpacked directory has Erlang binary (`bin/erl.exe`).
pub fn check(dir: &Path, archive: &Path) -> Result<()> {
    let bin_erl = dir.join("bin").join("erl.exe");
    if !bin_erl.is_file() {
        bail!(
            "{} does not contain bin/erl.exe; it does not look like an Erlang/OTP distribution for Windows",
            archive.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_from_file_name() {
        assert_eq!(
            version_from_file_name("otp_win64_27.2.exe").as_deref(),
            Some("27.2")
        );
        assert_eq!(
            version_from_file_name("erlang-27.2.zip").as_deref(),
            Some("27.2")
        );
        assert_eq!(version_from_file_name("invalid.zip"), None);
    }
}
