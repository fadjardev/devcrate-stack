//! Placing Adminer: one verified PHP file, and the `index.php` devcrate serves
//! it through.
//!
//! A tool like Composer, not a runtime: one file in `adminer\`, replaced on
//! update rather than kept side by side. It runs under the stack's own PHP, as
//! an ordinary site (`devcrate site add db.test --path adminer`), so there is
//! no service to start and no port of its own -- the database browser is a
//! page in the browser, which is how the stack meets "visual" without a GUI.
//!
//! The wrapper is the part that is devcrate's. Adminer refuses to log in
//! without a password, and the stack's MariaDB `root` has none, so the
//! wrapper overrides `login()` to let anyone in -- which is only acceptable
//! because the same wrapper first refuses every request that is not from this
//! machine. nginx listens on all interfaces, so without that check any device
//! on the network that guessed the hostname would reach a database admin with
//! no password. The check is in PHP rather than the vhost so it holds however
//! the site is served.

use std::path::Path;

use anyhow::{Context, Result, bail};

/// The hostname the install suggests serving Adminer as.
pub const SITE: &str = "db.test";

/// `adminer-6.1.2.php` -> `6.1.2`. Only the full build: the `-en` and `-mysql`
/// variants drop languages or drivers the stack wants.
pub fn version_from_file_name(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".php")?;
    let version = stem.strip_prefix("adminer-")?;
    let dotted = !version.is_empty()
        && version.split('.').all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    dotted.then(|| version.to_string())
}

/// Is this file Adminer, rather than an error page or something else saved
/// under its name? Its namespace is the cheapest unmistakable mark.
pub fn check(file: &Path) -> Result<()> {
    let bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    if !bytes.starts_with(b"<?php") || !declares_namespace(&bytes) {
        bail!("{} is not an Adminer build (no `namespace Adminer`)", file.display());
    }
    Ok(())
}

/// `namespace Adminer;` with any whitespace between the two words -- the
/// release build is minified, and puts a line break there.
fn declares_namespace(bytes: &[u8]) -> bool {
    bytes.windows(b"namespace".len()).enumerate().any(|(at, word)| {
        word == b"namespace" && {
            let rest = &bytes[at + word.len()..];
            let gap = rest.iter().take_while(|b| b.is_ascii_whitespace()).count();
            gap > 0 && rest[gap..].starts_with(b"Adminer;")
        }
    })
}

/// The page devcrate serves Adminer through. Rewritten on every install; edit
/// `adminer.php`'s customisation here rather than by hand, or lose it.
pub const WRAPPER: &str = r#"<?php
// Written by `devcrate install adminer`, and rewritten by every install.
//
// Local development only. Adminer refuses to log in without a password and the
// stack's MariaDB root has none, so this lets anyone in -- which is why it first
// refuses anything that is not this machine: nginx listens on every interface.
if (!in_array($_SERVER['REMOTE_ADDR'] ?? '', ['127.0.0.1', '::1'], true)) {
    http_response_code(403);
    exit("Adminer is served to this machine only.\n");
}

function adminer_object() {
    class DevcrateAdminer extends Adminer\Adminer {
        function login($login, $password) {
            return true;
        }
    }
    return new DevcrateAdminer;
}

include __DIR__ . '/adminer.php';
"#;

pub fn write_wrapper(dir: &Path) -> Result<()> {
    let path = dir.join("index.php");
    std::fs::write(&path, WRAPPER).with_context(|| format!("writing {}", path.display()))
}

/// Which of Adminer's drivers for the stack's databases a `php.ini` enables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Drivers {
    /// `mysqli` or `pdo_mysql`.
    pub mariadb: bool,
    /// `pgsql` or `pdo_pgsql`.
    pub postgres: bool,
}

/// Read the enabled `extension=` lines of a `php.ini`, in any of the spellings
/// the stack's files use (`mysqli`, `php_mysqli.dll`, with or without spaces).
pub fn drivers(php_ini: &str) -> Drivers {
    let enabled: Vec<String> = php_ini
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with(';'))
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim().eq_ignore_ascii_case("extension")).then(|| {
                let value = value.trim().trim_matches('"').to_ascii_lowercase();
                let value = value.strip_prefix("php_").unwrap_or(&value);
                value.strip_suffix(".dll").unwrap_or(value).to_string()
            })
        })
        .collect();
    let on = |names: &[&str]| names.iter().any(|n| enabled.iter().any(|e| e == n));
    Drivers { mariadb: on(&["mysqli", "pdo_mysql"]), postgres: on(&["pgsql", "pdo_pgsql"]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_full_build_names_a_version() {
        assert_eq!(version_from_file_name("adminer-6.1.2.php").as_deref(), Some("6.1.2"));
        assert_eq!(version_from_file_name("Adminer-6.1.2.PHP").as_deref(), Some("6.1.2"));
        assert_eq!(version_from_file_name("adminer-6.1.2-en.php"), None);
        assert_eq!(version_from_file_name("adminer-6.1.2-mysql.php"), None);
        assert_eq!(version_from_file_name("adminer.php"), None);
    }

    /// The stack's own php.ini files spell extensions three ways.
    #[test]
    fn drivers_read_every_spelling_and_skip_comments() {
        let php56 = "extension=php_mysqli.dll\n;extension=php_pgsql.dll\n";
        assert_eq!(drivers(php56), Drivers { mariadb: true, postgres: false });

        let php82 = "extension = pdo_mysql\nextension = mysqli\n";
        assert_eq!(drivers(php82), Drivers { mariadb: true, postgres: false });

        let php85 = "; extension=mysqli\nextension=pdo_mysql\n;extension=pdo_pgsql\nextension=pgsql\n";
        assert_eq!(drivers(php85), Drivers { mariadb: true, postgres: true });

        assert_eq!(drivers(""), Drivers { mariadb: false, postgres: false });
    }

    /// The release build breaks the line inside `namespace Adminer;`.
    #[test]
    fn the_namespace_is_found_across_a_line_break() {
        assert!(declares_namespace(b"<?php /**/ namespace\nAdminer;if(1){}"));
        assert!(declares_namespace(b"<?php namespace Adminer;"));
        assert!(!declares_namespace(b"<?php namespace Other;"));
        assert!(!declares_namespace(b"<?php namespaceAdminer;"));
    }

    /// The wrapper refuses other machines *before* it relaxes the login.
    #[test]
    fn the_wrapper_checks_the_address_before_anything_else() {
        let guard = WRAPPER.find("REMOTE_ADDR").unwrap();
        let login = WRAPPER.find("function login").unwrap();
        let include = WRAPPER.find("include").unwrap();
        assert!(guard < login && login < include);
    }
}
