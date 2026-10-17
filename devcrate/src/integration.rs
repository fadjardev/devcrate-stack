//! What devcrate writes outside the stack root, and taking it back.
//!
//! Four things, all per user and none needing Administrator:
//!
//! - **Shortcuts** to the dashboard in the Start Menu and on the Desktop,
//!   made by `setup` unless `--no-shortcuts`.
//! - **A Windows Terminal profile**, as a JSON *fragment* in Windows Terminal's
//!   fragments folder -- the documented way for an app to add a profile, which
//!   leaves the user's own `settings.json` untouched. Written only where
//!   Windows Terminal is installed.
//! - **Autostart**: a Task Scheduler task, `\Devcrate\Start <root>`, that runs
//!   `devcrate start` when this user logs on. Off unless `devcrate autostart
//!   on`.
//! - **User `PATH` entries** for the stack's folders, added by `install` the
//!   first time it installs a runtime and by `devcrate path add` (see
//!   [`crate::userpath`]).
//!
//! Every path and task written is recorded in the root's `devcrate-setup.json`,
//! and `setup --undo` removes exactly what that receipt lists -- so two stack
//! roots on one machine never take each other's shortcuts with them. Names
//! carry the root's folder when it is not called `devcrate`, for the same
//! reason.
//!
//! The shell and the scheduler are reached through PowerShell, with the script
//! passed as `-EncodedCommand` so no path ever goes through a second layer of
//! quoting.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::Stack;
use crate::{control, exit};

const RECEIPT: &str = "devcrate-setup.json";
const TASK_PATH: &str = "\\Devcrate\\";

// ---------------------------------------------------------------------------
// The receipt
// ---------------------------------------------------------------------------

/// What this root has written outside itself.
#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Receipt {
    /// Shortcuts and the Windows Terminal fragment.
    #[serde(default)]
    pub files: Vec<PathBuf>,
    /// The autostart task's name under `\Devcrate\`, while it is on.
    #[serde(default)]
    pub task: Option<String>,
    /// Folders put on the user `PATH`, as they were written there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<String>,
}

impl Receipt {
    pub fn read(stack: &Stack) -> Result<Receipt> {
        let path = stack.root.join(RECEIPT);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).with_context(|| format!("reading {}", stack.rel(&path))),
            Err(_) => Ok(Receipt::default()),
        }
    }

    fn write(&self, stack: &Stack) -> Result<()> {
        let path = stack.root.join(RECEIPT);
        if self.files.is_empty() && self.task.is_none() && self.path.is_empty() {
            if path.exists() {
                std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
            }
            return Ok(());
        }
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, text + "\n").with_context(|| format!("writing {}", path.display()))
    }

    fn add_file(&mut self, path: &Path) {
        if !self.files.iter().any(|known| same_path(known, path)) {
            self.files.push(path.to_path_buf());
        }
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

/// Note folders put on, and taken off, the user `PATH`.
pub fn record_path(stack: &Stack, added: &[String], removed: &[String]) -> Result<()> {
    let mut receipt = Receipt::read(stack)?;
    receipt.path.retain(|known| !removed.iter().any(|gone| same_path(Path::new(known), Path::new(gone))));
    for dir in added {
        if !receipt.path.iter().any(|known| same_path(Path::new(known), Path::new(dir))) {
            receipt.path.push(dir.clone());
        }
    }
    receipt.write(stack)
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

/// `Devcrate` for a root folder called devcrate, `Devcrate (dev)` otherwise.
pub fn label(root: &Path) -> String {
    match root.file_name().map(|n| n.to_string_lossy().into_owned()) {
        Some(name) if !name.eq_ignore_ascii_case("devcrate") => format!("Devcrate ({name})"),
        _ => "Devcrate".to_string(),
    }
}

/// `E-dev` for `E:\dev`: this root's name in places every root shares.
pub fn tag(root: &Path) -> String {
    let text = root.to_string_lossy().replace(':', "");
    let mut out = String::new();
    for c in text.chars() {
        let c = if c.is_ascii_alphanumeric() || c == '.' || c == '_' { c } else { '-' };
        if !(c == '-' && out.ends_with('-')) {
            out.push(c);
        }
    }
    out.trim_matches('-').to_string()
}

/// The autostart task's name under `\Devcrate\`.
pub fn task_name(root: &Path) -> String {
    format!("Start {}", tag(root))
}

/// The `devcrate.exe` the shortcuts and the task run: the stack's own copy,
/// or this one when the root has none.
fn exe(stack: &Stack) -> Result<PathBuf> {
    let own = stack.root.join("devcrate.exe");
    if own.is_file() {
        return Ok(own);
    }
    std::env::current_exe().context("finding devcrate.exe")
}

// ---------------------------------------------------------------------------
// Where things go
// ---------------------------------------------------------------------------

/// The per-user folders the shortcuts and the profile go into.
#[derive(Debug, Clone)]
pub struct Places {
    /// The Start Menu's `Programs`.
    pub programs: PathBuf,
    pub desktop: PathBuf,
    /// Windows Terminal's fragments folder, when Windows Terminal is installed.
    pub wt_fragments: Option<PathBuf>,
}

impl Places {
    /// This user's, as Windows reports them -- the Desktop is often redirected
    /// into OneDrive, so it is asked for rather than assumed.
    pub fn of_user() -> Result<Places> {
        let out = powershell(
            "[Environment]::GetFolderPath('Programs')\n[Environment]::GetFolderPath('Desktop')\n",
        )?;
        let mut lines = out.lines().map(str::trim).filter(|l| !l.is_empty());
        let programs = PathBuf::from(lines.next().ok_or_else(|| anyhow!("Windows named no Start Menu folder"))?);
        let desktop = PathBuf::from(lines.next().ok_or_else(|| anyhow!("Windows named no Desktop folder"))?);
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let wt_fragments = local
            .filter(|local| local.join("Microsoft").join("WindowsApps").join("wt.exe").exists())
            .map(|local| local.join("Microsoft").join("Windows Terminal").join("Fragments"));
        Ok(Places { programs, desktop, wt_fragments })
    }
}

// ---------------------------------------------------------------------------
// Shortcuts and the Windows Terminal profile
// ---------------------------------------------------------------------------

/// One file written, and whether it was there before.
#[derive(Debug)]
pub struct Made {
    pub path: PathBuf,
    pub existed: bool,
}

/// Write (or refresh) the Start Menu and Desktop shortcuts and, where Windows
/// Terminal is installed, its profile; record them in the receipt.
pub fn write_shortcuts(stack: &Stack, places: &Places) -> Result<Vec<Made>> {
    let exe = exe(stack)?;
    let name = format!("{}.lnk", label(&stack.root));
    let links = [places.programs.join(&name), places.desktop.join(&name)];
    let mut made: Vec<Made> = links.iter().map(|path| Made { path: path.clone(), existed: path.exists() }).collect();

    let mut script = String::from("$shell = New-Object -ComObject WScript.Shell\n");
    for link in &links {
        script.push_str(&format!(
            "$s = $shell.CreateShortcut({link})\n$s.TargetPath = {exe}\n$s.Arguments = {args}\n\
             $s.WorkingDirectory = {root}\n$s.Description = {description}\n$s.Save()\n",
            link = ps_quote(&link.to_string_lossy()),
            exe = ps_quote(&exe.to_string_lossy()),
            args = ps_quote(&format!("--root \"{}\"", stack.root.display())),
            root = ps_quote(&stack.root.to_string_lossy()),
            description = ps_quote(&format!("The devcrate dashboard for {}", stack.root.display())),
        ));
    }
    powershell(&script).context("writing the shortcuts")?;

    if let Some(fragments) = &places.wt_fragments {
        let path = fragments.join("Devcrate").join(format!("{}.json", tag(&stack.root)));
        let existed = path.exists();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        std::fs::write(&path, wt_fragment(&stack.root, &exe))
            .with_context(|| format!("writing {}", path.display()))?;
        made.push(Made { path, existed });
    }

    let mut receipt = Receipt::read(stack)?;
    for one in &made {
        receipt.add_file(&one.path);
    }
    receipt.write(stack)?;
    Ok(made)
}

/// A Windows Terminal fragment adding one profile that opens the dashboard.
/// The GUID is derived from the root, so a rewrite updates the same profile
/// rather than adding a second.
pub fn wt_fragment(root: &Path, exe: &Path) -> String {
    let json = serde_json::json!({
        "profiles": [{
            "guid": profile_guid(root),
            "name": label(root),
            "commandline": format!("\"{}\" --root \"{}\"", exe.display(), root.display()),
            "startingDirectory": root.display().to_string(),
        }]
    });
    serde_json::to_string_pretty(&json).unwrap_or_default() + "\n"
}

fn profile_guid(root: &Path) -> String {
    let digest = Sha256::digest(root.to_string_lossy().to_ascii_lowercase().as_bytes());
    let mut b = [0u8; 16];
    b.copy_from_slice(&digest[..16]);
    b[6] = (b[6] & 0x0f) | 0x50; // version 5 (name-based)
    b[8] = (b[8] & 0x3f) | 0x80; // RFC 4122 variant
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{{{}-{}-{}-{}-{}}}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

// ---------------------------------------------------------------------------
// Autostart
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum Autostart {
    Off,
    /// On; when it last ran (`None`: not yet) and its exit code then.
    On { last_run: Option<String>, last_result: Option<i64> },
}

pub fn autostart_status(stack: &Stack) -> Result<Autostart> {
    let script = format!(
        "$t = Get-ScheduledTask -TaskPath {path} -TaskName {name} -ErrorAction SilentlyContinue\n\
         if (-not $t) {{ 'off'; exit 0 }}\n\
         $i = $t | Get-ScheduledTaskInfo\n\
         'on'\n\
         if ($i.LastRunTime -and $i.LastRunTime.Year -ge 2000) {{ $i.LastRunTime.ToString('yyyy-MM-dd HH:mm', [Globalization.CultureInfo]::InvariantCulture) }} else {{ '-' }}\n\
         $i.LastTaskResult\n",
        path = ps_quote(TASK_PATH),
        name = ps_quote(&task_name(&stack.root)),
    );
    let out = powershell(&script)?;
    let mut lines = out.lines().map(str::trim);
    if lines.next() != Some("on") {
        return Ok(Autostart::Off);
    }
    let last_run = lines.next().filter(|l| *l != "-").map(str::to_string);
    let last_result = match last_run {
        Some(_) => lines.next().and_then(|l| l.parse().ok()),
        None => None,
    };
    Ok(Autostart::On { last_run, last_result })
}

/// Register the task; `Ok(false)` when it was already there (it is rewritten,
/// so a root that moved points at its new place).
pub fn autostart_on(stack: &Stack) -> Result<bool> {
    let was_on = matches!(autostart_status(stack)?, Autostart::On { .. });
    let exe = exe(stack)?;
    let name = task_name(&stack.root);
    let script = format!(
        "$user = \"$env:USERDOMAIN\\$env:USERNAME\"\n\
         $action = New-ScheduledTaskAction -Execute {exe} -Argument {args} -WorkingDirectory {root}\n\
         $trigger = New-ScheduledTaskTrigger -AtLogOn -User $user\n\
         $principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited\n\
         $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 10)\n\
         Register-ScheduledTask -TaskPath {path} -TaskName {name} -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Description {description} -Force | Out-Null\n",
        exe = ps_quote(&exe.to_string_lossy()),
        args = ps_quote(&format!("--root \"{}\" autostart run", stack.root.display())),
        root = ps_quote(&stack.root.to_string_lossy()),
        path = ps_quote(TASK_PATH),
        name = ps_quote(&name),
        description = ps_quote(&format!("devcrate start for {}, at logon", stack.root.display())),
    );
    powershell(&script).context("registering the autostart task")?;
    let mut receipt = Receipt::read(stack)?;
    receipt.task = Some(name);
    receipt.write(stack)?;
    Ok(!was_on)
}

/// Remove the task, and `\Devcrate\` with it once nothing else is in it;
/// `Ok(false)` when it was not there.
pub fn autostart_off(stack: &Stack) -> Result<bool> {
    let name = Receipt::read(stack)?.task.unwrap_or_else(|| task_name(&stack.root));
    let was_on = remove_task(&name)?;
    let mut receipt = Receipt::read(stack)?;
    receipt.task = None;
    receipt.write(stack)?;
    Ok(was_on)
}

fn remove_task(name: &str) -> Result<bool> {
    let script = format!(
        "$t = Get-ScheduledTask -TaskPath {path} -TaskName {name} -ErrorAction SilentlyContinue\n\
         if ($t) {{ Unregister-ScheduledTask -TaskPath {path} -TaskName {name} -Confirm:$false; 'removed' }}\n\
         $service = New-Object -ComObject Schedule.Service\n\
         $service.Connect()\n\
         try {{\n\
           $folder = $service.GetFolder('\\Devcrate')\n\
           if ($folder.GetTasks(1).Count -eq 0 -and $folder.GetFolders(0).Count -eq 0) {{ $service.GetFolder('\\').DeleteFolder('Devcrate', 0) }}\n\
         }} catch {{ }}\n",
        path = ps_quote(TASK_PATH),
        name = ps_quote(name),
    );
    Ok(powershell(&script).context("removing the autostart task")?.contains("removed"))
}

/// `devcrate autostart run`: what the task runs at logon. There is no one to
/// read a console, so it lets go of the one it was given and writes what
/// `start` said to `logs\autostart.log`, which the dashboard's Logs pane lists.
pub fn run_at_logon(stack: &Stack) -> Result<u8> {
    #[cfg(windows)]
    // SAFETY: no arguments; detaching from the console is always permitted.
    unsafe {
        windows_sys::Win32::System::Console::FreeConsole();
    }
    let mut lines = vec![format!("devcrate autostart for {}", stack.root.display())];
    let failed = match control::run_start(stack, None) {
        Ok(steps) => {
            for step in &steps {
                let kind = stack.services.iter().find(|s| s.id == step.id).map(|s| s.kind);
                let why = match kind {
                    Some(kind) => control::describe_start(kind, &step.outcome),
                    None => String::new(),
                };
                lines.push(format!("  {:<11} {}", step.name, why.lines().collect::<Vec<_>>().join(" ")));
            }
            steps.iter().any(|step| step.outcome.is_failure())
        }
        Err(err) => {
            lines.push(format!("  FAILED {err:#}"));
            true
        }
    };
    let log = stack.root.join("logs").join("autostart.log");
    if let Some(dir) = log.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&log, lines.join("\r\n") + "\r\n");
    Ok(if failed { exit::ERROR } else { exit::OK })
}

// ---------------------------------------------------------------------------
// Undo
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct Undone {
    pub removed: Vec<PathBuf>,
    /// Listed in the receipt but already gone.
    pub missing: Vec<PathBuf>,
    pub task_removed: bool,
    /// Taken off the user `PATH`.
    pub path_removed: Vec<String>,
}

/// Remove everything the receipt lists, then the receipt.
pub fn undo(stack: &Stack) -> Result<Undone> {
    let receipt = Receipt::read(stack)?;
    let mut undone = Undone::default();
    for path in &receipt.files {
        if path.exists() {
            std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
            // The fragment's own Devcrate\ folder goes too once it is empty.
            if let Some(dir) = path.parent()
                && dir.file_name().is_some_and(|n| n == "Devcrate")
            {
                let _ = std::fs::remove_dir(dir);
            }
            undone.removed.push(path.clone());
        } else {
            undone.missing.push(path.clone());
        }
    }
    if let Some(name) = &receipt.task {
        undone.task_removed = remove_task(name)?;
    }
    if !receipt.path.is_empty() {
        undone.path_removed = crate::userpath::remove_dirs(stack, &receipt.path)?;
    }
    Receipt::default().write(stack)?;
    Ok(undone)
}

// ---------------------------------------------------------------------------
// PowerShell
// ---------------------------------------------------------------------------

/// Run a script; its output, or its first error line.
fn powershell(script: &str) -> Result<String> {
    let script = format!("$ErrorActionPreference = 'Stop'\n$ProgressPreference = 'SilentlyContinue'\n{script}");
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand"])
        .arg(encode_command(&script))
        .stdin(Stdio::null())
        .output()
        .context("running powershell.exe")?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let message = stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("At line") && !l.starts_with('+'))
        .unwrap_or("powershell.exe failed")
        .to_string();
    bail!("{message}")
}

/// A PowerShell single-quoted string: nothing inside is expanded.
fn ps_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// `-EncodedCommand`'s form: the script as UTF-16LE, in base64.
fn encode_command(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64(&bytes)
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

// ---------------------------------------------------------------------------
// The CLI
// ---------------------------------------------------------------------------

/// `devcrate autostart on|off|status`.
pub fn print_autostart(stack: &Stack, action: &str) -> Result<u8> {
    let name = format!("{TASK_PATH}{}", task_name(&stack.root));
    match action {
        "on" => {
            let new = autostart_on(stack)?;
            println!(
                "autostart {}: `devcrate start` runs when {} logs on (Task Scheduler {name})",
                if new { "on" } else { "already on, refreshed" },
                std::env::var("USERNAME").unwrap_or_else(|_| "this user".into())
            );
            println!("  what it did is written to {}", stack.rel(&stack.root.join("logs").join("autostart.log")));
        }
        "off" => match autostart_off(stack)? {
            true => println!("autostart off: {name} removed"),
            false => println!("autostart was already off"),
        },
        _ => match autostart_status(stack)? {
            Autostart::Off => println!("autostart off"),
            Autostart::On { last_run: None, .. } => println!("autostart on ({name}); it has not run yet"),
            Autostart::On { last_run: Some(at), last_result } => println!(
                "autostart on ({name}); last ran {at}, {}",
                match last_result {
                    Some(0) => "all started".to_string(),
                    Some(code) => format!("exit code {code} -- see logs\\autostart.log"),
                    None => "result unknown".to_string(),
                }
            ),
        },
    }
    Ok(exit::OK)
}

/// `devcrate setup --undo`.
pub fn print_undo(stack: &Stack) -> Result<u8> {
    let undone = undo(stack)?;
    if undone.removed.is_empty() && undone.missing.is_empty() && !undone.task_removed && undone.path_removed.is_empty()
    {
        println!("Nothing to undo: this stack has written nothing outside {}", stack.root.display());
        return Ok(exit::OK);
    }
    for path in &undone.removed {
        println!("  removed  {}", path.display());
    }
    for path in &undone.missing {
        println!("  gone     {} (already removed)", path.display());
    }
    if undone.task_removed {
        println!("  removed  the autostart task {TASK_PATH}{}", task_name(&stack.root));
    }
    for dir in &undone.path_removed {
        println!("  removed  {dir} from your user PATH");
    }
    println!("Nothing outside {} is devcrate's any more. The stack itself is untouched.", stack.root.display());
    Ok(exit::OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_carry_the_root_unless_it_is_devcrate() {
        assert_eq!(label(Path::new(r"C:\devcrate")), "Devcrate");
        assert_eq!(label(Path::new(r"C:\tools\DevCrate")), "Devcrate");
        assert_eq!(label(Path::new(r"E:\dev")), "Devcrate (dev)");
        assert_eq!(tag(Path::new(r"E:\dev")), "E-dev");
        assert_eq!(tag(Path::new(r"D:\My Tools\devcrate")), "D-My-Tools-devcrate");
        assert_eq!(task_name(Path::new(r"E:\dev")), "Start E-dev");
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        // `dir` as PowerShell expects it: UTF-16LE.
        assert_eq!(encode_command("dir"), "ZABpAHIA");
    }

    #[test]
    fn the_terminal_profile_is_stable_per_root() {
        let a = wt_fragment(Path::new(r"E:\dev"), Path::new(r"E:\dev\devcrate.exe"));
        let b = wt_fragment(Path::new(r"E:\DEV"), Path::new(r"E:\dev\devcrate.exe"));
        let json: serde_json::Value = serde_json::from_str(&a).unwrap();
        let profile = &json["profiles"][0];
        assert_eq!(profile["name"], "Devcrate (dev)");
        assert_eq!(profile["commandline"], r#""E:\dev\devcrate.exe" --root "E:\dev""#);
        let guid = profile["guid"].as_str().unwrap();
        assert_eq!(guid.len(), 38);
        assert_eq!(&guid[15..16], "5", "a version-5 GUID: {guid}");
        let other: serde_json::Value = serde_json::from_str(&b).unwrap();
        assert_eq!(other["profiles"][0]["guid"], profile["guid"], "same root, any case, same profile");
    }

    /// Real shortcuts, into scratch folders standing in for the Start Menu and
    /// the Desktop: written, recorded, refreshed, and undone, the receipt with
    /// them.
    #[test]
    fn shortcuts_are_recorded_and_undone() {
        use crate::root::{RootSource, StackRoot};
        let base = std::env::temp_dir().join(format!("devcrate-integration-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("shop");
        std::fs::create_dir_all(root.join("nginx").join("conf")).unwrap();
        std::fs::write(root.join("nginx").join("conf").join("nginx.conf"), "").unwrap();
        let places = Places {
            programs: base.join("Programs"),
            desktop: base.join("Desktop"),
            wt_fragments: Some(base.join("Fragments")),
        };
        std::fs::create_dir_all(&places.programs).unwrap();
        std::fs::create_dir_all(&places.desktop).unwrap();
        let stack = Stack::open(StackRoot { path: root.clone(), source: RootSource::Flag }).unwrap();

        let made = write_shortcuts(&stack, &places).unwrap();
        assert_eq!(made.len(), 3);
        assert!(made.iter().all(|m| !m.existed && m.path.is_file()), "{made:?}");
        assert!(places.desktop.join("Devcrate (shop).lnk").is_file());
        assert_eq!(Receipt::read(&stack).unwrap().files.len(), 3);

        let again = write_shortcuts(&stack, &places).unwrap();
        assert!(again.iter().all(|m| m.existed));
        assert_eq!(Receipt::read(&stack).unwrap().files.len(), 3, "not recorded twice");

        let undone = undo(&stack).unwrap();
        assert_eq!(undone.removed.len(), 3);
        assert!(!undone.task_removed);
        assert!(!places.programs.join("Devcrate (shop).lnk").exists());
        assert!(!base.join("Fragments").join("Devcrate").exists(), "its folder went with it");
        assert!(!root.join(RECEIPT).exists());
        assert!(undo(&stack).unwrap().removed.is_empty(), "a second undo has nothing to do");
        let _ = std::fs::remove_dir_all(&base);
    }
}
