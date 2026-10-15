//! Placing mkcert: one `mkcert.exe` in the stack root, where
//! [`crate::mkcert::find_mkcert`] looks first.
//!
//! The binary is the whole of it -- mkcert keeps its CA under the user's
//! profile, not beside itself. Its GitHub releases carry no digest (they
//! predate GitHub recording one), so the download is length-checked over TLS,
//! like nginx's, and the caller is told so.

use anyhow::{Context, Result, anyhow};

use super::{DOWNLOADS_DIR, download};
use crate::config::Stack;

pub struct Placed {
    pub version: String,
    /// Root-relative: `mkcert.exe`.
    pub path: String,
    pub cached: bool,
    pub verified: bool,
}

/// Download the newest mkcert for Windows x64 into `mkcert.exe`, through a
/// temporary name, so a failed copy never leaves half a binary where the
/// stack looks for one.
pub fn place(stack: &Stack, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<Placed> {
    let release = download::mkcert_catalogue()?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("mkcert's releases list no Windows x64 build"))?;
    let fetched = download::fetch(&release.download(), &stack.root.join(DOWNLOADS_DIR), progress)?;

    let dest = stack.root.join("mkcert.exe");
    let temp = stack.root.join("mkcert.exe.devcrate-new");
    std::fs::copy(&fetched.path, &temp).with_context(|| format!("writing {}", temp.display()))?;
    if let Err(err) = std::fs::rename(&temp, &dest) {
        let _ = std::fs::remove_file(&temp);
        return Err(err).with_context(|| format!("putting {} in place", dest.display()));
    }
    Ok(Placed {
        version: release.version,
        path: stack.rel(&dest),
        cached: fetched.cached,
        verified: fetched.verified,
    })
}
