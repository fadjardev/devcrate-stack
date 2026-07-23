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

/// Create a vhost: web root, conf, junction, reload. What `new-vhost.bat` does.
pub fn add(stack: &Stack, host: &str, want_php: Option<&str>, force: bool) -> Result<u8> {
    let host = check_host(host)?;

    // Resolve the PHP version through the same matcher `php use` uses, so
    // `--php 8.5`, `--php 85`, and `--php php85` all work -- new-vhost.bat
    // accepts only `php85`, against a port map hard-coded in the script.
    let service = match want_php {
        Some(wanted) => php::find(stack, wanted)?,
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

    let public = stack.root.join("projects").join(host).join("public");
    if public.is_dir() {
        println!("  exists   {}", stack.rel(&public));
    } else {
        std::fs::create_dir_all(&public)
            .with_context(|| format!("creating {}", public.display()))?;
        println!("  created  {}", stack.rel(&public));
    }

    let index = public.join("index.php");
    if !index.exists() {
        std::fs::write(&index, "<?php phpinfo();\n")
            .with_context(|| format!("writing {}", index.display()))?;
        println!("  created  {}", stack.rel(&index));
    }

    let sites_dir = stack.sites_dir();
    std::fs::create_dir_all(&sites_dir)
        .with_context(|| format!("creating {}", sites_dir.display()))?;
    std::fs::write(&conf, conf_text(host, &service.name, &service.id, port))
        .with_context(|| format!("writing {}", conf.display()))?;
    println!("  wrote    {}", stack.rel(&conf));

    // The conf's `root projects/<host>/public` resolves through this.
    control::ensure_projects_junction(stack)?;

    match control::reload_nginx(stack) {
        Ok(true) => println!("  reloaded nginx"),
        Ok(false) => println!("  nginx is not running; it will pick this up on next start"),
        Err(err) => {
            // The conf is written either way, so this is worth reporting
            // loudly rather than swallowing -- most likely a syntax error in
            // some other conf, which blocks the reload of all of them.
            println!("  FAILED to reload nginx: {err:#}");
            println!("  the vhost is written; fix the error and run `devcrate restart nginx`");
            return Ok(exit::ERROR);
        }
    }

    println!();
    println!("https://{host} -> {} (fastcgi {port})", service.name);
    println!();
    println!("Two manual steps remain, as with new-vhost.bat:");
    println!("  1. Add this line to C:\\Windows\\System32\\drivers\\etc\\hosts as Administrator:");
    println!("       127.0.0.1   {host}");
    if host.matches('.').count() > 1 {
        println!("  2. {host} is a third-level domain, so the *.test wildcard does not");
        println!("     cover it. Issue a cert for *.{} with mkcert and update the", parent_domain(host));
        println!("     ssl_certificate lines in the generated conf.");
    } else {
        println!("  2. Nothing else -- the existing *.test wildcard certificate covers it.");
    }
    Ok(exit::OK)
}

/// Delete a vhost's conf and reload. The project folder is never touched.
pub fn remove(stack: &Stack, host: &str) -> Result<u8> {
    let host = check_host(host)?;
    let conf = stack.sites_dir().join(format!("{host}.conf"));
    if !conf.is_file() {
        return Err(anyhow!("{} does not exist", stack.rel(&conf)));
    }

    std::fs::remove_file(&conf).with_context(|| format!("removing {}", conf.display()))?;
    println!("  removed  {}", stack.rel(&conf));

    match control::reload_nginx(stack) {
        Ok(true) => println!("  reloaded nginx"),
        Ok(false) => println!("  nginx is not running"),
        Err(err) => println!("  FAILED to reload nginx: {err:#}"),
    }

    println!();
    println!("The project folder under projects\\{host} was left alone.");
    println!("Remove the `127.0.0.1  {host}` line from your hosts file if you are done with it.");
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
fn parent_domain(host: &str) -> &str {
    host.split_once('.').map(|(_, rest)| rest).unwrap_or(host)
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

/// The conf `new-vhost.bat` writes, byte for byte in structure.
///
/// Every path in it is relative, and to two different bases: `root` and the
/// logs resolve against the nginx *prefix*, `ssl_certificate` against the
/// *conf directory*. That is nginx's rule, not a choice made here -- see
/// docs/nginx-vhosts.md.
fn conf_text(host: &str, php_name: &str, php_id: &str, port: u16) -> String {
    format!(
        "# Auto-generated by devcrate site add\n\
         # Domain  : {host}\n\
         # PHP     : {php_id} ({php_name}) -> 127.0.0.1:{port}\n\
         # Cert    : _wildcard.test  (edit below if using a sub-group domain)\n\
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
         \x20   root   projects/{host}/public;\n\
         \x20   index  index.php index.html;\n\
         \n\
         \x20   ssl_certificate      certs/_wildcard.test.pem;\n\
         \x20   ssl_certificate_key  certs/_wildcard.test-key.pem;\n\
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
        std::fs::write(&conf, conf_text("myapp.test", "PHP 8.5", "php85", 9085)).unwrap();

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
        assert_eq!(parent_domain("myapp.test"), "test");
    }
}
