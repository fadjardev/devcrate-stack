//! `devcrate cert` -- the vhosts' certificates: when each expires, whether it
//! covers its host, whether this machine's mkcert CA issued it, and issuing
//! it again when any of that is wrong.
//!
//! Every HTTPS vhost names its certificate in its conf (`ssl_certificate
//! certs/_wildcard.shop.test.pem;`, relative to `nginx\conf`). Several vhosts
//! can share one: `api.shop.test` and `www.shop.test` both use the
//! `shop.test` group's. A certificate is judged per vhost, because "does it
//! cover this host" is a question about the pair:
//!
//! - **expired**, or **expiring** within [`WARN_DAYS`];
//! - **not covering** -- the old `*.test` certificate, which browsers refuse
//!   for any host, or one issued for another group;
//! - **foreign CA** -- issued by a CA that is not this machine's mkcert CA, as
//!   happens when a stack is copied to another machine or mkcert's CA is made
//!   again. Browsers refuse it. Only judged when mkcert can name its CA.
//! - **missing** or **unreadable**.
//!
//! Renewing issues the host's group certificate with mkcert, through
//! [`crate::mkcert::issue`], points the conf's two `ssl_certificate*` lines at
//! it when they named something else, and reloads nginx once.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow};
use serde::Serialize;

use crate::config::Stack;
use crate::site::{Reload, Site};
use crate::{exit, mkcert, x509};

/// A certificate this close to expiring is renewed by `start` and flagged
/// everywhere else.
pub const WARN_DAYS: i64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CertState {
    Ok,
    Expiring,
    Expired,
    NotCovering,
    ForeignCa,
    Missing,
    Unreadable,
}

impl CertState {
    pub fn label(self) -> &'static str {
        match self {
            CertState::Ok => "ok",
            CertState::Expiring => "expiring",
            CertState::Expired => "expired",
            CertState::NotCovering => "does not cover the host",
            CertState::ForeignCa => "issued by another CA",
            CertState::Missing => "missing",
            CertState::Unreadable => "unreadable",
        }
    }
}

/// One HTTPS vhost's certificate, judged.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SiteCert {
    pub host: String,
    /// As the conf names it: `certs/_wildcard.shop.test.pem`.
    pub file: String,
    pub state: CertState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub days_left: Option<i64>,
    /// `YYYY-MM-DD`, UTC.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
}

impl SiteCert {
    pub fn needs_renewal(&self) -> bool {
        self.state != CertState::Ok
    }
}

/// The `ssl_certificate` and `ssl_certificate_key` values of a conf.
pub fn ssl_files(conf_text: &str) -> (Option<String>, Option<String>) {
    let value = |directive: &str| {
        conf_text.lines().find_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix(directive)?;
            rest.starts_with(char::is_whitespace)
                .then(|| rest.trim().trim_end_matches(';').trim().trim_matches('"').to_string())
        })
    };
    (value("ssl_certificate"), value("ssl_certificate_key"))
}

/// The subject of this machine's mkcert CA, read once per process: `None` when
/// mkcert or its CA cannot be found, and then nothing is called foreign.
fn ca_subject(stack: &Stack) -> Option<&'static Vec<u8>> {
    static CA: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    CA.get_or_init(|| {
        let bin = mkcert::find_mkcert(stack)?;
        let out = std::process::Command::new(bin)
            .arg("-CAROOT")
            .stdin(std::process::Stdio::null())
            .output()
            .ok()?;
        let dir = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        let text = std::fs::read_to_string(dir.join("rootCA.pem")).ok()?;
        x509::parse_pem(&text).ok().map(|ca| ca.subject)
    })
    .as_ref()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Judge one certificate for one host.
pub fn judge(host: &str, file: &str, path: &Path, ca: Option<&[u8]>, now: i64) -> SiteCert {
    let mut site = SiteCert {
        host: host.to_string(),
        file: file.to_string(),
        state: CertState::Ok,
        days_left: None,
        expires: None,
        names: Vec::new(),
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        site.state = CertState::Missing;
        return site;
    };
    let Ok(cert) = x509::parse_pem(&text) else {
        site.state = CertState::Unreadable;
        return site;
    };
    let days = (cert.not_after - now).div_euclid(86_400);
    site.days_left = Some(days);
    site.expires = Some(civil_date(cert.not_after));
    site.names = cert.names.clone();
    site.state = if cert.not_after <= now {
        CertState::Expired
    } else if !x509::covers(&cert.names, host) {
        CertState::NotCovering
    } else if ca.is_some_and(|ca| ca != cert.issuer.as_slice()) {
        CertState::ForeignCa
    } else if days <= WARN_DAYS {
        CertState::Expiring
    } else {
        CertState::Ok
    };
    site
}

fn civil_date(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    // Inverse of x509::days_from_civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Every HTTPS vhost's certificate, by host.
pub fn inspect(stack: &Stack) -> Vec<SiteCert> {
    let ca = ca_subject(stack).map(Vec::as_slice);
    let now = now();
    let conf_dir = stack.nginx_prefix.join("conf");
    stack
        .sites()
        .iter()
        .filter_map(|conf| {
            let text = std::fs::read_to_string(conf).ok()?;
            let (cert, _) = ssl_files(&text);
            let cert = cert?;
            let host = Site::read(conf).host;
            let path = match Path::new(&cert).is_absolute() {
                true => PathBuf::from(&cert),
                false => conf_dir.join(&cert),
            };
            Some(judge(&host, &cert, &path, ca, now))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Renewing
// ---------------------------------------------------------------------------

/// One group certificate issued again, and the vhosts it serves.
#[derive(Debug)]
pub struct Renewed {
    /// `_wildcard.shop.test.pem`.
    pub file: String,
    pub hosts: Vec<String>,
    /// Vhosts whose conf named another certificate and now names this one.
    pub repointed: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Renewal {
    pub renewed: Vec<Renewed>,
    pub failed: Vec<(String, String)>,
    /// `None` when nothing changed, so there was nothing to reload.
    pub reload: Option<Reload>,
}

/// Issue the certificates of `hosts` again -- once per group -- point their
/// confs at them, and reload nginx.
pub fn renew(stack: &Stack, hosts: &[String]) -> Result<Renewal> {
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for host in hosts {
        groups.entry(mkcert::cert_domain(host).to_string()).or_default().push(host.clone());
    }
    let mut renewal = Renewal::default();
    for (domain, hosts) in groups {
        match mkcert::issue(stack, &domain) {
            Ok(file) => {
                let mut repointed = Vec::new();
                for host in &hosts {
                    match point_at(stack, host, &file) {
                        Ok(true) => repointed.push(host.clone()),
                        Ok(false) => {}
                        Err(err) => renewal.failed.push((host.clone(), format!("{err:#}"))),
                    }
                }
                renewal.renewed.push(Renewed { file, hosts, repointed });
            }
            Err(err) => {
                for host in hosts {
                    renewal.failed.push((host, format!("{err:#}")));
                }
            }
        }
    }
    if !renewal.renewed.is_empty() {
        renewal.reload = Some(Reload::of(stack));
    }
    Ok(renewal)
}

/// Every vhost whose certificate needs it.
pub fn renew_needed(stack: &Stack) -> Result<Renewal> {
    let hosts: Vec<String> =
        inspect(stack).into_iter().filter(SiteCert::needs_renewal).map(|site| site.host).collect();
    if hosts.is_empty() {
        return Ok(Renewal::default());
    }
    renew(stack, &hosts)
}

/// Make the host's conf name `certs/<file>` and its key; `Ok(false)` when it
/// already did. Only those two lines change.
fn point_at(stack: &Stack, host: &str, file: &str) -> Result<bool> {
    let conf = stack.sites_dir().join(format!("{host}.conf"));
    let text = std::fs::read_to_string(&conf).with_context(|| format!("reading {}", conf.display()))?;
    let key = file.strip_suffix(".pem").map(|stem| format!("{stem}-key.pem")).unwrap_or_default();
    let rewritten = set_ssl_files(&text, &format!("certs/{file}"), &format!("certs/{key}"));
    if rewritten == text {
        return Ok(false);
    }
    std::fs::write(&conf, rewritten).with_context(|| format!("writing {}", conf.display()))?;
    Ok(true)
}

/// Replace the values of the two `ssl_certificate*` directives, keeping each
/// line's indentation, spacing, and ending.
pub fn set_ssl_files(text: &str, cert: &str, key: &str) -> String {
    text.split_inclusive('\n')
        .map(|line| {
            let body = line.trim_end_matches(['\r', '\n']);
            let ending = &line[body.len()..];
            let indent = &body[..body.len() - body.trim_start().len()];
            let trimmed = body.trim_start();
            let Some((name, rest)) = trimmed.split_once(char::is_whitespace) else {
                return line.to_string();
            };
            let value = match name {
                "ssl_certificate" => cert,
                "ssl_certificate_key" => key,
                _ => return line.to_string(),
            };
            let gap = &rest[..rest.len() - rest.trim_start().len()];
            format!("{indent}{name} {gap}{value};{ending}")
        })
        .collect()
}

/// The lines a renewal comes to, the same everywhere.
pub fn describe(renewal: &Renewal) -> Vec<String> {
    let mut lines = Vec::new();
    for renewed in &renewal.renewed {
        lines.push(format!("issued   {} for {}", renewed.file, renewed.hosts.join(", ")));
        for host in &renewed.repointed {
            lines.push(format!("         {host}.conf now names it"));
        }
    }
    for (host, why) in &renewal.failed {
        lines.push(format!("FAILED   {host}: {why}"));
    }
    if let Some(reload) = &renewal.reload {
        lines.push(reload.note());
    }
    lines
}

// ---------------------------------------------------------------------------
// The CLI
// ---------------------------------------------------------------------------

/// `devcrate cert list`.
pub fn print_list(stack: &Stack) -> Result<u8> {
    let all = inspect(stack);
    if all.is_empty() {
        println!("No HTTPS vhosts.");
        return Ok(exit::OK);
    }
    let width = all.iter().map(|s| s.host.len()).max().unwrap_or(4).max(4);
    let file_width = all.iter().map(|s| s.file.len()).max().unwrap_or(4).max(4);
    println!("  {:<width$}  {:<file_width$}  {:<10}  STATE", "HOST", "CERTIFICATE", "EXPIRES");
    for site in &all {
        let state = match (site.state, site.days_left) {
            (CertState::Ok | CertState::Expiring, Some(days)) => format!("{} ({days} days)", site.state.label()),
            (state, _) => state.label().to_string(),
        };
        println!(
            "  {:<width$}  {:<file_width$}  {:<10}  {state}",
            site.host,
            site.file,
            site.expires.as_deref().unwrap_or("-")
        );
    }
    let needing = all.iter().filter(|s| s.needs_renewal()).count();
    if needing > 0 {
        println!("{needing} need(s) renewing: `devcrate cert renew` issues them again.");
    }
    if ca_subject(stack).is_none() {
        println!("(mkcert's CA was not found, so no certificate is judged by its issuer)");
    }
    Ok(exit::OK)
}

/// `devcrate cert renew [host...] [--all]`: the hosts named, every HTTPS
/// vhost, or -- with neither -- the ones that need it.
pub fn print_renew(stack: &Stack, hosts: &[String], all: bool) -> Result<u8> {
    let renewal = if all {
        let hosts: Vec<String> = inspect(stack).into_iter().map(|site| site.host).collect();
        renew(stack, &hosts)?
    } else if hosts.is_empty() {
        renew_needed(stack)?
    } else {
        let named: Vec<String> = hosts
            .iter()
            .map(|name| crate::autosite::host_for(name).ok_or_else(|| anyhow!("{name:?} is not a site name")))
            .collect::<Result<_>>()?;
        renew(stack, &named)?
    };
    let lines = describe(&renewal);
    if lines.is_empty() {
        println!("Every certificate is fine; nothing to renew.");
        return Ok(exit::OK);
    }
    for line in &lines {
        println!("  {line}");
    }
    let failed = !renewal.failed.is_empty() || matches!(renewal.reload, Some(Reload::Failed(_)));
    Ok(if failed { exit::ERROR } else { exit::OK })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEAF: &str = include_str!("../tests/fixtures/db.test.pem");
    const CA: &str = include_str!("../tests/fixtures/test-ca.pem");

    fn fixture() -> PathBuf {
        let path = std::env::temp_dir().join(format!("devcrate-cert-{}.pem", std::process::id()));
        std::fs::write(&path, LEAF).unwrap();
        path
    }

    #[test]
    fn a_certificate_is_judged_for_its_host() {
        let path = fixture();
        let ca = x509::parse_pem(CA).unwrap().subject;
        // The fixture expires 2029-01-18 07:29:33 UTC.
        let expires = x509::days_from_civil(2029, 1, 18) * 86_400 + 7 * 3600 + 29 * 60 + 33;
        let early = expires - 400 * 86_400;
        let judged = |host: &str, ca: Option<&[u8]>, now: i64| judge(host, "certs/x.pem", &path, ca, now).state;

        assert_eq!(judged("db.test", Some(&ca), early), CertState::Ok);
        assert_eq!(judged("api.db.test", Some(&ca), early), CertState::Ok);
        assert_eq!(judged("shop.test", Some(&ca), early), CertState::NotCovering);
        assert_eq!(judged("db.test", Some(b"another CA"), early), CertState::ForeignCa);
        assert_eq!(judged("db.test", None, early), CertState::Ok, "no CA known, no verdict on it");
        assert_eq!(judged("db.test", Some(&ca), expires - 10 * 86_400), CertState::Expiring);
        assert_eq!(judged("db.test", Some(&ca), expires + 1), CertState::Expired);

        let site = judge("db.test", "certs/x.pem", &path, Some(&ca), early);
        assert_eq!(site.expires.as_deref(), Some("2029-01-18"));
        assert_eq!(site.days_left, Some(400));
        let missing = judge("db.test", "certs/x.pem", Path::new("Z:\\nope.pem"), None, early);
        assert_eq!(missing.state, CertState::Missing);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn only_the_two_ssl_lines_change() {
        let conf = "server {\r\n    listen 443 ssl;\r\n    ssl_certificate      certs/_wildcard.test.pem;\r\n    ssl_certificate_key  certs/_wildcard.test-key.pem;\r\n    ssl_protocols TLSv1.2;\r\n}\r\n";
        let out = set_ssl_files(conf, "certs/_wildcard.shop.test.pem", "certs/_wildcard.shop.test-key.pem");
        assert_eq!(
            out,
            "server {\r\n    listen 443 ssl;\r\n    ssl_certificate      certs/_wildcard.shop.test.pem;\r\n    ssl_certificate_key  certs/_wildcard.shop.test-key.pem;\r\n    ssl_protocols TLSv1.2;\r\n}\r\n"
        );
        assert_eq!(
            ssl_files(&out),
            (Some("certs/_wildcard.shop.test.pem".into()), Some("certs/_wildcard.shop.test-key.pem".into()))
        );
        assert_eq!(ssl_files("server { listen 80; }"), (None, None));
    }
}
