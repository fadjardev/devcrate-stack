//! `devcrate uninstall` -- removing a runtime from the stack root.
//!
//! The runtimes split into two shapes, and removal follows whichever one
//! applies:
//!
//! - **Versioned, side by side, with a `current` junction** (php, nginx, node,
//!   bun, python): removing one is deleting its folder. If `current` points at
//!   it, the CLI would resolve to nothing afterwards, so that needs `--force`
//!   and the junction is cleared rather than left dangling.
//! - **Single directory holding both binaries and live data** (mariadb,
//!   postgres, rabbitmq): the `data\` a service keeps beside its binaries is
//!   not something `uninstall` destroys by default, the same way `install
//!   --force` never overwrites an existing cluster (see
//!   `install::postgres::initialise`). Everything *but* `data\` is removed;
//!   actually deleting it needs `--data`, and `--data` alone is not enough --
//!   it must be paired with `--force`, since it is the one irreversible path
//!   through this command.
//!
//! Composer is neither: one phar and its shims, no version and no data, so
//! removing it is deleting `composer\` outright. Erlang is a dependency of
//! RabbitMQ rather than a peer, so removing it while RabbitMQ is still
//! installed needs `--force` too.
//!
//! A service that is still running is always refused, `--force` or not --
//! Windows will not cleanly delete a directory a process is executing out of,
//! and a half-deleted install is worse than an error telling you to stop it
//! first.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::config::{Service, ServiceKind, Stack};
use crate::install::Runtime;
use crate::probe::ProcessTable;
use crate::{bun, exit, junction, nginx, node, php, python, site};

/// What a completed removal has to say for itself. The printing lives in
/// [`uninstall`]; this is what a dashboard action would call.
pub struct Uninstalled {
    pub runtime: Runtime,
    /// Empty for the runtimes that carry no version: composer, mariadb,
    /// postgres, rabbitmq, erlang.
    pub version: String,
    /// Root-relative path of what was removed.
    pub dir: String,
    /// A data directory found beside the binaries, present only for the
    /// database services.
    pub data_dir: Option<String>,
    /// Whether `data_dir` was actually deleted (`--data --force`) rather than
    /// left behind.
    pub data_removed: bool,
    /// The version removed was the one `current` pointed at, so the junction
    /// was cleared rather than left dangling.
    pub current_cleared: bool,
}

pub fn uninstall(
    stack: &Stack,
    runtime: &str,
    version: Option<&str>,
    data: bool,
    force: bool,
) -> Result<u8> {
    let runtime = Runtime::parse(runtime)?;

    if version.is_none() && needs_version(runtime) {
        print_candidates(stack, runtime);
        return Ok(exit::OK);
    }

    let done = match runtime {
        Runtime::Php => uninstall_php(stack, version.unwrap(), force)?,
        Runtime::Nginx => uninstall_nginx(stack, version.unwrap(), force)?,
        Runtime::Node => uninstall_node(stack, version.unwrap(), force)?,
        Runtime::Bun => uninstall_bun(stack, version.unwrap(), force)?,
        Runtime::Python => uninstall_python(stack, version.unwrap(), force)?,
        Runtime::Composer => uninstall_composer(stack)?,
        Runtime::MariaDb => uninstall_database_dir(stack, runtime, ServiceKind::MariaDb, data, force)?,
        Runtime::Postgres => uninstall_database_dir(stack, runtime, ServiceKind::Postgres, data, force)?,
        Runtime::RabbitMq => uninstall_database_dir(stack, runtime, ServiceKind::RabbitMq, data, force)?,
        Runtime::Erlang => uninstall_erlang(stack, force)?,
    };

    print_uninstalled(&done);
    Ok(exit::OK)
}

fn needs_version(runtime: Runtime) -> bool {
    matches!(runtime, Runtime::Php | Runtime::Nginx | Runtime::Node | Runtime::Bun | Runtime::Python)
}

fn print_candidates(stack: &Stack, runtime: Runtime) {
    println!("Which {} version? Installed:", runtime.label());
    match runtime {
        Runtime::Php => {
            for s in stack.php_services() {
                println!("  {}", s.id);
            }
        }
        Runtime::Nginx => {
            for v in nginx::versions(stack) {
                println!("  {}", v.version);
            }
        }
        Runtime::Node => {
            for v in node::list_installed(stack) {
                println!("  {}", v.version);
            }
        }
        Runtime::Bun => {
            for v in bun::list_installed(stack) {
                println!("  {}", v.version);
            }
        }
        Runtime::Python => {
            for (_, v) in python::versions(stack) {
                println!("  {}", v);
            }
        }
        _ => {}
    }
}

fn print_uninstalled(done: &Uninstalled) {
    match done.version.is_empty() {
        true => println!("{} removed from {}", done.runtime.label(), done.dir),
        false => println!("{} {} removed from {}", done.runtime.label(), done.version, done.dir),
    }
    match (&done.data_dir, done.data_removed) {
        (Some(data), true) => println!("  {data} was also deleted (--data)"),
        (Some(data), false) => {
            println!("  {data} was left in place; pass --data --force to also delete it")
        }
        (None, _) => {}
    }
    if done.current_cleared {
        println!("  it was the active version; `current` has been cleared");
    }
}

// ---------------------------------------------------------------------------
// Versioned, side by side, with a `current` junction
// ---------------------------------------------------------------------------

fn uninstall_php(stack: &Stack, wanted: &str, force: bool) -> Result<Uninstalled> {
    let service = php::find(stack, wanted)?;
    let dir = service
        .install_marker
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent", service.install_marker.display()))?
        .to_path_buf();
    let id = service.id.clone();
    let version = service.name.trim_start_matches("PHP ").to_string();
    let port = service.ports.first().copied();

    let running = ProcessTable::scan().matching(&service.process_prefix, &service.exclude_names);
    if !running.is_empty() {
        bail!(
            "PHP {id} is still running (pid {}). Stop it first: `devcrate stop {id}`.",
            running[0]
        );
    }

    let using: Vec<String> = stack
        .sites()
        .iter()
        .map(|conf| site::Site::read(conf))
        .filter(|s| s.fastcgi_port.is_some() && s.fastcgi_port == port)
        .map(|s| s.host)
        .collect();
    if !using.is_empty() && !force {
        bail!(
            "{id} is still used by vhost(s): {}.\n\
             Point them at another version first (`devcrate site set-php <host> <version>`), \
             or pass --force to remove it anyway.",
            using.join(", ")
        );
    }

    let was_current = stack.current_php().as_deref() == Some(dir.as_path());

    std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", stack.rel(&dir)))?;

    let current_cleared = if was_current {
        junction::remove(&stack.php_dir.join("current"))?;
        true
    } else {
        false
    };

    Ok(Uninstalled {
        runtime: Runtime::Php,
        version,
        dir: stack.rel(&dir),
        data_dir: None,
        data_removed: false,
        current_cleared,
    })
}

fn uninstall_nginx(stack: &Stack, wanted: &str, force: bool) -> Result<Uninstalled> {
    let available = nginx::versions(stack);
    let target = nginx::find(&available, wanted)?;
    let dir = target.dir.clone();
    let version = target.version.clone();

    if !nginx::running(&dir).is_empty() {
        bail!(
            "nginx {version} is still running out of {}. Stop it first: `devcrate stop nginx`.",
            stack.rel(&dir)
        );
    }

    let was_current = stack.current_nginx().as_deref() == Some(dir.as_path());
    if was_current && !force {
        bail!(
            "nginx\\current points at {version}; removing it leaves nothing for \
             `devcrate start nginx` to run. Switch first (`devcrate nginx use <version>`), \
             or pass --force."
        );
    }

    std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", stack.rel(&dir)))?;

    let current_cleared = if was_current {
        junction::remove(&stack.nginx_prefix.join("current"))?;
        true
    } else {
        false
    };

    Ok(Uninstalled {
        runtime: Runtime::Nginx,
        version,
        dir: stack.rel(&dir),
        data_dir: None,
        data_removed: false,
        current_cleared,
    })
}

fn uninstall_node(stack: &Stack, wanted: &str, force: bool) -> Result<Uninstalled> {
    let target = node::find(stack, wanted)?;

    if !ProcessTable::scan().matching(&target.path, &[]).is_empty() {
        bail!(
            "Node.js v{} is still running out of {}. Stop whatever is using it first.",
            target.version,
            stack.rel(&target.path)
        );
    }
    if target.active && !force {
        bail!(
            "node\\current points at v{}; removing it leaves `node` unresolved. \
             Switch first (`devcrate node use <version>`), or pass --force.",
            target.version
        );
    }

    std::fs::remove_dir_all(&target.path)
        .with_context(|| format!("removing {}", stack.rel(&target.path)))?;

    let current_cleared = if target.active {
        junction::remove(&stack.root.join("node").join("current"))?;
        true
    } else {
        false
    };

    Ok(Uninstalled {
        runtime: Runtime::Node,
        version: target.version,
        dir: stack.rel(&target.path),
        data_dir: None,
        data_removed: false,
        current_cleared,
    })
}

fn uninstall_bun(stack: &Stack, wanted: &str, force: bool) -> Result<Uninstalled> {
    let target = bun::find(stack, wanted)?;

    if !ProcessTable::scan().matching(&target.path, &[]).is_empty() {
        bail!(
            "Bun v{} is still running out of {}. Stop whatever is using it first.",
            target.version,
            stack.rel(&target.path)
        );
    }
    if target.active && !force {
        bail!(
            "bun\\current points at v{}; removing it leaves `bun` unresolved. \
             Switch first (`devcrate bun use <version>`), or pass --force.",
            target.version
        );
    }

    std::fs::remove_dir_all(&target.path)
        .with_context(|| format!("removing {}", stack.rel(&target.path)))?;

    let current_cleared = if target.active {
        junction::remove(&stack.root.join("bun").join("current"))?;
        true
    } else {
        false
    };

    Ok(Uninstalled {
        runtime: Runtime::Bun,
        version: target.version,
        dir: stack.rel(&target.path),
        data_dir: None,
        data_removed: false,
        current_cleared,
    })
}

fn uninstall_python(stack: &Stack, wanted: &str, force: bool) -> Result<Uninstalled> {
    let (dir, version) = python::find(stack, wanted)?;

    if !ProcessTable::scan().matching(&dir, &[]).is_empty() {
        bail!("Python {version} is still running out of {}.", stack.rel(&dir));
    }

    let was_current = stack.current_python().as_deref() == Some(dir.as_path());
    if was_current && !force {
        bail!(
            "python\\current points at {version}; removing it leaves `python` unresolved. \
             Switch first (`devcrate python use <version>`), or pass --force."
        );
    }

    std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", stack.rel(&dir)))?;

    let current_cleared = if was_current {
        junction::remove(&stack.python_dir.join("current"))?;
        true
    } else {
        false
    };

    Ok(Uninstalled {
        runtime: Runtime::Python,
        version,
        dir: stack.rel(&dir),
        data_dir: None,
        data_removed: false,
        current_cleared,
    })
}

// ---------------------------------------------------------------------------
// A tool: no version, no data
// ---------------------------------------------------------------------------

fn uninstall_composer(stack: &Stack) -> Result<Uninstalled> {
    let dir = stack.root.join("composer");
    if !dir.join("composer.phar").is_file() {
        bail!("Composer is not installed ({} not found)", stack.rel(&dir.join("composer.phar")));
    }

    std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", stack.rel(&dir)))?;

    Ok(Uninstalled {
        runtime: Runtime::Composer,
        version: String::new(),
        dir: stack.rel(&dir),
        data_dir: None,
        data_removed: false,
        current_cleared: false,
    })
}

// ---------------------------------------------------------------------------
// Single directory holding both binaries and live data
// ---------------------------------------------------------------------------

/// `service.install_marker` is `<dir>\bin\<exe>` (mariadb, postgres) or
/// `<dir>\sbin\<script>` (rabbitmq) -- two levels up from either is the
/// directory this runtime actually lives in, the same walk
/// [`Stack::to_config`](crate::config::Stack::to_config) does.
fn service_dir(service: &Service) -> Result<PathBuf> {
    service
        .install_marker
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("{} has no ancestor 2 levels up", service.install_marker.display()))
}

fn uninstall_database_dir(
    stack: &Stack,
    runtime: Runtime,
    kind: ServiceKind,
    remove_data: bool,
    force: bool,
) -> Result<Uninstalled> {
    let service =
        stack.by_kind(kind).ok_or_else(|| anyhow!("{} is not configured", runtime.label()))?;
    let dest = service_dir(service)?;

    if !dest.is_dir() {
        bail!("{} is not installed ({} not found)", runtime.label(), stack.rel(&dest));
    }

    let running = ProcessTable::scan().matching(&service.process_prefix, &service.exclude_names);
    if !running.is_empty() {
        bail!(
            "{} is still running (pid {}). Stop it first: `devcrate stop {}`.",
            runtime.label(),
            running[0],
            service.id
        );
    }

    let data = dest.join("data");
    let has_data = data.is_dir();

    if has_data && remove_data && !force {
        bail!(
            "--data also deletes {}, which cannot be undone. Pass --force to confirm.",
            stack.rel(&data)
        );
    }

    if has_data && !remove_data {
        remove_contents_except(&dest, &data)
            .with_context(|| format!("removing {} (keeping data\\)", stack.rel(&dest)))?;
        return Ok(Uninstalled {
            runtime,
            version: String::new(),
            dir: stack.rel(&dest),
            data_dir: Some(stack.rel(&data)),
            data_removed: false,
            current_cleared: false,
        });
    }

    std::fs::remove_dir_all(&dest).with_context(|| format!("removing {}", stack.rel(&dest)))?;
    Ok(Uninstalled {
        runtime,
        version: String::new(),
        dir: stack.rel(&dest),
        data_dir: has_data.then(|| stack.rel(&data)),
        data_removed: has_data,
        current_cleared: false,
    })
}

/// Delete everything directly under `dir` except `keep`.
fn remove_contents_except(dir: &Path, keep: &Path) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path == keep {
            continue;
        }
        if path.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

fn uninstall_erlang(stack: &Stack, force: bool) -> Result<Uninstalled> {
    let rabbit = stack.by_kind(ServiceKind::RabbitMq);
    let erlang_dir = rabbit.map(|r| r.process_prefix.clone()).unwrap_or_else(|| stack.root.join("erlang"));

    if !erlang_dir.is_dir() {
        bail!("Erlang/OTP is not installed ({} not found)", stack.rel(&erlang_dir));
    }

    if !ProcessTable::scan().matching(&erlang_dir, &[]).is_empty() {
        bail!("Erlang/OTP is still running. Stop RabbitMQ first: `devcrate stop rabbitmq`.");
    }

    let rabbitmq_installed = rabbit.is_some_and(|r| r.install_marker.is_file());
    if rabbitmq_installed && !force {
        bail!(
            "Erlang/OTP is RabbitMQ's dependency, and RabbitMQ is still installed. \
             Remove RabbitMQ first (`devcrate uninstall rabbitmq`), or pass --force."
        );
    }

    std::fs::remove_dir_all(&erlang_dir)
        .with_context(|| format!("removing {}", stack.rel(&erlang_dir)))?;

    Ok(Uninstalled {
        runtime: Runtime::Erlang,
        version: String::new(),
        dir: stack.rel(&erlang_dir),
        data_dir: None,
        data_removed: false,
        current_cleared: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one thing worth a unit test with no `Stack` fixture to build: the
    /// data-preserving delete keeps exactly the one directory it was told to,
    /// whatever else is sitting beside it.
    #[test]
    fn removing_contents_except_keeps_only_the_named_entry() {
        let dir = std::env::temp_dir().join("devcrate-uninstall-keep-data");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(dir.join("data").join("PG_VERSION"), "13").unwrap();
        std::fs::write(dir.join("my.ini"), "[mysqld]").unwrap();

        remove_contents_except(&dir, &dir.join("data")).unwrap();

        assert!(!dir.join("bin").exists());
        assert!(!dir.join("my.ini").exists());
        assert!(dir.join("data").join("PG_VERSION").is_file());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
