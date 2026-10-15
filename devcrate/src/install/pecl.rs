//! Fetching a PECL extension's DLL for one PHP build.
//!
//! The zip holds the DLL beside its debug symbols, licence, and sample ini;
//! only `php_<name>.dll` is wanted, and it is written to the path the caller
//! names. Putting it into `ext\` and switching it on is [`crate::php_ext`]'s
//! business, which is also where a DLL PHP will not load is taken out again.
//!
//! An extension that ships further DLLs of its own (ImageMagick's, say) needs
//! those beside `php.exe` too. They are not copied: PHP's load check then
//! fails, and the install is refused rather than half-done.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use super::{DOWNLOADS_DIR, archive, download};
use crate::config::Stack;

pub struct FetchedDll {
    pub version: String,
    pub file_name: String,
    /// The zip came from `_downloads\` rather than the network.
    pub cached: bool,
}

/// Download the newest build of `extension` for `build` (`8.5-ts-vs17-x64`),
/// or the release named, and write its DLL to `dest`.
pub fn fetch_dll(
    stack: &Stack,
    extension: &str,
    version: Option<&str>,
    build: &str,
    dest: &Path,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<FetchedDll> {
    let found = download::pecl_build(extension, version, build)?.ok_or_else(|| match version {
        Some(version) => anyhow!("{extension} {version} has no build for PHP {build} on the PECL host"),
        None => anyhow!("no release of {extension} has a build for PHP {build} on the PECL host"),
    })?;

    let downloads = stack.root.join(DOWNLOADS_DIR);
    let fetched = download::fetch(&found.download(), &downloads, progress)?;

    let scratch = downloads.join(format!(".pecl-{extension}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let result = extract_dll(&fetched.path, &scratch, extension, dest);
    let _ = std::fs::remove_dir_all(&scratch);
    result?;

    Ok(FetchedDll { version: found.version, file_name: found.file_name, cached: fetched.cached })
}

fn extract_dll(archive: &Path, scratch: &Path, extension: &str, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(scratch).with_context(|| format!("creating {}", scratch.display()))?;
    archive::extract(archive, scratch, &mut |_, _| {})?;
    let wanted = format!("php_{extension}.dll").to_ascii_lowercase();
    let dll = find(scratch, &wanted)
        .ok_or_else(|| anyhow!("{} holds no {wanted}", archive.display()))?;
    std::fs::copy(&dll, dest)
        .with_context(|| format!("copying {wanted} to {}", dest.display()))?;
    Ok(())
}

/// The file named `wanted` (lowercase) anywhere under `dir`.
fn find(dir: &Path, wanted: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find(&path, wanted) {
                return Some(found);
            }
        } else if entry.file_name().to_string_lossy().to_ascii_lowercase() == wanted {
            return Some(path);
        }
    }
    None
}
