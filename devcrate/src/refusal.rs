//! A refusal that `--force` would override.
//!
//! The CLI prints it like any other error. The dashboard never forces, so it
//! looks for this type -- not for "--force" in the message -- to name the
//! terminal command that can, instead of reporting a plain failure.

use std::fmt;

/// The message names `--force` itself; it is printed as is.
#[derive(Debug)]
pub struct NeedsForce(pub String);

impl fmt::Display for NeedsForce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NeedsForce {}

/// Returns early with a [`NeedsForce`], formatted like `bail!`.
macro_rules! refuse {
    ($($arg:tt)*) => {
        return Err($crate::refusal::NeedsForce(format!($($arg)*)).into())
    };
}
pub(crate) use refuse;

/// Whether `err`, under any context added on the way up, is a refusal that
/// `--force` would override.
pub fn needs_force(err: &anyhow::Error) -> bool {
    err.downcast_ref::<NeedsForce>().is_some()
}

#[cfg(test)]
mod tests {
    use anyhow::{Context, Result};

    use super::*;

    fn refused() -> Result<()> {
        refuse!("{} already exists; pass --force to replace it", "php-8.5");
    }

    #[test]
    fn a_refusal_is_known_by_type_and_reads_as_its_message() {
        let err = refused().context("installing PHP 8.5").unwrap_err();
        assert!(needs_force(&err));
        assert_eq!(format!("{err:#}"), "installing PHP 8.5: php-8.5 already exists; pass --force to replace it");
    }

    #[test]
    fn a_message_that_mentions_force_is_not_a_refusal() {
        let err = anyhow::anyhow!("download failed; retry, --force will not help");
        assert!(!needs_force(&err));
    }
}
