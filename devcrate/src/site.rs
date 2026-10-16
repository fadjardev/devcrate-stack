//! Reading and writing the generated vhost confs.
//!
//! Reading is deliberately a shallow scan rather than an nginx config parser:
//! it pulls out the two lines `new-vhost.bat` writes per site and leaves
//! everything else alone, so a hand-edited conf is reported, never rewritten.
//!
//! Writing produces the same conf `new-vhost.bat` produces, and refuses to
//! overwrite one that already exists unless told to.

use std::path::Path;

use anyhow::{Context, Result, anyhow};

use crate::config::Stack;
use crate::{control, exit, php};

#[derive(Debug, Clone)]
pub struct Site {
    pub host: String,
    /// The `root` directive, as written -- prefix-relative in generated confs.
    pub root: Option<String>,
    /// Port from `fastcgi_pass 127.0.0.1:<port>`.
    pub fastcgi_port: Option<u16>,
    /// Port from `proxy_pass http://127.0.0.1:<port>`: a vhost that forwards
    /// to another server (a Node app, a dev server) instead of running PHP.
    pub proxy_port: Option<u16>,
    /// A `listen` on 443 or with `ssl` -- every generated conf has one; a
    /// hand-written one may not.
    pub tls: bool,
}

impl Site {
    pub fn read(conf: &Path) -> Site {
        let host = conf
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| conf.display().to_string());
        let text = std::fs::read_to_string(conf).unwrap_or_default();

        let mut root = None;
        let mut fastcgi_port = None;
        let mut proxy_port = None;
        let mut tls = false;

        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            if root.is_none()
                && let Some(rest) = line.strip_prefix("root ")
            {
                root = Some(rest.trim().trim_end_matches(';').trim().to_string());
            }
            if fastcgi_port.is_none()
                && let Some(rest) = line.strip_prefix("fastcgi_pass ")
            {
                fastcgi_port = rest
                    .trim()
                    .trim_end_matches(';')
                    .rsplit(':')
                    .next()
                    .and_then(|p| p.trim().parse().ok());
            }
            if proxy_port.is_none()
                && let Some(rest) = line.strip_prefix("proxy_pass ")
            {
                proxy_port = rest
                    .trim()
                    .trim_end_matches(';')
                    .trim_end_matches('/')
                    .rsplit(':')
                    .next()
                    .and_then(|p| p.trim().parse().ok());
            }
            if let Some(rest) = line.strip_prefix("listen ") {
                tls |= rest
                    .trim_end_matches(';')
                    .split_whitespace()
                    .any(|word| word == "ssl" || word.rsplit(':').next() == Some("443"));
            }
        }

        Site { host, root, fastcgi_port, proxy_port, tls }
    }

    /// Where a browser should be sent for this vhost.
    pub fn url(&self) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        format!("{scheme}://{}/", self.host)
    }
}

/// The project folder behind a vhost: its `root` resolved against the nginx
/// prefix and through the `projects` junctions to the real directory, less a
/// trailing `public` -- the folder you work in, not the one nginx serves.
pub fn folder(stack: &Stack, site: &Site) -> Result<std::path::PathBuf> {
    let root = site
        .root
        .as_deref()
        .ok_or_else(|| anyhow!("{}.conf has no root directive", site.host))?;
    let root = Path::new(root.trim_matches('"'));
    let served = if root.is_absolute() { root.to_path_buf() } else { stack.nginx_prefix.join(root) };
    let project = match (served.file_name(), served.parent()) {
        (Some(name), Some(parent)) if name.eq_ignore_ascii_case("public") => parent.to_path_buf(),
        _ => served,
    };
    crate::root::clean(&project).map_err(|_| anyhow!("{} does not exist", project.display()))
}

/// How the reload that follows a conf change went. Not an error on its own: the
/// conf is written either way, and a stopped nginx will read it on next start.
#[derive(Debug)]
pub enum Reload {
    Done,
    NginxNotRunning,
    Failed(String),
}

impl Reload {
    pub fn of(stack: &Stack) -> Reload {
        match control::reload_nginx(stack) {
            Ok(true) => Reload::Done,
            Ok(false) => Reload::NginxNotRunning,
            Err(err) => Reload::Failed(format!("{err:#}")),
        }
    }

    pub fn note(&self) -> String {
        match self {
            Reload::Done => "reloaded nginx".into(),
            Reload::NginxNotRunning => {
                "nginx is not running; it will pick this up on next start".into()
            }
            Reload::Failed(why) => format!("FAILED to reload nginx: {why}"),
        }
    }
}

/// A vhost that now exists.
#[derive(Debug)]
pub struct Created {
    pub host: String,
    pub php_name: String,
    pub port: u16,
    pub conf: String,
    pub public: String,
    pub public_existed: bool,
    pub reload: Reload,
    pub cert_name: String,
    pub hosts_updated: bool,
    pub hosts_note: Option<String>,
}

/// Detect PHP version constraint from composer.json.
pub fn detect_composer_php(project_dir: &Path) -> Option<String> {
    let composer_file = project_dir.join("composer.json");
    let text = std::fs::read_to_string(&composer_file).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let req = json.get("require")?.get("php")?.as_str()?;

    let mut version = String::new();
    for c in req.chars() {
        if c.is_ascii_digit() || c == '.' {
            version.push(c);
        } else if !version.is_empty() {
            break;
        }
    }
    if !version.is_empty() {
        Some(version)
    } else {
        None
    }
}

/// What [`create_with`] is asked for beyond the hostname.
#[derive(Debug, Default, Clone, Copy)]
pub struct Request<'a> {
    pub project_path: Option<&'a Path>,
    pub want_php: Option<&'a str>,
    pub no_hosts: bool,
    pub no_tls: bool,
    pub force: bool,
    /// Leave the project folder exactly as it is: no `public\` made in it and
    /// no placeholder `index.php`. Auto-vhost serves folders someone already
    /// put work in, and writing into them is not its business.
    pub untouched: bool,
    /// Forward to `127.0.0.1:<port>` instead of running PHP. Nothing is
    /// written into the project folder, which need not exist.
    pub proxy: Option<u16>,
}

/// The folder directly under `projects\` that `path` names, if it is one --
/// including a junction there, which is not followed.
fn folder_in_projects(stack: &Stack, path: &Path) -> Option<String> {
    let path = match path.is_absolute() {
        true => path.to_path_buf(),
        false => std::env::current_dir().ok()?.join(path),
    };
    let projects = stack.root.join("projects").canonicalize().ok()?;
    let parent = path.parent()?.canonicalize().ok()?;
    (parent == projects && path.is_dir())
        .then(|| path.file_name().map(|name| name.to_string_lossy().into_owned()))
        .flatten()
}

pub fn create_with(stack: &Stack, host: &str, request: &Request) -> Result<Created> {
    let Request { project_path, want_php, no_hosts, no_tls, force, untouched, proxy } = *request;
    let untouched = untouched || proxy.is_some();
    let host = check_host(host)?;

    // Checked before anything is created: a path that does not resolve would
    // otherwise become a junction to nowhere under projects\, which then
    // blocks the next attempt with "already exists".
    //
    // A folder already directly under projects\ is served where it is, by
    // its own name: no junction beside it. That is every auto-vhost, and it
    // keeps `--path projects\<host>` from deleting the very folder it names
    // to make room for a junction to it.
    let (project_dir, folder, in_tree) = if let Some(path) = project_path {
        let abs = path.canonicalize().map_err(|_| {
            let cwd = std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default();
            anyhow!(
                "{} does not exist. A relative path is taken from the current \
                 directory ({cwd}), not the stack root.",
                path.display()
            )
        })?;
        if !abs.is_dir() {
            return Err(anyhow!("{} is not a directory", path.display()));
        }
        match folder_in_projects(stack, path) {
            Some(folder) => (abs, folder, true),
            None => (abs, host.to_string(), false),
        }
    } else {
        (stack.root.join("projects").join(host), host.to_string(), true)
    };

    // A proxy needs no PHP; anything else is served by one.
    let service = match proxy {
        Some(_) => None,
        None => {
            let php_hint = want_php.map(|s| s.to_string()).or_else(|| detect_composer_php(&project_dir));
            Some(match php_hint {
                Some(wanted) => php::find(stack, &wanted)?,
                None => default_php(stack)?,
            })
        }
    };
    let port = match (&service, proxy) {
        (_, Some(port)) => port,
        (Some(service), None) => service
            .ports
            .first()
            .copied()
            .ok_or_else(|| anyhow!("{} has no FastCGI port configured", service.id))?,
        (None, None) => unreachable!("no PHP is chosen only for a proxy"),
    };

    let conf = stack.sites_dir().join(format!("{host}.conf"));
    if conf.exists() && !force {
        return Err(anyhow!(
            "{} already exists; pass --force to overwrite it",
            stack.rel(&conf)
        ));
    }

    let target_projects_dir = stack.root.join("projects").join(&folder);
    if !in_tree {
        if target_projects_dir.exists() {
            let _ = std::fs::remove_file(&target_projects_dir);
            let _ = std::fs::remove_dir_all(&target_projects_dir);
        }
        crate::junction::create(&target_projects_dir, &project_dir)
            .with_context(|| format!("creating junction for {}", target_projects_dir.display()))?;
    }

    let (public, root_rel) = if project_dir.join("public").is_dir() {
        (target_projects_dir.join("public"), format!("projects/{folder}/public"))
    } else {
        (target_projects_dir.clone(), format!("projects/{folder}"))
    };

    let public_existed = public.is_dir();
    if !public_existed && !untouched {
        std::fs::create_dir_all(&public)
            .with_context(|| format!("creating {}", public.display()))?;
    }

    let index = public.join("index.php");
    if !index.exists() && !untouched {
        std::fs::write(&index, "<?php phpinfo();\n")
            .with_context(|| format!("writing {}", index.display()))?;
    }

    let cert_name = if !no_tls {
        crate::mkcert::ensure_cert_for_host(stack, host)?
            .unwrap_or_else(|| "_wildcard.test.pem".to_string())
    } else {
        "_wildcard.test.pem".to_string()
    };

    let (hosts_updated, hosts_note) = if !no_hosts {
        match crate::hosts::add_entry(host) {
            Ok(crate::hosts::HostsResult::Updated) => (true, None),
            Ok(crate::hosts::HostsResult::Unchanged) => (true, None),
            Ok(crate::hosts::HostsResult::FallbackManual(why)) => (false, Some(why)),
            Err(err) => (false, Some(err.to_string())),
        }
    } else {
        (false, None)
    };

    let sites_dir = stack.sites_dir();
    std::fs::create_dir_all(&sites_dir)
        .with_context(|| format!("creating {}", sites_dir.display()))?;
    let text = match &service {
        Some(service) => conf_text_custom(host, &service.name, &service.id, port, &root_rel, &cert_name),
        // A proxy keeps a root only when there is a folder for it to name --
        // the dashboard opens it, and a dev server runs in it.
        None => proxy_conf_text(host, port, target_projects_dir.is_dir().then_some(root_rel.as_str()), &cert_name),
    };
    std::fs::write(&conf, text).with_context(|| format!("writing {}", conf.display()))?;

    control::ensure_projects_junction(stack)?;

    Ok(Created {
        host: host.to_string(),
        php_name: match &service {
            Some(service) => service.name.clone(),
            None => format!("proxy to localhost:{port}"),
        },
        port,
        conf: stack.rel(&conf),
        public: stack.rel(&public),
        public_existed,
        reload: Reload::of(stack),
        cert_name,
        hosts_updated,
        hosts_note,
    })
}

/// `devcrate site add`: [`create_with`], printed.
pub fn add(stack: &Stack, host: &str, request: &Request) -> Result<u8> {
    let proxy = request.proxy;
    let made = create_with(stack, host, request)?;

    let verb = if made.public_existed { "exists  " } else { "created " };
    println!("  {verb} {}", made.public);
    println!("  wrote    {}", made.conf);

    if made.hosts_updated {
        println!("  hosts    updated C:\\Windows\\System32\\drivers\\etc\\hosts");
    } else if let Some(note) = &made.hosts_note {
        println!("  hosts    failed to update: {note}");
        println!("           Add `127.0.0.1   {host}` to hosts file manually as Administrator.");
    }

    if let Reload::Failed(why) = &made.reload {
        println!("  FAILED to reload nginx: {why}");
        println!("  the vhost is written; fix the error and run `devcrate restart nginx`");
        return Ok(exit::ERROR);
    }
    println!("  {}", made.reload.note());

    println!();
    match proxy {
        Some(_) => println!("https://{} -> {}", made.host, made.php_name),
        None => println!("https://{} -> {} (fastcgi {})", made.host, made.php_name, made.port),
    }
    println!("  SSL cert: {}", made.cert_name);
    Ok(exit::OK)
}

/// Point an existing vhost at another PHP version.
///
/// This is the one command that *edits* a conf rather than writing or deleting
/// one, so it edits as little as possible: the `fastcgi_pass` port, and the
/// generated header comment when there is one to keep honest. Every other line
/// -- including anything hand-added since the file was generated -- comes
/// through byte for byte. Rewriting the whole file from a template would be
/// simpler and would silently discard exactly the customisation someone cared
/// enough to make by hand.
#[derive(Debug)]
pub struct Repointed {
    pub host: String,
    pub php_name: String,
    /// Service id, for telling the reader which worker has to be running.
    pub php_id: String,
    pub port: u16,
    pub was: Option<u16>,
    pub conf: String,
    pub reload: Reload,
    /// True when the vhost already pointed there and nothing was written.
    pub unchanged: bool,
    /// Set when the vhost was a proxy and its conf was written anew: where
    /// the old one was kept.
    pub backup: Option<String>,
}

pub fn repoint_site(stack: &Stack, host: &str, wanted: &str) -> Result<Repointed> {
    let host = check_host(host)?;
    let conf = stack.sites_dir().join(format!("{host}.conf"));
    if !conf.is_file() {
        return Err(anyhow!(
            "{} does not exist; `devcrate site add {host}` creates it",
            stack.rel(&conf)
        ));
    }

    let service = php::find(stack, wanted)?;
    let port = service
        .ports
        .first()
        .copied()
        .ok_or_else(|| anyhow!("{} has no FastCGI port configured", service.id))?;

    let site = Site::read(&conf);
    if site.proxy_port.is_some() {
        return proxy_to_php(stack, host, &conf, &site, service, port);
    }

    let was = site.fastcgi_port;
    let mut done = Repointed {
        host: host.to_string(),
        php_name: service.name.clone(),
        php_id: service.id.clone(),
        port,
        was,
        conf: stack.rel(&conf),
        reload: Reload::NginxNotRunning,
        unchanged: was == Some(port),
        backup: None,
    };
    if done.unchanged {
        return Ok(done);
    }

    let text = std::fs::read_to_string(&conf)
        .with_context(|| format!("reading {}", conf.display()))?;
    let (edited, changed) = repoint(&text, &service.name, &service.id, port);
    if changed == 0 {
        return Err(anyhow!(
            "{} has no fastcgi_pass line to change; edit it by hand",
            stack.rel(&conf)
        ));
    }

    std::fs::write(&conf, edited).with_context(|| format!("writing {}", conf.display()))?;
    done.reload = Reload::of(stack);
    Ok(done)
}

/// A vhost turned from PHP into a proxy, or the other way.
#[derive(Debug)]
pub struct Converted {
    pub host: String,
    pub conf: String,
    /// Where the old conf was kept: `<host>.conf.bak`.
    pub backup: String,
    pub reload: Reload,
}

/// Write a whole conf anew -- turning PHP into a proxy or back changes its
/// location blocks, so there is no one line to swap -- keeping the old one as
/// `<host>.conf.bak` (which `include sites/*.conf` does not read), and putting
/// it back when nginx refuses the new one.
fn rewrite_conf(stack: &Stack, conf: &Path, text: &str) -> Result<(String, Reload)> {
    let backup = conf.with_extension("conf.bak");
    std::fs::copy(conf, &backup).with_context(|| format!("keeping {}", backup.display()))?;
    std::fs::write(conf, text).with_context(|| format!("writing {}", conf.display()))?;
    let reload = Reload::of(stack);
    if let Reload::Failed(why) = &reload {
        std::fs::copy(&backup, conf).with_context(|| format!("restoring {}", conf.display()))?;
        let _ = Reload::of(stack);
        return Err(anyhow!("nginx refused the new {}: {why}\nThe old conf is back in place.", stack.rel(conf)));
    }
    Ok((stack.rel(&backup), reload))
}

/// The certificate file a conf names, by its name in `certs\`.
fn cert_name_of(text: &str) -> String {
    crate::cert::ssl_files(text)
        .0
        .and_then(|cert| Path::new(&cert).file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "_wildcard.test.pem".to_string())
}

/// `devcrate site set-proxy`: make a vhost forward to `127.0.0.1:<port>`. Its
/// root, certificate, and logs carry over.
pub fn set_proxy(stack: &Stack, host: &str, port: u16) -> Result<Option<Converted>> {
    let host = check_host(host)?;
    let conf = stack.sites_dir().join(format!("{host}.conf"));
    if !conf.is_file() {
        return Err(anyhow!("{} does not exist; `devcrate site add {host} --proxy {port}` creates it", stack.rel(&conf)));
    }
    let site = Site::read(&conf);
    if site.proxy_port == Some(port) {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&conf).with_context(|| format!("reading {}", conf.display()))?;
    let root = site.root.as_deref().map(|r| r.trim_matches('"'));
    let new = proxy_conf_text(host, port, root, &cert_name_of(&text));
    let (backup, reload) = rewrite_conf(stack, &conf, &new)?;
    Ok(Some(Converted { host: host.to_string(), conf: stack.rel(&conf), backup, reload }))
}

/// A proxy vhost back to PHP, through `set-php` or the dashboard's `p`.
fn proxy_to_php(
    stack: &Stack,
    host: &str,
    conf: &Path,
    site: &Site,
    service: &crate::config::Service,
    port: u16,
) -> Result<Repointed> {
    let root = site.root.as_deref().map(|r| r.trim_matches('"')).ok_or_else(|| {
        anyhow!(
            "{} is a proxy with no root, and PHP needs a folder to serve. \
             Remove it and add it with one: `devcrate site add {host} --path <folder>`.",
            stack.rel(conf)
        )
    })?;
    let text = std::fs::read_to_string(conf).with_context(|| format!("reading {}", conf.display()))?;
    let new = conf_text_custom(host, &service.name, &service.id, port, root, &cert_name_of(&text));
    let (backup, reload) = rewrite_conf(stack, conf, &new)?;
    Ok(Repointed {
        host: host.to_string(),
        php_name: service.name.clone(),
        php_id: service.id.clone(),
        port,
        was: None,
        conf: stack.rel(conf),
        reload,
        unchanged: false,
        backup: Some(backup),
    })
}

/// `devcrate site set-proxy`, printed.
pub fn print_set_proxy(stack: &Stack, host: &str, port: u16) -> Result<u8> {
    let Some(done) = set_proxy(stack, host, port)? else {
        println!("{host} already forwards to localhost:{port}");
        return Ok(exit::OK);
    };
    println!("  {} : now forwards to http://localhost:{port}", done.conf);
    println!("  the old conf is kept as {}", done.backup);
    println!("  {}", done.reload.note());
    println!();
    println!("https://{} -> http://localhost:{port}", done.host);
    println!("Something has to be listening there: a Node app, or `devcrate dev start {}`.", done.host);
    Ok(exit::OK)
}

pub fn set_php(stack: &Stack, host: &str, wanted: &str) -> Result<u8> {
    let done = repoint_site(stack, host, wanted)?;
    if done.unchanged {
        println!(
            "{} already serves through {} (fastcgi {})",
            done.host, done.php_name, done.port
        );
        return Ok(exit::OK);
    }

    match (&done.backup, done.was) {
        (Some(backup), _) => {
            println!("  {} : proxy -> fastcgi {}", done.conf, done.port);
            println!("  the old conf is kept as {backup}");
        }
        (None, Some(old)) => println!("  {} : fastcgi {old} -> {}", done.conf, done.port),
        (None, None) => println!("  {} : fastcgi -> {}", done.conf, done.port),
    }

    if let Reload::Failed(why) = &done.reload {
        println!("  FAILED to reload nginx: {why}");
        println!("  the change is written; fix the error and run `devcrate restart nginx`");
        return Ok(exit::ERROR);
    }
    println!("  {}", done.reload.note());

    println!();
    println!("https://{} -> {} (fastcgi {})", done.host, done.php_name, done.port);
    println!(
        "The FastCGI worker for {} has to be running: `devcrate start {}`.",
        done.php_name, done.php_id
    );
    Ok(exit::OK)
}

/// Rewrite the FastCGI port in place. Returns the new text and how many
/// `fastcgi_pass` lines were changed, so a conf with none can be reported
/// rather than silently rewritten to no effect.
///
/// Indentation and the rest of each line are preserved, and only the loopback
/// address the generator writes is touched -- a `fastcgi_pass` pointing at a
/// unix socket or another host is somebody's deliberate choice, not a port to
/// swap.
fn repoint(text: &str, php_name: &str, php_id: &str, port: u16) -> (String, usize) {
    let mut changed = 0;
    let ends_with_newline = text.ends_with('\n');

    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];

        if let Some(rest) = trimmed.strip_prefix("fastcgi_pass")
            && rest.trim_start().starts_with("127.0.0.1:")
        {
            out.push(format!("{indent}fastcgi_pass    127.0.0.1:{port};"));
            changed += 1;
        } else if trimmed.starts_with("# PHP") && trimmed.contains("->") {
            // The generated header. Keeping it in step matters because it is
            // what someone reads before `devcrate site list`.
            out.push(format!("{indent}# PHP     : {php_id} ({php_name}) -> 127.0.0.1:{port}"));
        } else {
            out.push(line.to_string());
        }
    }

    let mut text = out.join("\n");
    if ends_with_newline {
        text.push('\n');
    }
    (text, changed)
}

#[derive(Debug)]
pub struct Deleted {
    pub host: String,
    pub conf: String,
    /// The project folder the conf served, as its `root` named it, less a
    /// trailing `public` -- not necessarily `projects\<host>`.
    pub folder: String,
    /// The folder's name when auto-vhost is on and the folder is still under
    /// `projects\`: the next sync will serve it again unless it is ignored.
    pub auto_returns: Option<String>,
    pub reload: Reload,
    /// The host was taken out of the devcrate hosts block.
    pub hosts_removed: bool,
    /// Why the hosts block could not be updated, when it held the host but the
    /// write failed or elevation was declined.
    pub hosts_note: Option<String>,
}

/// Delete a vhost's conf, take the host out of the devcrate hosts block, and
/// reload. The project folder and the certificate are never touched -- the
/// certificate may be a wildcard another site still uses.
pub fn delete(stack: &Stack, host: &str) -> Result<Deleted> {
    let host = check_host(host)?;
    let conf = stack.sites_dir().join(format!("{host}.conf"));
    if !conf.is_file() {
        return Err(anyhow!("{} does not exist", stack.rel(&conf)));
    }

    // Read before the conf goes: it is the only record of which folder it
    // served.
    let site = Site::read(&conf);
    let served = site.root.as_deref().map(|root| {
        let root = root.trim_matches('"');
        root.strip_suffix("/public").unwrap_or(root).replace('/', "\\")
    });
    let folder = served.clone().unwrap_or_else(|| format!("projects\\{host}"));
    let auto_returns = served
        .as_deref()
        .and_then(|s| s.strip_prefix("projects\\"))
        .filter(|name| {
            stack.auto_sites
                && !name.contains('\\')
                && !stack.auto_sites_ignore.iter().any(|i| i.eq_ignore_ascii_case(name))
                && stack.root.join("projects").join(name).is_dir()
        })
        .map(str::to_string);

    std::fs::remove_file(&conf).with_context(|| format!("removing {}", conf.display()))?;

    // Here rather than in `remove`, so the dashboard's delete does it too.
    let (hosts_removed, hosts_note) = match crate::hosts::remove_entry(host) {
        Ok(crate::hosts::HostsResult::Updated) => (true, None),
        Ok(crate::hosts::HostsResult::Unchanged) => (false, None),
        Ok(crate::hosts::HostsResult::FallbackManual(why)) => (false, Some(why)),
        Err(err) => (false, Some(format!("{err:#}"))),
    };

    Ok(Deleted {
        host: host.to_string(),
        conf: stack.rel(&conf),
        folder,
        auto_returns,
        reload: Reload::of(stack),
        hosts_removed,
        hosts_note,
    })
}

pub fn remove(stack: &Stack, host: &str) -> Result<u8> {
    let gone = delete(stack, host)?;
    println!("  removed  {}", gone.conf);
    println!("  {}", gone.reload.note());

    if gone.hosts_removed {
        println!("  hosts    removed {} from C:\\Windows\\System32\\drivers\\etc\\hosts", gone.host);
    } else if let Some(note) = &gone.hosts_note {
        println!("  hosts    failed to update: {note}");
        println!("           Remove `127.0.0.1   {}` from the hosts file manually as Administrator.", gone.host);
    }

    println!();
    println!("The project folder {} was left alone.", gone.folder);
    if gone.auto_returns.is_some() {
        println!("Auto-vhost will serve it again on the next sync; add its name to [sites] ignore in devcrate.toml to stop that.");
    }
    Ok(exit::OK)
}

/// A hostname has to survive being pasted into a file path and an nginx
/// `server_name` without meaning something else.
fn check_host(host: &str) -> Result<&str> {
    let host = host.trim();
    if host.is_empty() {
        return Err(anyhow!("no hostname given"));
    }
    let bad = |c: char| c == '/' || c == '\\' || c == ':' || c.is_whitespace();
    if host.contains(bad) || host.starts_with('.') || host.ends_with('.') {
        return Err(anyhow!("{host:?} is not a usable hostname"));
    }
    Ok(host)
}

/// `api.mygroup.test` -> `mygroup.test`, the domain a wildcard has to cover.
#[allow(dead_code)]
pub fn parent_domain(host: &str) -> &str {
    crate::mkcert::parent_domain(host)
}

/// Default to whatever the CLI resolves to, so `site add myapp.test` picks the
/// version already in use rather than guessing.
pub fn default_php(stack: &Stack) -> Result<&crate::config::Service> {
    let current = stack
        .current_php()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
    match current {
        Some(tag) => php::find(stack, &tag),
        None => Err(anyhow!(
            "no --php given and php\\current is not set; pass --php 8.5 (or run `devcrate php use 8.5`)"
        )),
    }
}

/// A vhost that forwards everything to `localhost:<port>` -- a Node app, a
/// dev server -- over HTTP/1.1 with WebSocket upgrades passed through, so hot
/// reload and sockets work behind `https://<host>`.
///
/// `localhost`, not `127.0.0.1`: Vite and other Node servers listen on `::1`
/// alone by default on Windows, and nginx resolves `localhost` to both
/// addresses and moves on to the next when one refuses -- measured: a
/// `127.0.0.1` upstream answered 502 for a default Vite, `localhost` 200. The `map` that turns an
/// Upgrade header into `Connection: upgrade` is named after the host, so two
/// proxy vhosts never declare the same variable; site confs are included
/// inside nginx.conf's `http` block, where `map` belongs.
pub fn proxy_conf_text(host: &str, port: u16, root_rel: Option<&str>, cert_name: &str) -> String {
    let key_name = cert_name.replace(".pem", "-key.pem");
    let upgrade: String =
        host.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    let root = match root_rel {
        Some(root) => format!("    root   {root};\n\n"),
        None => String::new(),
    };
    format!(
        "# Auto-generated by devcrate site add --proxy\n\
         # Domain  : {host}\n\
         # Proxy   : http://localhost:{port}\n\
         # Cert    : {cert_name}\n\
         # Paths: root/logs are prefix-relative, certs are conf-relative.\n\
         \n\
         map $http_upgrade $devcrate_upgrade_{upgrade} {{\n\
         \x20   default  upgrade;\n\
         \x20   ''       close;\n\
         }}\n\
         \n\
         server {{\n\
         \x20   listen       80;\n\
         \x20   server_name  {host};\n\
         \x20   return 301   https://$host$request_uri;\n\
         }}\n\
         \n\
         server {{\n\
         \x20   listen       443 ssl;\n\
         \x20   server_name  {host};\n\
         \n\
         {root}\
         \x20   ssl_certificate      certs/{cert_name};\n\
         \x20   ssl_certificate_key  certs/{key_name};\n\
         \n\
         \x20   access_log  logs/{host}.access.log  main;\n\
         \x20   error_log   logs/{host}.error.log   warn;\n\
         \n\
         \x20   location / {{\n\
         \x20       proxy_pass          http://localhost:{port};\n\
         \x20       proxy_http_version  1.1;\n\
         \x20       proxy_set_header    Host               $host;\n\
         \x20       proxy_set_header    X-Real-IP          $remote_addr;\n\
         \x20       proxy_set_header    X-Forwarded-For    $proxy_add_x_forwarded_for;\n\
         \x20       proxy_set_header    X-Forwarded-Proto  https;\n\
         \x20       proxy_set_header    Upgrade            $http_upgrade;\n\
         \x20       proxy_set_header    Connection         $devcrate_upgrade_{upgrade};\n\
         \x20       proxy_read_timeout  1h;\n\
         \x20   }}\n\
         }}\n"
    )
}

#[allow(dead_code)]
fn conf_text(host: &str, php_name: &str, php_id: &str, port: u16) -> String {
    conf_text_custom(
        host,
        php_name,
        php_id,
        port,
        &format!("projects/{host}/public"),
        "_wildcard.test.pem",
    )
}

fn conf_text_custom(
    host: &str,
    php_name: &str,
    php_id: &str,
    port: u16,
    root_rel: &str,
    cert_name: &str,
) -> String {
    let key_name = cert_name.replace(".pem", "-key.pem");
    format!(
        "# Auto-generated by devcrate site add\n\
         # Domain  : {host}\n\
         # PHP     : {php_id} ({php_name}) -> 127.0.0.1:{port}\n\
         # Cert    : {cert_name}\n\
         # Paths: root/logs are prefix-relative, certs are conf-relative.\n\
         \n\
         server {{\n\
         \x20   listen       80;\n\
         \x20   server_name  {host};\n\
         \x20   return 301   https://$host$request_uri;\n\
         }}\n\
         \n\
         server {{\n\
         \x20   listen       443 ssl;\n\
         \x20   server_name  {host};\n\
         \n\
         \x20   root   {root_rel};\n\
         \x20   index  index.php index.html;\n\
         \n\
         \x20   ssl_certificate      certs/{cert_name};\n\
         \x20   ssl_certificate_key  certs/{key_name};\n\
         \n\
         \x20   access_log  logs/{host}.access.log  main;\n\
         \x20   error_log   logs/{host}.error.log   warn;\n\
         \n\
         \x20   location / {{\n\
         \x20       try_files $uri $uri/ /index.php?$query_string;\n\
         \x20   }}\n\
         \n\
         \x20   location ~ \\.php$ {{\n\
         \x20       try_files       $uri =404;\n\
         \x20       fastcgi_pass    127.0.0.1:{port};\n\
         \x20       fastcgi_index   index.php;\n\
         \x20       include         fastcgi_params;\n\
         \x20       fastcgi_param   SCRIPT_FILENAME  $document_root$fastcgi_script_name;\n\
         \x20       fastcgi_param   HTTPS            on;\n\
         \x20   }}\n\
         \n\
         \x20   location ~ /\\. {{\n\
         \x20       deny all;\n\
         \x20   }}\n\
         }}\n"
    )
}

#[cfg(test)]
mod tests {
    use crate::root::{RootSource, StackRoot};

    #[test]
    fn a_proxy_conf_forwards_upgrades_under_its_own_map() {
        let conf = proxy_conf_text("api.shop.test", 3000, Some("projects/shop"), "_wildcard.shop.test.pem");
        assert!(conf.contains("map $http_upgrade $devcrate_upgrade_api_shop_test {"), "{conf}");
        assert!(conf.contains("proxy_pass          http://localhost:3000;"));
        assert!(conf.contains("proxy_set_header    Connection         $devcrate_upgrade_api_shop_test;"));
        assert!(conf.contains("proxy_set_header    X-Forwarded-Proto  https;"));
        assert!(conf.contains("    root   projects/shop;\n"));
        assert!(conf.contains("ssl_certificate_key  certs/_wildcard.shop.test-key.pem;"));
        assert!(!proxy_conf_text("api.shop.test", 3000, None, "x.pem").contains("root "), "no folder, no root");

        let path = std::env::temp_dir().join(format!("devcrate-proxy-{}.conf", std::process::id()));
        std::fs::write(&path, &conf).unwrap();
        let site = Site::read(&path);
        assert_eq!((site.proxy_port, site.fastcgi_port, site.tls), (Some(3000), None, true));
        let _ = std::fs::remove_file(path);
    }

    /// PHP to proxy and back: the conf is written anew each way, the old one
    /// kept as .conf.bak, and the root and certificate carry over. nginx is not
    /// running here, so nothing is reloaded or tested.
    #[test]
    fn a_vhost_turns_into_a_proxy_and_back() {
        let root = std::env::temp_dir().join(format!("devcrate-convert-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("nginx").join("conf").join("sites")).unwrap();
        std::fs::write(root.join("nginx").join("conf").join("nginx.conf"), "").unwrap();
        let php = root.join("php").join("php-8.5");
        std::fs::create_dir_all(&php).unwrap();
        std::fs::write(php.join("php-cgi.exe"), "").unwrap();
        let stack = Stack::open(StackRoot { path: root.clone(), source: RootSource::Flag }).unwrap();
        let conf = stack.sites_dir().join("shop.test.conf");
        std::fs::write(
            &conf,
            conf_text_custom("shop.test", "PHP 8.5", "php-8.5", 9085, "projects/shop/public", "_wildcard.shop.test.pem"),
        )
        .unwrap();

        let done = set_proxy(&stack, "shop.test", 5173).unwrap().expect("converted");
        assert!(done.backup.ends_with("shop.test.conf.bak"), "{}", done.backup);
        assert!(std::fs::read_to_string(stack.sites_dir().join("shop.test.conf.bak")).unwrap().contains("fastcgi_pass"));
        let site = Site::read(&conf);
        assert_eq!(site.proxy_port, Some(5173));
        assert_eq!(site.root.as_deref(), Some("projects/shop/public"), "the root carries over");
        assert!(std::fs::read_to_string(&conf).unwrap().contains("certs/_wildcard.shop.test.pem;"));
        assert!(set_proxy(&stack, "shop.test", 5173).unwrap().is_none(), "already there");
        assert_eq!(stack.sites().len(), 1, "the .bak is not a site");

        let back = repoint_site(&stack, "shop.test", "8.5").unwrap();
        assert!(back.backup.is_some());
        let site = Site::read(&conf);
        assert_eq!((site.proxy_port, site.fastcgi_port), (None, Some(9085)));
        assert_eq!(site.root.as_deref(), Some("projects/shop/public"));
        let _ = std::fs::remove_dir_all(&root);
    }
    use super::*;

    #[test]
    fn pulls_root_and_fastcgi_port_from_a_generated_conf() {
        let dir = std::env::temp_dir().join("devcrate-site-test");
        std::fs::create_dir_all(&dir).unwrap();
        let conf = dir.join("myapp.test.conf");
        std::fs::write(
            &conf,
            "# Auto-generated by new-vhost.bat\n\
             server {\n\
             \x20   root   projects/myapp.test/public;\n\
             \x20   fastcgi_pass    127.0.0.1:9085;\n\
             }\n",
        )
        .unwrap();

        let site = Site::read(&conf);
        assert_eq!(site.host, "myapp.test");
        assert_eq!(site.root.as_deref(), Some("projects/myapp.test/public"));
        assert_eq!(site.fastcgi_port, Some(9085));

        std::fs::remove_file(&conf).unwrap();
    }

    /// The conf we write has to be readable by the scanner that reads the ones
    /// the batch script wrote -- they are the same format on purpose.
    #[test]
    fn generated_confs_round_trip_through_the_reader() {
        let dir = std::env::temp_dir().join("devcrate-site-roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let conf = dir.join("myapp.test.conf");
        std::fs::write(&conf, conf_text("myapp.test", "PHP 8.5", "php-8.5", 9085)).unwrap();

        let site = Site::read(&conf);
        assert_eq!(site.host, "myapp.test");
        assert_eq!(site.root.as_deref(), Some("projects/myapp.test/public"));
        assert_eq!(site.fastcgi_port, Some(9085));

        std::fs::remove_file(&conf).unwrap();
    }

    /// A `--path` that does not exist is refused before anything is made --
    /// it used to leave a junction to nowhere under projects\ that blocked
    /// every retry with "already exists".
    #[test]
    fn a_project_path_that_does_not_exist_creates_nothing() {
        let root = std::env::temp_dir().join(format!("devcrate-site-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("projects")).unwrap();
        let stack = Stack::open(crate::root::StackRoot {
            path: root.clone(),
            source: crate::root::RootSource::Flag,
        })
        .unwrap();

        let missing = root.join("no-such-folder");
        let request = Request { project_path: Some(&missing), no_hosts: true, no_tls: true, ..Default::default() };
        let err = create_with(&stack, "db.test", &request)
            .err()
            .expect("refused")
            .to_string();
        assert!(err.contains("does not exist"), "{err}");
        assert!(!root.join("projects").join("db.test").exists(), "no junction left behind");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The generated conf serves TLS; a hand-written plain-HTTP one does not,
    /// and the browser is sent to whichever it is.
    #[test]
    fn the_url_follows_the_listen_directives() {
        let dir = std::env::temp_dir().join(format!("devcrate-site-url-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let generated = dir.join("myapp.test.conf");
        std::fs::write(&generated, conf_text("myapp.test", "PHP 8.5", "php-8.5", 9085)).unwrap();
        assert_eq!(Site::read(&generated).url(), "https://myapp.test/");

        let plain = dir.join("docs.test.conf");
        std::fs::write(&plain, "server {\n    listen 80;\n    # listen 443 ssl;\n    root projects/docs.test;\n}\n").unwrap();
        assert_eq!(Site::read(&plain).url(), "http://docs.test/");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `root` is followed through both junctions -- `nginx\projects` and the
    /// one `--path` makes -- to the real folder, without its `public`.
    #[test]
    fn the_folder_is_the_real_project_directory() {
        let root = std::env::temp_dir().join(format!("devcrate-site-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("projects").join("myapp.test").join("public")).unwrap();
        std::fs::create_dir_all(root.join("elsewhere").join("api")).unwrap();
        std::fs::create_dir_all(root.join("nginx").join("conf")).unwrap();
        std::fs::write(root.join("nginx").join("conf").join("nginx.conf"), "").unwrap();
        crate::junction::create(&root.join("nginx").join("projects"), &root.join("projects")).unwrap();
        crate::junction::create(&root.join("projects").join("api.test"), &root.join("elsewhere").join("api"))
            .unwrap();
        let stack = Stack::open(crate::root::StackRoot {
            path: root.clone(),
            source: crate::root::RootSource::Flag,
        })
        .unwrap();
        let site = |host: &str, root: &str| Site {
            host: host.into(),
            root: Some(root.into()),
            fastcgi_port: None,
            proxy_port: None,
            tls: true,
        };
        let real = crate::root::clean(&root).unwrap();

        let found = folder(&stack, &site("myapp.test", "projects/myapp.test/public")).unwrap();
        assert_eq!(found, real.join("projects").join("myapp.test"));
        let found = folder(&stack, &site("api.test", "projects/api.test")).unwrap();
        assert_eq!(found, real.join("elsewhere").join("api"));
        let err = folder(&stack, &site("gone.test", "projects/gone.test/public")).unwrap_err();
        assert!(err.to_string().contains("does not exist"), "{err}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hostnames_that_would_escape_the_sites_directory_are_refused() {
        assert!(check_host("myapp.test").is_ok());
        assert!(check_host("../../etc/passwd").is_err());
        assert!(check_host("a\\b").is_err());
        assert!(check_host("has space.test").is_err());
        assert!(check_host("").is_err());
    }

    #[test]
    fn wildcard_advice_targets_the_parent_domain() {
        assert_eq!(parent_domain("api.mygroup.test"), "mygroup.test");
        assert_eq!(parent_domain("myapp.test"), "myapp.test");
    }

    /// The whole point of editing rather than regenerating: everything the
    /// generator did not write has to survive.
    #[test]
    fn changing_the_php_version_leaves_the_rest_of_the_conf_alone() {
        let original = conf_text("myapp.test", "PHP 8.5", "php-8.5", 9085)
            .replace("    index  index.php index.html;", "    index  index.php;\n    client_max_body_size 64m;   # added by hand");

        let (edited, changed) = repoint(&original, "PHP 7.4", "php-7.4", 9074);
        assert_eq!(changed, 1);
        assert!(edited.contains("fastcgi_pass    127.0.0.1:9074;"));
        assert!(!edited.contains("9085"));
        assert!(edited.contains("client_max_body_size 64m;   # added by hand"));
        assert!(edited.contains("# PHP     : php-7.4 (PHP 7.4) -> 127.0.0.1:9074"));

        // ...and the result is still readable by the scanner.
        let dir = std::env::temp_dir().join("devcrate-site-setphp");
        std::fs::create_dir_all(&dir).unwrap();
        let conf = dir.join("myapp.test.conf");
        std::fs::write(&conf, &edited).unwrap();
        assert_eq!(Site::read(&conf).fastcgi_port, Some(9074));
        std::fs::remove_file(&conf).unwrap();
    }

    /// A conf with nothing to repoint is reported, not quietly rewritten.
    #[test]
    fn a_conf_without_a_fastcgi_pass_reports_no_change() {
        let static_site = "server {\n    listen 80;\n    root projects/docs/public;\n}\n";
        let (edited, changed) = repoint(static_site, "PHP 8.5", "php-8.5", 9085);
        assert_eq!(changed, 0);
        assert_eq!(edited, static_site);
    }

    #[test]
    fn only_loopback_fastcgi_passes_are_repointed() {
        let remote = "server {\n    fastcgi_pass   backend.internal:9000;\n}\n";
        let (edited, changed) = repoint(remote, "PHP 8.5", "php-8.5", 9085);
        assert_eq!(changed, 0);
        assert_eq!(edited, remote);
    }

    #[test]
    fn test_detect_composer_php() {
        let dir = std::env::temp_dir().join("devcrate-site-composer");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("composer.json"), r#"{"require": {"php": "^8.2"}}"#).unwrap();

        assert_eq!(detect_composer_php(&dir).as_deref(), Some("8.2"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
