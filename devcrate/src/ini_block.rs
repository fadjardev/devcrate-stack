//! devcrate's marked blocks in a `php.ini`.
//!
//! Every change devcrate makes to a `php.ini` goes between a pair of markers,
//! `; --- devcrate <name> begin ---` and `; --- devcrate <name> end ---`, at the
//! end of the file -- the same discipline the hosts file gets. Only the block
//! is ever written: a hand-edited `php.ini` is never rewritten, a second write
//! replaces the block rather than adding another, and removing it gives back
//! the file exactly as it was. Being last, a block's values win over anything
//! earlier in the file.

use std::path::Path;

fn markers(name: &str) -> (String, String) {
    (format!("; --- devcrate {name} begin ---"), format!("; --- devcrate {name} end ---"))
}

/// `text` with the `name` block holding `body`: replaced where there is one,
/// appended where there is not. Everything outside the block is returned as
/// it was.
pub fn with_block(text: &str, name: &str, body: &[String]) -> String {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let (begin, end) = markers(name);
    let mut lines = vec![begin.as_str()];
    lines.extend(body.iter().map(String::as_str));
    lines.push(end.as_str());
    let block = lines.join(nl);

    // Exactly one blank line and the block are added, so exactly that comes
    // out again on removal -- the file's own trailing blank lines included.
    let mut base = without_block(text, name);
    if !base.is_empty() && !base.ends_with('\n') {
        base.push_str(nl);
    }
    format!("{base}{nl}{block}{nl}")
}

/// `text` with the `name` block taken out, and nothing else changed -- the
/// inverse of [`with_block`], blank separator line included.
pub fn without_block(text: &str, name: &str) -> String {
    let (begin, end) = markers(name);
    let Some((start, after)) = span(text, &begin, &end) else {
        return text.to_string();
    };
    // ...and the blank line `with_block` put before it goes too.
    let before = &text[..start];
    let before = before
        .strip_suffix("\r\n")
        .filter(|b| b.is_empty() || b.ends_with('\n'))
        .or_else(|| before.strip_suffix('\n').filter(|b| b.is_empty() || b.ends_with('\n')))
        .unwrap_or(before);
    format!("{before}{}", &text[after..])
}

/// The lines between the `name` markers, or `None` when there is no block.
pub fn body(text: &str, name: &str) -> Option<Vec<String>> {
    let (begin, end) = markers(name);
    let (start, _) = span(text, &begin, &end)?;
    let inner = &text[start + begin.len()..text[start..].find(&end)? + start];
    Some(inner.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
}

/// Whether line `index` (0-based) of `text` lies inside the `name` block.
pub fn inside(text: &str, name: &str, index: usize) -> bool {
    let (begin, end) = markers(name);
    let mut open = false;
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line == begin {
            open = true;
        } else if line == end {
            if i == index {
                return true;
            }
            open = false;
        }
        if i == index {
            return open;
        }
    }
    false
}

/// From the begin marker to just past the end marker's line break.
fn span(text: &str, begin: &str, end: &str) -> Option<(usize, usize)> {
    let start = text.find(begin)?;
    let end_at = start + text[start..].find(end)?;
    let mut after = end_at + end.len();
    if text[after..].starts_with("\r\n") {
        after += 2;
    } else if text[after..].starts_with('\n') {
        after += 1;
    }
    Some((start, after))
}

/// What one `php.ini` edit came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IniEdit {
    Written,
    AlreadyThere,
    Failed(String),
}

/// Apply `edit` to `php_ini`, through a temp file and a rename, writing
/// nothing when it changes nothing.
pub fn rewrite(php_ini: &Path, edit: impl Fn(&str) -> String) -> IniEdit {
    let text = match std::fs::read_to_string(php_ini) {
        Ok(text) => text,
        Err(err) => return IniEdit::Failed(format!("reading {}: {err}", php_ini.display())),
    };
    let new = edit(&text);
    if new == text {
        return IniEdit::AlreadyThere;
    }
    let temp = php_ini.with_extension("ini.devcrate-tmp");
    let written = std::fs::write(&temp, &new).and_then(|()| std::fs::rename(&temp, php_ini));
    match written {
        Ok(()) => IniEdit::Written,
        Err(err) => {
            let _ = std::fs::remove_file(&temp);
            IniEdit::Failed(format!("writing {}: {err}", php_ini.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(body: &[&str]) -> Vec<String> {
        body.iter().map(|s| s.to_string()).collect()
    }

    /// Two blocks keep out of each other's way, and each comes out alone.
    #[test]
    fn blocks_with_different_names_coexist() {
        let ini = "[PHP]\nmemory_limit = 1G\n";
        let one = with_block(ini, "mailpit", &lines(&["SMTP = 127.0.0.1"]));
        let both = with_block(&one, "extensions", &lines(&["extension=php_pgsql.dll"]));
        assert_eq!(body(&both, "extensions").unwrap(), ["extension=php_pgsql.dll"]);
        assert_eq!(body(&both, "mailpit").unwrap(), ["SMTP = 127.0.0.1"]);
        assert_eq!(without_block(&without_block(&both, "extensions"), "mailpit"), ini);
        assert_eq!(without_block(&both, "extensions"), one);
    }

    #[test]
    fn lines_inside_the_block_are_told_apart() {
        let text = with_block("extension=curl\n", "extensions", &lines(&["extension=php_pgsql.dll"]));
        let at = |needle: &str| text.lines().position(|l| l == needle).unwrap();
        assert!(!inside(&text, "extensions", at("extension=curl")));
        assert!(inside(&text, "extensions", at("extension=php_pgsql.dll")));
        assert!(body("extension=curl\n", "extensions").is_none());
    }
}
