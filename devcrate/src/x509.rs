//! Just enough X.509 to answer three questions about a certificate: when it
//! expires, which names it covers, and who issued it.
//!
//! A certificate is DER inside PEM. The walk is: `Certificate` -> its
//! `tbsCertificate` -> skip the optional `[0]` version, the serial, and the
//! signature algorithm -> `issuer` (kept as raw bytes) -> `validity`
//! (`notAfter`) -> `subject` (raw bytes) -> skip the key -> `[3]` extensions,
//! where subjectAltName (2.5.29.17) lists the DNS names.
//!
//! Issuer and subject are compared as raw DER, which is how chain building
//! matches them too. No crypto: nothing here checks a signature -- that is the
//! browser's job, and these answers are only for reporting.

use anyhow::{Result, anyhow, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cert {
    /// When it stops being valid, seconds since the Unix epoch.
    pub not_after: i64,
    /// Its subjectAltName DNS entries: `*.shop.test`, `shop.test`.
    pub names: Vec<String>,
    /// The issuer `Name`, raw DER.
    pub issuer: Vec<u8>,
    /// The subject `Name`, raw DER -- for a CA, what its certificates name as
    /// issuer.
    pub subject: Vec<u8>,
}

/// The first certificate in a PEM file.
pub fn parse_pem(text: &str) -> Result<Cert> {
    let begin = "-----BEGIN CERTIFICATE-----";
    let start = text.find(begin).ok_or_else(|| anyhow!("no certificate in the file"))? + begin.len();
    let end = text[start..].find("-----END CERTIFICATE-----").ok_or_else(|| anyhow!("the certificate is cut short"))?;
    let der = base64_decode(&text[start..start + end])?;
    parse_der(&der)
}

pub fn parse_der(der: &[u8]) -> Result<Cert> {
    let (certificate, _) = element(der, 0x30)?;
    let (tbs, _) = element(certificate, 0x30)?;
    let mut rest = tbs;
    if rest.first() == Some(&0xa0) {
        rest = skip(rest)?; // [0] version
    }
    rest = skip(rest)?; // serialNumber
    rest = skip(rest)?; // signature AlgorithmIdentifier
    let issuer = raw(rest)?.to_vec();
    rest = skip(rest)?;
    let (validity, after_validity) = element(rest, 0x30)?;
    let not_after = {
        let after_not_before = skip(validity)?;
        time(after_not_before)?
    };
    rest = after_validity;
    let subject = raw(rest)?.to_vec();
    rest = skip(rest)?;
    rest = skip(rest)?; // subjectPublicKeyInfo

    let mut names = Vec::new();
    while let Some(&tag) = rest.first() {
        if tag == 0xa3 {
            let (wrapper, _) = element(rest, 0xa3)?;
            let (extensions, _) = element(wrapper, 0x30)?;
            names = san_names(extensions)?;
            break;
        }
        rest = skip(rest)?; // [1] / [2] unique IDs
    }
    Ok(Cert { not_after, names, issuer, subject })
}

/// The DNS names of the subjectAltName extension, if there is one.
fn san_names(mut extensions: &[u8]) -> Result<Vec<String>> {
    const SAN: [u8; 5] = [0x06, 0x03, 0x55, 0x1d, 0x11];
    while !extensions.is_empty() {
        let (extension, next) = element(extensions, 0x30)?;
        extensions = next;
        if !extension.starts_with(&SAN) {
            continue;
        }
        let mut inner = skip(extension)?; // the OID
        if inner.first() == Some(&0x01) {
            inner = skip(inner)?; // critical
        }
        let (octets, _) = element(inner, 0x04)?;
        let (mut general_names, _) = element(octets, 0x30)?;
        let mut names = Vec::new();
        while !general_names.is_empty() {
            let tag = general_names[0];
            let (value, next) = element(general_names, tag)?;
            if tag == 0x82 {
                names.push(String::from_utf8_lossy(value).into_owned());
            }
            general_names = next;
        }
        return Ok(names);
    }
    Ok(Vec::new())
}

/// UTCTime (`YYMMDDHHMMSSZ`) or GeneralizedTime (`YYYYMMDDHHMMSSZ`), as epoch
/// seconds.
fn time(input: &[u8]) -> Result<i64> {
    let tag = *input.first().ok_or_else(|| anyhow!("missing time"))?;
    let (value, _) = element(input, tag)?;
    let text = std::str::from_utf8(value).map_err(|_| anyhow!("a time that is not text"))?;
    let digits = text.trim_end_matches('Z');
    let (year, rest) = match tag {
        0x17 if digits.len() >= 12 => {
            let yy: i64 = digits[..2].parse()?;
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, &digits[2..])
        }
        0x18 if digits.len() >= 14 => (digits[..4].parse()?, &digits[4..]),
        _ => bail!("an unexpected time: {text:?}"),
    };
    let field = |at: usize| -> Result<i64> { Ok(rest[at..at + 2].parse()?) };
    let (month, day, hour, minute, second) = (field(0)?, field(2)?, field(4)?, field(6)?, field(8)?);
    Ok(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

/// Days since 1970-01-01 for a calendar date (Howard Hinnant's algorithm).
pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// One element's tag, contents, and what follows it. `tag` must match.
fn element(input: &[u8], tag: u8) -> Result<(&[u8], &[u8])> {
    let (found, start, len) = header(input)?;
    if found != tag {
        bail!("expected DER tag {tag:#04x}, found {found:#04x}");
    }
    Ok((&input[start..start + len], &input[start + len..]))
}

/// The whole of the next element, header included.
fn raw(input: &[u8]) -> Result<&[u8]> {
    let (_, start, len) = header(input)?;
    Ok(&input[..start + len])
}

fn skip(input: &[u8]) -> Result<&[u8]> {
    let (_, start, len) = header(input)?;
    Ok(&input[start + len..])
}

/// (tag, where the contents start, their length), checked against the input.
fn header(input: &[u8]) -> Result<(u8, usize, usize)> {
    let tag = *input.first().ok_or_else(|| anyhow!("the certificate ends early"))?;
    let first = *input.get(1).ok_or_else(|| anyhow!("the certificate ends early"))? as usize;
    let (start, len) = if first < 0x80 {
        (2, first)
    } else {
        let count = first & 0x7f;
        if count == 0 || count > 4 || input.len() < 2 + count {
            bail!("a DER length the certificate cannot have");
        }
        let len = input[2..2 + count].iter().fold(0usize, |acc, b| (acc << 8) | *b as usize);
        (2 + count, len)
    };
    if input.len() < start + len {
        bail!("the certificate is cut short");
    }
    Ok((tag, start, len))
}

fn base64_decode(text: &str) -> Result<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0;
    for c in text.bytes() {
        if c.is_ascii_whitespace() {
            continue;
        }
        if c == b'=' {
            break;
        }
        let v = value(c).ok_or_else(|| anyhow!("the certificate's base64 has a stray {:?}", c as char))?;
        buffer = (buffer << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// Whether a certificate name covers a host: exactly, or as a wildcard for
/// exactly one more label (`*.shop.test` covers `api.shop.test`, not
/// `shop.test` and not `a.b.shop.test`), the way browsers match.
pub fn covers(names: &[String], host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    names.iter().any(|name| {
        let name = name.to_ascii_lowercase();
        match name.strip_prefix("*.") {
            Some(parent) => host
                .split_once('.')
                .is_some_and(|(label, rest)| !label.is_empty() && rest == parent),
            None => name == host,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A leaf shaped like mkcert's -- `*.db.test` and `db.test`, not after
    /// 2029-01-18 07:29:33 UTC -- issued by a throwaway "devcrate test CA",
    /// both made with openssl for these tests. Public halves only.
    const DB_TEST: &str = include_str!("../tests/fixtures/db.test.pem");
    const TEST_CA: &str = include_str!("../tests/fixtures/test-ca.pem");

    #[test]
    fn a_real_certificate_reads_back() {
        let cert = parse_pem(DB_TEST).unwrap();
        assert_eq!(cert.names, ["*.db.test", "db.test"]);
        assert_eq!(cert.not_after, days_from_civil(2029, 1, 18) * 86_400 + 7 * 3600 + 29 * 60 + 33);
        let ca = parse_pem(TEST_CA).unwrap();
        assert_eq!(cert.issuer, ca.subject, "issued by that CA, matched as raw DER");
        assert_ne!(cert.issuer, cert.subject, "a leaf, not its own CA");
        assert!(ca.names.is_empty(), "a CA with no subjectAltName");
    }

    #[test]
    fn wildcards_cover_one_label_only() {
        let names = vec!["*.shop.test".to_string()];
        assert!(covers(&names, "api.shop.test"));
        assert!(!covers(&names, "shop.test"));
        assert!(!covers(&names, "a.b.shop.test"));
        assert!(covers(&["shop.test".to_string()], "SHOP.test"));
        assert!(!covers(&["*.test".to_string()], "shop.local.test"));
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(parse_pem("nothing here").is_err());
        assert!(parse_pem("-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----").is_err());
        assert!(parse_der(&[0x30, 0x82, 0xff]).is_err());
    }

    #[test]
    fn civil_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    }
}
