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
        }

        Site { host, root, fastcgi_port }
    }
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
    fn of(stack: &Stack) -> Reload {
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

/// Create a vhost: web root, conf, junction, reload.
pub fn create(
    stack: &Stack,
    host: &str,
    project_path: Option<&Path>,
    want_php: Option<&str>,
    no_hosts: bool,
    no_tls: bool,
    force: bool,
) -> Result<Created> {
    let host = check_host(host)?;

    let (project_dir, in_tree) = if let Some(path) = project_path {
        let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        (abs, false)
    } else {
        (stack.root.join("projects").join(host), true)
    };

    let php_hint = want_php
        .map(|s| s.to_string())
        .or_else(|| detect_composer_php(&project_dir));

    let service = match php_hint {
        Some(wanted) => php::find(stack, &wanted)?,
        None => default_php(stack)?,
    };
    let port = service
        .ports
        .first()
        .copied()
        .ok_or_else(|| anyhow!("{} has no FastCGI port configured", service.id))?;

    let conf = stack.sites_dir().join(format!("{host}.conf"));
    if conf.exists() && !force {
        return Err(anyhow!(
            "{} already exists; pass --force to overwrite it",
            stack.rel(&conf)
        ));
    }

    let target_projects_dir = stack.root.join("projects").join(host);
    if !in_tree {
        if target_projects_dir.exists() {
            let _ = std::fs::remove_file(&target_projects_dir);
            let _ = std::fs::remove_dir_all(&target_projects_dir);
        }
        crate::junction::create(&target_projects_dir, &project_dir)
            .with_context(|| format!("creating junction for {}", target_projects_dir.display()))?;
    }

    let (public, root_rel) = if project_dir.join("public").is_dir() {
        (target_projects_dir.join("public"), format!("projects/{host}/public"))
    } else {
        (target_projects_dir.clone(), format!("projects/{host}"))
    };

    let public_existed = public.is_dir();
    if !public_existed {
        std::fs::create_dir_all(&public)
            .with_context(|| format!("creating {}", public.display()))?;
    }

    let index = public.join("index.php");
    if !index.exists() {
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
    std::fs::write(
        &conf,
        conf_text_custom(host, &service.name, &service.id, port, &root_rel, &cert_name),
    )
    .with_context(|| format!("writing {}", conf.display()))?;

    control::ensure_projects_junction(stack)?;

    Ok(Created {
        host: host.to_string(),
        php_name: service.name.clone(),
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

pub fn add(
    stack: &Stack,
    host: &str,
    project_path: Option<&Path>,
    want_php: Option<&str>,
    no_hosts: bool,
    no_tls: bool,
    force: bool,
) -> Result<u8> {
    let made = create(stack, host, project_path, want_php, no_hosts, no_tls, force)?;

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
    println!("https://{} -> {} (fastcgi {})", made.host, made.php_name, made.port);
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

    let was = Site::read(&conf).fastcgi_port;
    let mut done = Repointed {
        host: host.to_string(),
        php_name: service.name.clone(),
        php_id: service.id.clone(),
        port,
        was,
        conf: stack.rel(&conf),
        reload: Reload::NginxNotRunning,
        unchanged: was == Some(port),
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

pub fn set_php(stack: &Stack, host: &str, wanted: &str) -> Result<u8> {
    let done = repoint_site(stack, host, wanted)?;
    if done.unchanged {
        println!(
            "{} already serves through {} (fastcgi {})",
            done.host, done.php_name, done.port
        );
        return Ok(exit::OK);
    }

    match done.was {
        Some(old) => println!("  {} : fastcgi {old} -> {}", done.conf, done.port),
        None => println!("  {} : fastcgi -> {}", done.conf, done.port),
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
    pub reload: Reload,
}

/// Delete a vhost's conf and reload. The project folder is never touched.
pub fn delete(stack: &Stack, host: &str) -> Result<Deleted> {
    let host = check_host(host)?;
    let conf = stack.sites_dir().join(format!("{host}.conf"));
    if !conf.is_file() {
        return Err(anyhow!("{} does not exist", stack.rel(&conf)));
    }

    std::fs::remove_file(&conf).with_context(|| format!("removing {}", conf.display()))?;
    Ok(Deleted {
        host: host.to_string(),
        conf: stack.rel(&conf),
        reload: Reload::of(stack),
    })
}

pub fn remove(stack: &Stack, host: &str) -> Result<u8> {
    let gone = delete(stack, host)?;
    println!("  removed  {}", gone.conf);
    println!("  {}", gone.reload.note());

    if let Ok(crate::hosts::HostsResult::Updated) = crate::hosts::remove_entry(&gone.host) {
        println!("  hosts    removed {} from C:\\Windows\\System32\\drivers\\etc\\hosts", gone.host);
    }

    println!();
    println!("The project folder under projects\\{} was left alone.", gone.host);
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
fn default_php<'a>(stack: &'a Stack) -> Result<&'a crate::config::Service> {
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
