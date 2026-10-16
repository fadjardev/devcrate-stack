//! `devcrate setup` -- a working default stack in one step.
//!
//! nginx, the newest PHP, MariaDB, Composer, and mkcert, each through the
//! installer `devcrate install` already uses; then mkcert's local CA trusted,
//! `php\current` pointed at a PHP if nothing is, and the stack started.
//!
//! Every step looks first and acts only on what is missing, so a stack that is
//! already set up comes out of a second run unchanged -- the same steps report
//! "already there". The first failure stops the run: what came before it is
//! installed and stays installed, and running `setup` again carries on from
//! the step that failed.
//!
//! It installs binaries, not configuration. `nginx\conf\nginx.conf` and each
//! version's `php.ini` are the stack's own files, carried by the repository or
//! the release zip, so setup refuses to start without the first rather than
//! inventing one.

use anyhow::{Result, anyhow, bail};

use crate::config::{ServiceKind, Stack};
use crate::install::InstallOutcome;
use crate::root::StackRoot;
use crate::{control, exit, mkcert, php};

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Already there; nothing was done.
    Present(String),
    /// Done by this run.
    Done(String),
}

pub struct Step {
    pub name: &'static str,
    pub outcome: Outcome,
}

/// The runtimes setup installs, in order, with the version keyword each is
/// installed as.
const RUNTIMES: [(&str, &str); 4] =
    [("nginx", "stable"), ("php", "latest"), ("mariadb", "latest"), ("composer", "stable")];

/// Run every step, reporting each through `on_step` as it finishes and progress
/// through `on_progress`. Returns the steps, or the error the first failing one
/// gave, named.
pub fn run(
    root: &StackRoot,
    start: bool,
    on_step: &mut dyn FnMut(&Step),
    on_progress: &mut dyn FnMut(&str),
) -> Result<Vec<Step>> {
    let mut steps = Vec::new();
    let mut record = |step: Step, steps: &mut Vec<Step>| {
        on_step(&step);
        steps.push(step);
    };

    let stack = Stack::open(root.clone())?;
    let conf = stack.nginx_prefix.join("conf").join("nginx.conf");
    if !conf.is_file() {
        bail!(
            "{} is missing. Setup installs binaries, not the stack's configuration -- \
             run it from a clone of the repository or an extracted release zip.",
            stack.rel(&conf)
        );
    }

    for (runtime, keyword) in RUNTIMES {
        // Re-read each time: the service list is discovered from disk, so an
        // install a step ago is only visible to a freshly opened stack.
        let stack = Stack::open(root.clone())?;
        let name = label(runtime);
        let outcome = match present(&stack, runtime) {
            Some(what) => Outcome::Present(what),
            None => {
                let done = crate::install::install_quiet(&stack, runtime, Some(keyword), false, on_progress)
                    .map_err(|err| anyhow!("{name}: {err:#}"))?;
                match done {
                    InstallOutcome::Installed(done) => {
                        Outcome::Done(format!("{} {} installed into {}", name, done.release, done.dir))
                    }
                    InstallOutcome::Listed => bail!("{name}: nothing was installed for `{keyword}`"),
                }
            }
        };
        record(Step { name, outcome }, &mut steps);
    }

    let stack = Stack::open(root.clone())?;
    let outcome = match stack.current_php() {
        Some(dir) => Outcome::Present(format!("php\\current -> {}", stack.rel(&dir))),
        None => {
            let newest = stack
                .php_services()
                .filter(|service| service.is_installed())
                .max_by_key(|service| version_key(&service.id))
                .ok_or_else(|| anyhow!("CLI PHP: no PHP version is installed"))?;
            let switched = php::use_version(&stack, &newest.id).map_err(|err| anyhow!("CLI PHP: {err:#}"))?;
            Outcome::Done(format!("php\\current -> {}", switched.dir))
        }
    };
    record(Step { name: "CLI PHP", outcome }, &mut steps);

    let outcome = match mkcert::find_mkcert(&stack) {
        Some(found) => Outcome::Present(stack.rel(&found)),
        None => {
            let mut last = String::new();
            let mut progress = |done: u64, total: Option<u64>| {
                let text = match total {
                    Some(total) if total > 0 => format!("downloading mkcert  {}%", done * 100 / total),
                    _ => format!("downloading mkcert  {} KB", done / 1024),
                };
                if text != last {
                    on_progress(&text);
                    last = text;
                }
            };
            let placed = crate::install::mkcert::place(&stack, &mut progress)
                .map_err(|err| anyhow!("mkcert: {err:#}"))?;
            let check = match (placed.cached, placed.verified) {
                (true, _) => "from _downloads",
                (false, true) => "sha256-verified",
                (false, false) => "length-checked over TLS; the release publishes no checksum",
            };
            Outcome::Done(format!("mkcert {} placed at {} ({check})", placed.version, placed.path))
        }
    };
    record(Step { name: "mkcert", outcome }, &mut steps);

    // `-install` is idempotent: with the CA already trusted it changes nothing.
    // The first time, Windows asks to confirm adding the root certificate. A
    // CA that already existed is reported as such; `-install` still runs, so a
    // CA whose trust was removed is trusted again.
    let mkcert_bin = mkcert::find_mkcert(&stack).ok_or_else(|| anyhow!("local CA: mkcert.exe not found"))?;
    let existed = ca_exists(&mkcert_bin);
    mkcert::ensure_ca_installed(&mkcert_bin).map_err(|err| anyhow!("local CA: {err:#}"))?;
    let outcome = match existed {
        true => Outcome::Present("mkcert's CA, trusted by this user".into()),
        false => Outcome::Done("mkcert's CA created and trusted by this user".into()),
    };
    record(Step { name: "local CA", outcome }, &mut steps);

    if start {
        let started = control::run_start(&stack, None).map_err(|err| anyhow!("start: {err:#}"))?;
        let failed: Vec<String> = started
            .iter()
            .filter(|step| step.outcome.is_failure())
            .map(|step| {
                // One line: a refusal can run to two ("...never created." then
                // the command that fixes it), and these are joined with "; ".
                let why = control::describe_start(kind_of(&stack, &step.id), &step.outcome);
                format!("{} {}", step.name, why.lines().map(str::trim).collect::<Vec<_>>().join(" "))
            })
            .collect();
        if !failed.is_empty() {
            bail!("start: {}", failed.join("; "));
        }
        let up = |s: &&control::Step<control::Started>| !matches!(s.outcome, control::Started::NotInstalled(_));
        let fresh = started.iter().filter(|s| matches!(s.outcome, control::Started::Listening(_))).count();
        let total = started.iter().filter(up).count();
        let outcome = match fresh {
            0 => Outcome::Present(format!("all {total} installed services were already up")),
            _ => Outcome::Done(format!("{fresh} started, {total} up")),
        };
        record(Step { name: "start", outcome }, &mut steps);
    }
    Ok(steps)
}

/// Whether mkcert's CA files exist: `mkcert -CAROOT` names the directory.
fn ca_exists(mkcert_bin: &std::path::Path) -> bool {
    let Ok(output) = std::process::Command::new(mkcert_bin).arg("-CAROOT").output() else {
        return false;
    };
    let dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
    !dir.is_empty() && std::path::Path::new(&dir).join("rootCA.pem").is_file()
}

/// What is already installed for `runtime`, described, or `None`.
fn present(stack: &Stack, runtime: &str) -> Option<String> {
    match runtime {
        "nginx" => stack.nginx_bin.is_file().then(|| format!("nginx at {}", stack.rel(&stack.nginx_bin))),
        "php" => {
            let installed: Vec<&str> = stack
                .php_services()
                .filter(|service| service.is_installed())
                .map(|service| service.id.as_str())
                .collect();
            (!installed.is_empty()).then(|| installed.join(", "))
        }
        "mariadb" => stack
            .by_kind(ServiceKind::MariaDb)
            .filter(|service| service.is_installed())
            .map(|service| format!("MariaDB at {}", stack.rel(&service.install_marker))),
        "composer" => {
            let phar = stack.root.join("composer").join("composer.phar");
            phar.is_file().then(|| format!("Composer at {}", stack.rel(&phar)))
        }
        _ => None,
    }
}

fn label(runtime: &str) -> &'static str {
    match runtime {
        "nginx" => "nginx",
        "php" => "PHP",
        "mariadb" => "MariaDB",
        _ => "Composer",
    }
}

/// `php-8.5` -> (8, 5), so 8.10 sorts above 8.9.
fn version_key(id: &str) -> Vec<u32> {
    id.trim_start_matches("php-").split('.').filter_map(|part| part.parse().ok()).collect()
}

fn kind_of(stack: &Stack, id: &str) -> ServiceKind {
    stack.services.iter().find(|s| s.id == id).map(|s| s.kind).unwrap_or(ServiceKind::Nginx)
}

/// One step as a line, the same on the command line and in the dashboard.
pub fn describe(step: &Step) -> String {
    match &step.outcome {
        Outcome::Present(what) => format!("{:<9} already there: {what}", step.name),
        Outcome::Done(what) => format!("{:<9} {what}", step.name),
    }
}

/// `devcrate setup`.
pub fn print(stack: &Stack, start: bool) -> Result<u8> {
    let root = StackRoot { path: stack.root.clone(), source: stack.root_source };
    println!("Setting up the stack in {}", stack.root.display());
    let mut on_step = |step: &Step| println!("  {}", describe(step));
    let mut last_shown = std::time::Instant::now();
    let mut on_progress = |text: &str| {
        if last_shown.elapsed() >= std::time::Duration::from_secs(2) {
            last_shown = std::time::Instant::now();
            println!("            {text}");
        }
    };
    match run(&root, start, &mut on_step, &mut on_progress) {
        Ok(steps) => {
            if steps.iter().all(|s| matches!(s.outcome, Outcome::Present(_))) {
                println!("Nothing to do: the stack was already set up.");
            } else {
                println!("Ready. `devcrate` opens the dashboard; `devcrate site add myapp.test` serves a project.");
            }
            Ok(exit::OK)
        }
        Err(err) => {
            eprintln!("devcrate: setup stopped at {err:#}");
            eprintln!("What was installed before it stays installed; run `devcrate setup` again to carry on.");
            Ok(exit::ERROR)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn php_versions_order_numerically() {
        assert!(version_key("php-8.10") > version_key("php-8.9"));
        assert!(version_key("php-8.5") > version_key("php-7.4"));
    }

    /// Without the stack's nginx.conf there is nothing for the binaries to
    /// serve with, so setup stops before downloading anything.
    #[test]
    fn setup_refuses_a_root_without_the_stacks_configuration() {
        let root = std::env::temp_dir().join(format!("devcrate-setup-noconf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("php")).unwrap();
        let stack_root = StackRoot { path: root.clone(), source: crate::root::RootSource::Flag };
        let err = run(&stack_root, false, &mut |_| {}, &mut |_| {}).err().expect("refused");
        assert!(err.to_string().contains("nginx.conf is missing"), "{err}");
        assert!(!root.join("_downloads").exists(), "nothing downloaded");
        let _ = std::fs::remove_dir_all(&root);
    }
}
