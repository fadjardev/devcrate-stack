//! Auto-vhost: a folder under `projects\` that no vhost serves becomes
//! `https://<folder>.test`, the way Laragon does it.
//!
//! It only ever adds. A conf that exists is never rewritten, a folder that
//! disappears never takes its conf with it, and the folder itself is served
//! as it is -- no `public\` made in it, no placeholder `index.php`. Every vhost
//! goes through [`site::create_with`], the same as `site add`.
//!
//! "Served" is decided by where each conf's `root` really lands, junctions
//! followed, so a folder reached through `site add --path` or named
//! differently from its host (`projects\shop` behind `shop.local.test`) is
//! recognised as taken.
//!
//! On unless `[sites] auto = false`; `[sites] ignore` names folders to leave
//! alone. `devcrate site sync` runs it whatever the setting says.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::Stack;
use crate::site::{self, Site};

/// A folder that would become a vhost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub folder: String,
    pub host: String,
    pub path: PathBuf,
}

/// Why a folder under `projects\` is not a candidate, for those worth naming.
#[derive(Debug, PartialEq, Eq)]
pub enum Skip {
    /// In `[sites] ignore`.
    Ignored,
    /// Its name cannot be a hostname.
    BadName,
    /// `<host>.conf` exists but serves some other folder.
    ConfExists(String),
}

#[derive(Debug, Default)]
pub struct Plan {
    pub new: Vec<Candidate>,
    pub skipped: Vec<(String, Skip)>,
    /// Folders a vhost already serves.
    pub served: usize,
}

/// Look, and change nothing.
pub fn plan(stack: &Stack) -> Plan {
    let mut plan = Plan::default();
    let projects = stack.root.join("projects");
    let Ok(entries) = std::fs::read_dir(&projects) else {
        return plan;
    };
    let served = served_roots(stack);

    let mut folders: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| (entry.file_name().to_string_lossy().into_owned(), entry.path()))
        // Hidden and underscore-prefixed folders are scratch, not projects.
        .filter(|(name, _)| !name.starts_with('.') && !name.starts_with('_'))
        .collect();
    folders.sort_by_key(|(name, _)| name.to_ascii_lowercase());

    for (folder, path) in folders {
        let Ok(real) = crate::root::clean(&path) else { continue };
        if served.iter().any(|root| root.starts_with(&real) || real.starts_with(root)) {
            plan.served += 1;
            continue;
        }
        if stack.auto_sites_ignore.iter().any(|name| name.eq_ignore_ascii_case(&folder)) {
            plan.skipped.push((folder, Skip::Ignored));
            continue;
        }
        let Some(host) = host_for(&folder) else {
            plan.skipped.push((folder, Skip::BadName));
            continue;
        };
        let conf = stack.sites_dir().join(format!("{host}.conf"));
        if conf.exists() {
            plan.skipped.push((folder, Skip::ConfExists(stack.rel(&conf))));
            continue;
        }
        plan.new.push(Candidate { folder, host, path });
    }
    plan
}

/// Where every vhost's `root` really is, junctions followed.
fn served_roots(stack: &Stack) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(stack.sites_dir()) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("conf")))
        .filter_map(|conf| Site::read(&conf).root)
        .filter_map(|root| {
            let root = Path::new(root.trim_matches('"'));
            let root = if root.is_absolute() { root.to_path_buf() } else { stack.nginx_prefix.join(root) };
            crate::root::clean(&root).ok()
        })
        .collect()
}

/// `Shop` -> `shop.test`; `shop.test` stays itself. `None` for a name that
/// cannot be a hostname -- a space or an underscore, say -- rather than one
/// quietly rewritten into a host nobody would guess.
pub fn host_for(folder: &str) -> Option<String> {
    let name = folder.to_ascii_lowercase();
    let label_ok = |label: &str| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    if !name.split('.').all(label_ok) {
        return None;
    }
    Some(match name.ends_with(".test") {
        true => name,
        false => format!("{name}.test"),
    })
}

/// What a sync may touch outside the stack root. Both on for real use; off in
/// tests, which must not edit the machine's hosts file.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub hosts: bool,
    pub tls: bool,
}

impl Options {
    pub const FULL: Options = Options { hosts: true, tls: true };
}

#[derive(Debug, Default)]
pub struct Synced {
    pub plan: Plan,
    pub created: Vec<site::Created>,
    pub failed: Vec<(String, String)>,
    /// `None` when no hosts entry was asked for; otherwise why it could not
    /// be written, or `Some(None)` when it was.
    pub hosts: Option<Option<String>>,
}

/// Serve every candidate. One failing folder does not stop the others, and the
/// hosts entries go in together at the end -- one elevation prompt at most.
pub fn sync(stack: &Stack, options: Options) -> Result<Synced> {
    let plan = plan(stack);
    let mut synced = Synced::default();
    for candidate in &plan.new {
        let request = site::Request {
            project_path: Some(&candidate.path),
            no_hosts: true,
            no_tls: !options.tls,
            untouched: true,
            ..Default::default()
        };
        match site::create_with(stack, &candidate.host, &request) {
            Ok(created) => synced.created.push(created),
            Err(err) => synced.failed.push((candidate.folder.clone(), format!("{err:#}"))),
        }
    }
    if options.hosts && !synced.created.is_empty() {
        let hosts: Vec<&str> = synced.created.iter().map(|c| c.host.as_str()).collect();
        synced.hosts = Some(match crate::hosts::add_entries(&hosts) {
            Ok(crate::hosts::HostsResult::FallbackManual(why)) => Some(why),
            Ok(_) => None,
            Err(err) => Some(format!("{err:#}")),
        });
    }
    synced.plan = plan;
    Ok(synced)
}

/// The lines a sync comes to, the same on the command line and in the
/// dashboard. Empty when there was nothing to do.
pub fn describe(synced: &Synced) -> Vec<String> {
    let mut lines = Vec::new();
    for created in &synced.created {
        lines.push(format!(
            "new      https://{} -> {} ({})",
            created.host,
            created.public,
            created.php_name
        ));
    }
    for (folder, why) in &synced.failed {
        lines.push(format!("FAILED   {folder}: {why}"));
    }
    if let Some(Some(why)) = &synced.hosts {
        lines.push(format!("hosts    not updated ({why}); add the new hosts as 127.0.0.1 by hand"));
    }
    if let Some(last) = synced.created.last()
        && let site::Reload::Failed(why) = &last.reload
    {
        lines.push(format!("FAILED to reload nginx: {why}"));
    }
    lines
}

/// `devcrate site sync [--dry-run]`.
pub fn print(stack: &Stack, dry_run: bool) -> Result<u8> {
    let projects = stack.root.join("projects");
    println!("Auto-vhost in {}", projects.display());
    if !stack.auto_sites {
        println!("  ([sites] auto = false: only this command runs it)");
    }
    let (plan, lines) = match dry_run {
        true => {
            let plan = plan(stack);
            let lines =
                plan.new.iter().map(|c| format!("would    https://{} -> projects\\{}", c.host, c.folder)).collect();
            (plan, lines)
        }
        false => {
            let mut synced = sync(stack, Options::FULL)?;
            let lines = describe(&synced);
            (std::mem::take(&mut synced.plan), lines)
        }
    };
    for line in &lines {
        println!("  {line}");
    }
    for (folder, why) in &plan.skipped {
        match why {
            Skip::Ignored => println!("  ignored  {folder}"),
            Skip::BadName => println!("  skipped  {folder}: not a usable hostname -- rename it, or `site add` it by hand"),
            Skip::ConfExists(conf) => println!("  skipped  {folder}: {conf} exists and serves another folder"),
        }
    }
    println!("  {} folder(s) already served", plan.served);
    if plan.new.is_empty() {
        println!("Nothing to add.");
    } else if dry_run {
        println!("Dry run: nothing was written. `devcrate site sync` adds them.");
    }
    let failed = lines.iter().any(|line| line.starts_with("FAILED"));
    Ok(if failed { crate::exit::ERROR } else { crate::exit::OK })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root::{RootSource, StackRoot};

    #[test]
    fn folders_become_test_hosts_or_are_refused() {
        assert_eq!(host_for("Shop").as_deref(), Some("shop.test"));
        assert_eq!(host_for("shop.test").as_deref(), Some("shop.test"));
        assert_eq!(host_for("api.shop").as_deref(), Some("api.shop.test"));
        assert_eq!(host_for("1-project-knowledge").as_deref(), Some("1-project-knowledge.test"));
        assert_eq!(host_for("my app"), None);
        assert_eq!(host_for("my_app"), None);
        assert_eq!(host_for("-shop"), None);
        assert_eq!(host_for("shop..x"), None);
    }

    fn root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("devcrate-autosite-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("nginx").join("conf").join("sites")).unwrap();
        std::fs::write(root.join("nginx").join("conf").join("nginx.conf"), "").unwrap();
        std::fs::create_dir_all(root.join("projects")).unwrap();
        crate::junction::create(&root.join("nginx").join("projects"), &root.join("projects")).unwrap();
        root
    }

    fn open(root: &Path) -> Stack {
        Stack::open(StackRoot { path: root.to_path_buf(), source: RootSource::Flag }).unwrap()
    }

    fn conf(root: &Path, host: &str, served: &str) {
        std::fs::write(
            root.join("nginx").join("conf").join("sites").join(format!("{host}.conf")),
            format!("server {{\n    server_name {host};\n    root   {served};\n}}\n"),
        )
        .unwrap();
    }

    /// A served folder is recognised whatever its vhost is called, through
    /// `public`, through a junction, or deeper; the rest are sorted into new,
    /// ignored, and unusable.
    #[test]
    fn the_plan_finds_only_unserved_folders() {
        let root = root("plan");
        let projects = root.join("projects");
        for dir in ["shop/public", "blog", "deep/web/root", "fresh", "temp", "my app", ".git", "_scratch", "taken"] {
            std::fs::create_dir_all(projects.join(dir)).unwrap();
        }
        std::fs::write(projects.join("notes.txt"), "").unwrap();
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        crate::junction::create(&projects.join("linked.test"), &elsewhere).unwrap();

        conf(&root, "shop.local.test", "projects/shop/public");
        conf(&root, "blog.test", "projects/blog");
        conf(&root, "deep.test", "projects/deep/web/root");
        conf(&root, "linked.test", "projects/linked.test");
        conf(&root, "taken.test", "projects/blog");

        let mut stack = open(&root);
        stack.auto_sites_ignore = vec!["TEMP".into()];
        let plan = plan(&stack);

        let new: Vec<&str> = plan.new.iter().map(|c| c.host.as_str()).collect();
        assert_eq!(new, ["fresh.test"]);
        assert_eq!(plan.served, 4, "shop, blog, deep, linked.test");
        assert!(plan.skipped.contains(&("temp".into(), Skip::Ignored)));
        assert!(plan.skipped.contains(&("my app".into(), Skip::BadName)));
        assert!(plan.skipped.iter().any(|(f, why)| f == "taken" && matches!(why, Skip::ConfExists(_))));
        assert!(!plan.skipped.iter().any(|(f, _)| f.starts_with('.') || f.starts_with('_')));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A sync serves the folder where it is, by its own name, without writing
    /// into it, and a second sync finds nothing left to do.
    #[test]
    fn a_sync_serves_the_folder_untouched_and_only_once() {
        let root = root("sync");
        let php = root.join("php").join("php-8.5");
        std::fs::create_dir_all(&php).unwrap();
        std::fs::write(php.join("php-cgi.exe"), "").unwrap();
        crate::junction::create(&root.join("php").join("current"), &php).unwrap();
        let shop = root.join("projects").join("Shop");
        std::fs::create_dir_all(&shop).unwrap();
        std::fs::write(shop.join("index.html"), "<h1>mine</h1>").unwrap();

        let stack = open(&root);
        let synced = sync(&stack, Options { hosts: false, tls: false }).unwrap();
        assert!(synced.failed.is_empty(), "{:?}", synced.failed);
        assert_eq!(synced.created.len(), 1);
        assert_eq!(synced.hosts, None, "hosts left alone when told to");

        let conf = std::fs::read_to_string(root.join("nginx/conf/sites/shop.test.conf")).unwrap();
        assert!(conf.contains("root   projects/Shop;"), "{conf}");
        let mut inside: Vec<String> =
            std::fs::read_dir(&shop).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        inside.sort();
        assert_eq!(inside, ["index.html"], "no public\\ and no index.php written");
        assert!(!root.join("projects").join("shop.test").exists(), "no junction beside it");
        assert!(describe(&synced)[0].starts_with("new      https://shop.test -> projects\\Shop"));

        let again = sync(&stack, Options { hosts: false, tls: false }).unwrap();
        assert!(again.created.is_empty() && again.plan.new.is_empty());
        assert_eq!(again.plan.served, 1);

        // Deleting it names the folder it really served, and warns that the
        // folder will be served again. Never in the machine's hosts file, so
        // the hosts step changes nothing.
        let gone = site::delete(&stack, "shop.test").unwrap();
        assert_eq!(gone.folder, "projects\\Shop");
        assert_eq!(gone.auto_returns.as_deref(), Some("Shop"));
        assert!(shop.join("index.html").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }
}
