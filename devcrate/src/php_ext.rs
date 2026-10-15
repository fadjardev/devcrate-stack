//! `devcrate php ext` -- turning a PHP version's extensions on and off.
//!
//! An extension is on when an `extension=` (or `zend_extension=`) line names
//! it. devcrate adds such lines only inside its own marked block at the end of
//! the version's `php.ini` (see [`crate::ini_block`]), so `disable` can take
//! out what `enable` put in and nothing else. A line the file already had is
//! reported, not edited: disabling that one is a hand edit, and the command
//! says which line.
//!
//! Every enable is checked by running that version's `php -m` against the new
//! file. A DLL that will not load -- a missing dependency, a build for another
//! PHP -- shows up there as a startup warning, and the edit is undone rather
//! than left to break the worker. The version's FastCGI worker is restarted
//! afterwards, if it was running, so the change is live.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

use crate::config::{Service, Stack};
use crate::ini_block::{self, IniEdit};
use crate::probe::ProcessTable;
use crate::{control, exit, php};

/// The name in the markers, `; --- devcrate extensions begin ---`.
pub const BLOCK: &str = "extensions";

/// Extensions PHP loads with `zend_extension=` rather than `extension=`.
const ZEND: [&str; 2] = ["opcache", "xdebug"];

/// Where an extension is switched on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A line of the file's own, 1-based -- not devcrate's to change.
    Ini(usize),
    /// devcrate's block.
    Block,
}

#[derive(Debug, Clone)]
pub struct Extension {
    pub name: String,
    pub enabled: Option<Source>,
    /// A `php_<name>.dll` is in the extension directory.
    pub bundled: bool,
}

pub struct Listing {
    pub php_name: String,
    pub ini: String,
    pub extensions: Vec<Extension>,
}

/// What an enable or disable came to.
pub struct Changed {
    pub php_id: String,
    pub php_name: String,
    pub ini: String,
    /// Names whose state changed, and those that were already as asked.
    pub changed: Vec<String>,
    pub unchanged: Vec<String>,
    pub restart: Restart,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Restart {
    /// Nothing was written, so nothing needed restarting.
    NotNeeded,
    /// The worker was not running; it reads the file when it next starts.
    NotRunning,
    Done,
    Failed(String),
}

/// `curl`, `php_curl.dll`, `"curl"`, `ext\php_curl.dll` -> `curl`.
pub fn name_of(value: &str) -> String {
    let value = value.trim().trim_matches('"').trim_matches('\'');
    let file = value.rsplit(['\\', '/']).next().unwrap_or(value).to_ascii_lowercase();
    let file = file.strip_suffix(".dll").unwrap_or(&file);
    file.strip_prefix("php_").unwrap_or(file).to_string()
}

/// Every active `extension=` / `zend_extension=` line: 0-based index and name.
fn directives(text: &str) -> Vec<(usize, String)> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line = line.trim();
            if line.starts_with(';') || line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            let key = key.trim().to_ascii_lowercase();
            if key != "extension" && key != "zend_extension" {
                return None;
            }
            // A trailing comment: `extension=php_exif.dll ; after mbstring`.
            let value = value.split(';').next().unwrap_or("").trim();
            (!value.is_empty()).then(|| (index, name_of(value)))
        })
        .collect()
}

/// The extensions a `php.ini`'s text switches on, in file order.
pub fn enabled_in(text: &str) -> Vec<String> {
    directives(text).into_iter().map(|(_, name)| name).collect()
}

fn source_of(text: &str, name: &str) -> Option<Source> {
    directives(text).into_iter().find(|(_, n)| n == name).map(|(index, _)| {
        if ini_block::inside(text, BLOCK, index) { Source::Block } else { Source::Ini(index + 1) }
    })
}

/// The line written for `name`: the full file name, which every PHP from 5.x
/// on accepts (a bare `pgsql` needs 7.2).
fn directive(name: &str) -> String {
    let key = if ZEND.contains(&name) { "zend_extension" } else { "extension" };
    format!("{key}=php_{name}.dll")
}

/// `text` with devcrate's block holding exactly `names`, or without a block
/// at all when there are none.
fn with_names(text: &str, names: &[String]) -> String {
    if names.is_empty() {
        return ini_block::without_block(text, BLOCK);
    }
    let mut body = vec![
        "; Written by `devcrate php ext enable`; `devcrate php ext disable` takes lines out.".to_string(),
        // Its own section, so a file ending in [PATH=...] cannot swallow it.
        "[PHP]".to_string(),
    ];
    body.extend(names.iter().map(|name| directive(name)));
    ini_block::with_block(text, BLOCK, &body)
}

/// The names devcrate's block currently switches on.
fn block_names(text: &str) -> Vec<String> {
    ini_block::body(text, BLOCK)
        .map(|body| directives(&body.join("\n")).into_iter().map(|(_, name)| name).collect())
        .unwrap_or_default()
}

struct Target<'a> {
    service: &'a Service,
    dir: PathBuf,
    ini: PathBuf,
}

/// `--php`, or the version `php\current` names.
fn target<'a>(stack: &'a Stack, wanted: Option<&str>) -> Result<Target<'a>> {
    let service = match wanted {
        Some(wanted) => php::find(stack, wanted)?,
        None => {
            let current = stack
                .current_php()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .ok_or_else(|| anyhow!("no --php given and php\\current is not set; pass --php 8.5"))?;
            php::find(stack, &current)?
        }
    };
    let dir = service
        .install_marker
        .parent()
        .ok_or_else(|| anyhow!("{} has no directory", service.install_marker.display()))?
        .to_path_buf();
    let ini = dir.join("php.ini");
    if !ini.is_file() {
        bail!("{} not found", stack.rel(&ini));
    }
    Ok(Target { service, dir, ini })
}

/// The extension directory: `extension_dir` from the file, relative to the
/// version's folder (where the workers are started), `ext` by default.
fn ext_dir(dir: &Path, text: &str) -> PathBuf {
    let set = text.lines().rev().find_map(|line| {
        let (key, value) = line.trim().split_once('=')?;
        (key.trim().eq_ignore_ascii_case("extension_dir"))
            .then(|| value.split(';').next().unwrap_or("").trim().trim_matches('"').to_string())
    });
    match set.filter(|s| !s.is_empty()) {
        Some(set) if Path::new(&set).is_absolute() => PathBuf::from(set),
        Some(set) => dir.join(set),
        None => dir.join("ext"),
    }
}

fn bundled(ext_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(ext_dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .filter_map(|entry| {
            let file = entry.ok()?.file_name().to_string_lossy().to_ascii_lowercase();
            (file.starts_with("php_") && file.ends_with(".dll")).then(|| name_of(&file))
        })
        .collect();
    names.sort();
    names
}

pub fn list(stack: &Stack, wanted: Option<&str>) -> Result<Listing> {
    let target = target(stack, wanted)?;
    let text = std::fs::read_to_string(&target.ini)
        .with_context(|| format!("reading {}", target.ini.display()))?;
    let bundled = bundled(&ext_dir(&target.dir, &text));

    let mut names = bundled.clone();
    names.extend(directives(&text).into_iter().map(|(_, name)| name));
    names.sort();
    names.dedup();

    let extensions = names
        .into_iter()
        .map(|name| Extension {
            enabled: source_of(&text, &name),
            bundled: bundled.contains(&name),
            name,
        })
        .collect();
    Ok(Listing {
        php_name: target.service.name.clone(),
        ini: stack.rel(&target.ini),
        extensions,
    })
}

pub fn enable(stack: &Stack, wanted: Option<&str>, exts: &[String]) -> Result<Changed> {
    let target = target(stack, wanted)?;
    let original = std::fs::read_to_string(&target.ini)
        .with_context(|| format!("reading {}", target.ini.display()))?;
    let ext_dir = ext_dir(&target.dir, &original);

    let mut names = block_names(&original);
    let (mut changed, mut unchanged) = (Vec::new(), Vec::new());
    for ext in exts {
        let name = name_of(ext);
        if source_of(&original, &name).is_some() || names.contains(&name) {
            unchanged.push(name);
            continue;
        }
        if !ext_dir.join(format!("php_{name}.dll")).is_file() {
            // Compiled in, like OPcache in 8.5: always on, nothing to write.
            if loaded_modules(&target.dir).iter().any(|m| module_is(m, &name)) {
                unchanged.push(name);
                continue;
            }
            let have = bundled(&ext_dir);
            bail!(
                "{} has no php_{name}.dll in {}; bundled: {}",
                target.service.name,
                stack.rel(&ext_dir),
                if have.is_empty() { "none".to_string() } else { have.join(", ") }
            );
        }
        names.push(name.clone());
        changed.push(name);
    }

    let mut done = Changed {
        php_id: target.service.id.clone(),
        php_name: target.service.name.clone(),
        ini: stack.rel(&target.ini),
        changed,
        unchanged,
        restart: Restart::NotNeeded,
    };
    if done.changed.is_empty() {
        return Ok(done);
    }

    names.sort();
    names.dedup();
    write(&target.ini, |text| with_names(text, &names))?;

    // The proof: the new file loads. If it does not, put the old one back
    // rather than leave a worker that will not start.
    if let Some(problem) = startup_problem(&target.dir) {
        let restored = ini_block::rewrite(&target.ini, |_| original.clone());
        let note = match restored {
            IniEdit::Failed(why) => format!("; restoring the file failed too: {why}"),
            _ => String::new(),
        };
        bail!("{} would not load it, so the change was undone{note}:\n{problem}", target.service.name);
    }

    done.restart = restart_if_running(stack, target.service);
    Ok(done)
}

pub fn disable(stack: &Stack, wanted: Option<&str>, exts: &[String]) -> Result<Changed> {
    let target = target(stack, wanted)?;
    let text = std::fs::read_to_string(&target.ini)
        .with_context(|| format!("reading {}", target.ini.display()))?;

    let mut names = block_names(&text);
    let (mut changed, mut unchanged) = (Vec::new(), Vec::new());
    for ext in exts {
        let name = name_of(ext);
        match source_of(&text, &name) {
            Some(Source::Ini(line)) => bail!(
                "{name} is switched on at line {line} of {}, outside devcrate's block. \
                 devcrate does not edit the rest of the file -- put a `;` in front of that line to turn it off.",
                stack.rel(&target.ini)
            ),
            Some(Source::Block) => {
                names.retain(|n| n != &name);
                changed.push(name);
            }
            None => unchanged.push(name),
        }
    }

    let mut done = Changed {
        php_id: target.service.id.clone(),
        php_name: target.service.name.clone(),
        ini: stack.rel(&target.ini),
        changed,
        unchanged,
        restart: Restart::NotNeeded,
    };
    if done.changed.is_empty() {
        return Ok(done);
    }
    write(&target.ini, |text| with_names(text, &names))?;
    done.restart = restart_if_running(stack, target.service);
    Ok(done)
}

/// What `php ext install` came to.
pub struct Installed {
    pub php_id: String,
    pub php_name: String,
    pub extension: String,
    pub version: String,
    pub file_name: String,
    /// The build it was matched to: `8.5-ts-vs17-x64`.
    pub build: String,
    pub dll: String,
    pub cached: bool,
    pub replaced: bool,
    pub restart: Restart,
}

/// The build a PECL DLL has to match, as the PECL host spells it in a file
/// name -- `8.5-ts-vs17-x64` -- read from a version's `php -i`.
pub fn parse_build(php_i: &str) -> Option<String> {
    let field = |key: &str| {
        php_i.lines().find_map(|line| {
            let (k, v) = line.split_once("=>")?;
            (k.trim() == key).then(|| v.trim().to_string())
        })
    };
    let version = field("PHP Version")?;
    let mut parts = version.split('.');
    let minor = format!("{}.{}", parts.next()?, parts.next()?);
    // `API20250925,TS,VS17`
    let build = field("PHP Extension Build")?;
    let mut build = build.split(',').skip(1);
    let thread = build.next()?.to_ascii_lowercase();
    let toolchain = build.next()?.to_ascii_lowercase();
    let arch = match field("Architecture")?.as_str() {
        "x64" => "x64",
        "x86" => "x86",
        _ => return None,
    };
    Some(format!("{minor}-{thread}-{toolchain}-{arch}"))
}

fn build_of(dir: &Path) -> Result<String> {
    let php = dir.join("php.exe");
    let output = Command::new(&php)
        .arg("-i")
        .current_dir(dir)
        .output()
        .with_context(|| format!("running {} -i", php.display()))?;
    parse_build(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| anyhow!("{} -i did not say which build it is", php.display()))
}

/// Download a PECL extension built for this exact PHP, put its DLL in `ext\`,
/// and switch it on. A DLL PHP will not load -- the wrong build, a missing
/// dependency -- is taken out again and the old one, if any, put back.
pub fn install(
    stack: &Stack,
    wanted: Option<&str>,
    extension: &str,
    version: Option<&str>,
    force: bool,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<Installed> {
    let target = target(stack, wanted)?;
    let name = name_of(extension);
    let text = std::fs::read_to_string(&target.ini)
        .with_context(|| format!("reading {}", target.ini.display()))?;
    let dll = ext_dir(&target.dir, &text).join(format!("php_{name}.dll"));
    let existed = dll.is_file();
    if existed && !force {
        bail!(
            "{} already has {}. `devcrate php ext enable {name}` switches it on; --force replaces it.",
            target.service.name,
            stack.rel(&dll)
        );
    }

    let build = build_of(&target.dir)?;
    let new = dll.with_extension("dll.devcrate-new");
    let fetched = crate::install::pecl::fetch_dll(stack, &name, version, &build, &new, progress);
    let fetched = match fetched {
        Ok(fetched) => fetched,
        Err(err) => {
            let _ = std::fs::remove_file(&new);
            return Err(err);
        }
    };

    // The old DLL is renamed rather than deleted: Windows allows that while a
    // worker still has it loaded, and it is what goes back if this fails.
    let old = dll.with_extension("dll.devcrate-old");
    if existed {
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&dll, &old).with_context(|| format!("moving {} aside", dll.display()))?;
    }
    if let Err(err) = std::fs::rename(&new, &dll) {
        let _ = std::fs::remove_file(&new);
        if existed {
            let _ = std::fs::rename(&old, &dll);
        }
        return Err(err).with_context(|| format!("putting {} in place", dll.display()));
    }
    let undo = || {
        let _ = std::fs::remove_file(&dll);
        if existed {
            let _ = std::fs::rename(&old, &dll);
        }
    };

    // Switching it on runs `php -m` against it, which is the build check: PHP
    // names the mismatch and refuses to load it.
    let restart = match enable(stack, Some(&target.service.id), std::slice::from_ref(&name)) {
        Err(err) => {
            undo();
            bail!("{err:#}\nThe downloaded DLL was removed again.");
        }
        // Already switched on, so `enable` checked nothing: a reinstall.
        Ok(done) if done.changed.is_empty() => {
            if let Some(problem) = startup_problem(&target.dir) {
                undo();
                bail!(
                    "{} would not load it, so the old DLL was put back:\n{problem}",
                    target.service.name
                );
            }
            restart_if_running(stack, target.service)
        }
        Ok(done) => done.restart,
    };
    if existed {
        let _ = std::fs::remove_file(&old);
    }

    Ok(Installed {
        php_id: target.service.id.clone(),
        php_name: target.service.name.clone(),
        extension: name,
        version: fetched.version,
        file_name: fetched.file_name,
        build,
        dll: stack.rel(&dll),
        cached: fetched.cached,
        replaced: existed,
        restart,
    })
}

fn write(ini: &Path, edit: impl Fn(&str) -> String) -> Result<()> {
    match ini_block::rewrite(ini, edit) {
        IniEdit::Failed(why) => Err(anyhow!(why)),
        IniEdit::Written | IniEdit::AlreadyThere => Ok(()),
    }
}

/// `php -m` for one version, run from its folder so a relative
/// `extension_dir` resolves as it does for the worker.
fn php_m(dir: &Path) -> Option<std::process::Output> {
    Command::new(dir.join("php.exe")).arg("-m").current_dir(dir).output().ok()
}

fn loaded_modules(dir: &Path) -> Vec<String> {
    let Some(output) = php_m(dir) else { return Vec::new() };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('['))
        .map(str::to_string)
        .collect()
}

/// Whether a `php -m` line is the extension `name`: it says `Zend OPcache`
/// for `opcache`.
fn module_is(module: &str, name: &str) -> bool {
    let module = module.trim().to_ascii_lowercase();
    module == name || module.strip_prefix("zend ") == Some(name)
}

/// The startup warnings `php -m` prints when an extension will not load, or
/// `None` when it starts clean.
fn startup_problem(dir: &Path) -> Option<String> {
    let Some(output) = php_m(dir) else {
        return Some(format!("{} could not be run", dir.join("php.exe").display()));
    };
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let problems: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("PHP Startup") || line.contains("Failed loading"))
        .collect();
    if !problems.is_empty() {
        return Some(problems.join("\n"));
    }
    (!output.status.success()).then(|| format!("php -m exited {}", output.status.code().unwrap_or(-1)))
}

fn restart_if_running(stack: &Stack, service: &Service) -> Restart {
    let table = ProcessTable::scan();
    if table.matching(&service.process_prefix, &service.exclude_names).is_empty() {
        return Restart::NotRunning;
    }
    let stopped = match control::run_stop(stack, Some(&service.id)) {
        Ok(steps) => steps,
        Err(err) => return Restart::Failed(format!("{err:#}")),
    };
    if let Some(step) = stopped.iter().find(|step| step.outcome.is_failure()) {
        return Restart::Failed(control::describe(&step.outcome, step.helpers));
    }
    match control::run_start(stack, Some(&service.id)) {
        Ok(steps) => match steps.iter().find(|step| step.outcome.is_failure()) {
            Some(step) => Restart::Failed(control::describe_start(service.kind, &step.outcome)),
            None => Restart::Done,
        },
        Err(err) => Restart::Failed(format!("{err:#}")),
    }
}

// ---------------------------------------------------------------------------
// The command line
// ---------------------------------------------------------------------------

pub fn print_list(stack: &Stack, wanted: Option<&str>) -> Result<u8> {
    let listing = list(stack, wanted)?;
    println!("{} ({})", listing.php_name, listing.ini);
    for ext in &listing.extensions {
        let (mark, mut note) = match &ext.enabled {
            Some(Source::Ini(line)) => ("on ", format!("php.ini line {line}")),
            Some(Source::Block) => ("on ", "devcrate".to_string()),
            None => ("off", String::new()),
        };
        if !ext.bundled {
            note.push_str(&format!("; no php_{}.dll in ext", ext.name));
        }
        println!("  {mark}  {:<14} {note}", ext.name);
    }
    Ok(exit::OK)
}

pub fn print_change(stack: &Stack, wanted: Option<&str>, exts: &[String], on: bool) -> Result<u8> {
    let done = if on { enable(stack, wanted, exts)? } else { disable(stack, wanted, exts)? };
    let verb = if on { "enabled" } else { "disabled" };
    for name in &done.changed {
        println!("{} {name} {verb} in {}", done.php_name, done.ini);
    }
    for name in &done.unchanged {
        println!("{} {name} was already {}", done.php_name, if on { "on" } else { "off" });
    }
    match &done.restart {
        Restart::NotNeeded => {}
        Restart::NotRunning => println!("  {} is not running; it reads the file when it starts", done.php_id),
        Restart::Done => println!("  {} restarted", done.php_id),
        Restart::Failed(why) => {
            println!("  {} did not restart cleanly: {why}", done.php_id);
            return Ok(exit::ERROR);
        }
    }
    Ok(exit::OK)
}

pub fn print_install(
    stack: &Stack,
    wanted: Option<&str>,
    extension: &str,
    version: Option<&str>,
    force: bool,
) -> Result<u8> {
    let mut said = false;
    let mut progress = |_: u64, _: Option<u64>| {
        if !said {
            said = true;
            println!("Downloading {extension} from the PECL host...");
        }
    };
    let done = install(stack, wanted, extension, version, force, &mut progress)?;
    let verb = if done.replaced { "replaced" } else { "installed" };
    println!("{} {} {} {verb}: {}", done.php_name, done.extension, done.version, done.dll);
    let source = if done.cached { "cached in _downloads" } else { "downloaded" };
    println!("  {} ({source}), built for {}", done.file_name, done.build);
    println!("  length-checked over TLS only -- the PECL host publishes no checksum");
    println!("  PHP loaded it, and it is switched on in devcrate's block of php.ini");
    match &done.restart {
        Restart::NotNeeded => {}
        Restart::NotRunning => println!("  {} is not running; it loads it when it starts", done.php_id),
        Restart::Done => println!("  {} restarted", done.php_id),
        Restart::Failed(why) => {
            println!("  {} did not restart cleanly: {why}", done.php_id);
            return Ok(exit::ERROR);
        }
    }
    Ok(exit::OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spelling_names_the_same_extension() {
        for spelling in ["pgsql", "php_pgsql.dll", "\"pgsql\"", "ext\\php_pgsql.dll", "PHP_PGSQL.DLL"] {
            assert_eq!(name_of(spelling), "pgsql", "{spelling}");
        }
    }

    #[test]
    fn active_lines_are_found_and_comments_are_not() {
        let ini = "[PHP]\n;extension=bz2\nextension = curl\nextension=php_exif.dll      ; after mbstring\nzend_extension=opcache\n";
        let names: Vec<String> = directives(ini).into_iter().map(|(_, n)| n).collect();
        assert_eq!(names, ["curl", "exif", "opcache"]);
        assert_eq!(source_of(ini, "curl"), Some(Source::Ini(3)));
        assert_eq!(source_of(ini, "bz2"), None);
    }

    /// Enabling writes only the block; disabling the last one takes the block
    /// away and leaves the file as it was.
    #[test]
    fn the_block_holds_exactly_what_was_enabled() {
        let ini = "[PHP]\nextension=curl\n";
        let on = with_names(ini, &["pdo_pgsql".into(), "pgsql".into()]);
        assert!(on.starts_with(ini));
        assert_eq!(block_names(&on), ["pdo_pgsql", "pgsql"]);
        assert_eq!(source_of(&on, "pgsql"), Some(Source::Block));
        assert_eq!(source_of(&on, "curl"), Some(Source::Ini(2)));

        let one = with_names(&on, &["pgsql".into()]);
        assert_eq!(block_names(&one), ["pgsql"]);
        assert_eq!(with_names(&one, &[]), ini);
    }

    #[test]
    fn php_m_names_match_extension_names() {
        assert!(module_is("Zend OPcache", "opcache"));
        assert!(module_is("Xdebug", "xdebug"));
        assert!(module_is("pdo_pgsql", "pdo_pgsql"));
        assert!(!module_is("pdo_pgsql", "pgsql"));
    }

    /// The PECL host's spelling of a build, from what `php -i` says.
    #[test]
    fn the_build_is_read_from_php_i() {
        let php_i = "PHP Version => 8.5.8\nArchitecture => x64\nPHP Extension Build => API20250925,TS,VS17\n";
        assert_eq!(parse_build(php_i).as_deref(), Some("8.5-ts-vs17-x64"));
        let old = "PHP Version => 5.6.40\nArchitecture => x64\nPHP Extension Build => API20131226,TS,VC11\n";
        assert_eq!(parse_build(old).as_deref(), Some("5.6-ts-vc11-x64"));
        assert_eq!(parse_build("PHP Version => 8.5.8\n"), None);
    }

    #[test]
    fn zend_extensions_get_their_own_directive() {
        assert_eq!(directive("opcache"), "zend_extension=php_opcache.dll");
        assert_eq!(directive("pgsql"), "extension=php_pgsql.dll");
    }

    #[test]
    fn the_extension_directory_follows_the_file() {
        let dir = Path::new(r"C:\devcrate\php\php-8.5");
        assert_eq!(ext_dir(dir, "extension_dir = \"ext\"\n"), dir.join("ext"));
        assert_eq!(ext_dir(dir, ";extension_dir = \"x\"\n"), dir.join("ext"));
        assert_eq!(ext_dir(dir, "extension_dir = \"D:\\exts\"\n"), PathBuf::from(r"D:\exts"));
    }
}
