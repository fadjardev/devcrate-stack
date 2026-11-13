//! Which installed PHP a project's `composer.json` accepts.
//!
//! `"php": "^8.2"` means 8.2 *or anything newer in 8.x*, so a stack with only
//! 8.5 installed serves it. Taking the constraint's first number as an exact
//! version -- what this replaced -- refused that project with "PHP 8.2 is not
//! installed", which is every freshly cloned Laravel app on a fresh stack.
//!
//! The question asked of a constraint is narrower than Composer's: not "is
//! this exact release allowed" but "is *some* release of PHP X.Y allowed",
//! since a stack installs PHP by X.Y and keeps whichever patch it has. Each
//! clause is therefore turned into a half-open range of versions and checked
//! for overlap with `[X.Y.0, X.(Y+1).0)`.
//!
//! Covered: `||` (and the older `|`) for OR; spaces and commas for AND; `^`,
//! `~`, `>=`, `>`, `<=`, `<`, `=`, `==`, `!=`; wildcards (`8.*`, `*`); hyphen
//! ranges (`8.1 - 8.3`); stability suffixes (`@dev`, `-RC1`) ignored. Anything
//! else makes the whole constraint unreadable, and the caller falls back to
//! the default PHP rather than guessing.

use std::path::Path;

/// A version as three numbers. Missing parts are zero.
type Version = (u32, u32, u32);

const MAX: Version = (u32::MAX, 0, 0);

/// The `require.php` constraint of `project_dir`'s `composer.json`, if any.
pub fn constraint(project_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(project_dir.join("composer.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let php = json.get("require")?.get("php")?.as_str()?.trim();
    (!php.is_empty()).then(|| php.to_string())
}

/// Does `constraint` allow some release of PHP `major.minor`? `None` when the
/// constraint cannot be read.
pub fn allows(constraint: &str, major: u32, minor: u32) -> Option<bool> {
    let candidate = ((major, minor, 0), (major, minor + 1, 0));
    let mut any = false;
    for alternative in constraint.split("||").flat_map(|part| part.split('|')) {
        let ranges = conjunction(alternative)?;
        let lo = ranges.iter().map(|r| r.0).chain([candidate.0]).max()?;
        let hi = ranges.iter().map(|r| r.1).chain([candidate.1]).min()?;
        any |= lo < hi;
    }
    Some(any)
}

/// One `||` alternative: its clauses, each as a half-open range.
fn conjunction(text: &str) -> Option<Vec<(Version, Version)>> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // A hyphen range is the one clause with spaces inside it.
    if let Some((from, to)) = text.split_once(" - ") {
        let (lo, _) = parse(from.trim())?;
        let (to, parts) = parse(to.trim())?;
        return Some(vec![(lo, after(to, parts))]);
    }

    let mut ranges = Vec::new();
    let tokens: Vec<&str> = text.split([' ', ',']).filter(|t| !t.is_empty()).collect();
    let mut i = 0;
    while i < tokens.len() {
        // `>= 8.1`, with a space after the operator, is two tokens.
        let mut clause = tokens[i].to_string();
        if clause.chars().all(|c| "<>=!^~".contains(c)) {
            i += 1;
            clause.push_str(tokens.get(i)?);
        }
        if let Some(range) = clause_range(&clause)? {
            ranges.push(range);
        }
        i += 1;
    }
    Some(ranges)
}

/// One clause as a range. `Some(None)` for a clause that limits nothing at
/// X.Y granularity (`!=`, `*`).
fn clause_range(clause: &str) -> Option<Option<(Version, Version)>> {
    let op_len = clause.find(|c: char| c.is_ascii_digit() || c == '*' || c == 'v').unwrap_or(clause.len());
    let (op, version) = clause.split_at(op_len);
    if version == "*" {
        return Some(None);
    }
    let (v, parts) = parse(version)?;
    Some(Some(match op {
        "^" => (v, caret(v, parts)),
        "~" => (v, tilde(v, parts)),
        ">=" => (v, MAX),
        // Composer reads a partial version in a comparison as X.Y.0, so `>8.2`
        // admits 8.2.1 and `<=8.2` stops at 8.2.0 -- unlike a wildcard.
        ">" => (after(v, 3), MAX),
        "<=" => ((0, 0, 0), after(v, 3)),
        "<" => ((0, 0, 0), v),
        "!=" => return Some(None),
        // Exact, or a wildcard: `8.2` and `8.2.*` both cover all of 8.2.
        "" | "=" | "==" => (v, after(v, parts)),
        _ => return None,
    }))
}

/// A version and how many numbers it was written with: `8.2` is `((8,2,0), 2)`.
/// A trailing `.*` counts as not written. Stability suffixes are dropped.
fn parse(text: &str) -> Option<(Version, usize)> {
    let text = text.trim().trim_start_matches('v');
    let text = text.split(['@', '-', '+']).next()?;
    let mut numbers = Vec::new();
    for part in text.split('.') {
        if part == "*" || part == "x" {
            break;
        }
        numbers.push(part.parse::<u32>().ok()?);
    }
    if numbers.is_empty() || numbers.len() > 4 {
        return None;
    }
    let at = |i: usize| numbers.get(i).copied().unwrap_or(0);
    Some(((at(0), at(1), at(2)), numbers.len().min(3)))
}

/// The first version past everything `v`, written with `parts` numbers,
/// covers: `8.2` covers all of 8.2.x, so it is followed by 8.3.0.
fn after((a, b, c): Version, parts: usize) -> Version {
    match parts {
        1 => (a + 1, 0, 0),
        2 => (a, b + 1, 0),
        _ => (a, b, c + 1),
    }
}

/// `^`: up to the next change in the leftmost non-zero part.
fn caret((a, b, c): Version, parts: usize) -> Version {
    match (a, b) {
        (0, 0) if parts >= 3 => (0, 0, c + 1),
        (0, 0) if parts == 2 => (0, 1, 0),
        (0, _) if parts >= 2 => (0, b + 1, 0),
        _ => (a + 1, 0, 0),
    }
}

/// `~`: the last written part may grow, the one before it may not.
fn tilde((a, b, _): Version, parts: usize) -> Version {
    match parts {
        1 | 2 => (a + 1, 0, 0),
        _ => (a, b + 1, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(constraint: &str, major: u32, minor: u32) -> bool {
        allows(constraint, major, minor).unwrap_or_else(|| panic!("{constraint:?} did not parse"))
    }

    /// The case behind the fix: a caret constraint accepts newer minors.
    #[test]
    fn a_caret_accepts_every_later_minor_of_its_major() {
        assert!(ok("^8.2", 8, 2));
        assert!(ok("^8.2", 8, 5));
        assert!(!ok("^8.2", 8, 1));
        assert!(!ok("^8.2", 9, 0));
        assert!(!ok("^7.4", 8, 0));
        assert!(ok("^8.2.10", 8, 2), "some 8.2 patch is at least .10");
    }

    #[test]
    fn the_common_composer_spellings() {
        assert!(ok(">=8.1", 8, 5));
        assert!(!ok(">=8.1", 7, 4));
        assert!(ok(">=7.4 <8.3", 8, 2));
        assert!(!ok(">=7.4 <8.3", 8, 3));
        assert!(ok(">=7.4,<8.3", 7, 4));
        assert!(ok(">= 8.1", 8, 2), "a space after the operator");
        assert!(ok("^7.4 || ^8.0", 7, 4));
        assert!(ok("^7.4 || ^8.0", 8, 5));
        assert!(!ok("^7.4 || ^8.0", 5, 6));
        assert!(ok("^7.4|^8.0", 8, 1), "the old single bar");
        assert!(ok("~8.1", 8, 4), "~8.1 is >=8.1 <9");
        assert!(!ok("~8.1.0", 8, 2), "~8.1.0 is >=8.1.0 <8.2");
        assert!(ok("8.2.*", 8, 2));
        assert!(!ok("8.2.*", 8, 3));
        assert!(ok("8.*", 8, 5));
        assert!(ok("*", 5, 6));
        assert!(ok("8.1 - 8.3", 8, 3));
        assert!(!ok("8.1 - 8.3", 8, 4));
        assert!(ok("<=8.2", 8, 2));
        assert!(ok(">8.2", 8, 3));
        assert!(ok(">8.2", 8, 2), ">8.2 is >8.2.0, which 8.2.1 is");
        assert!(!ok("<=8.1", 8, 2));
        assert!(!ok("<8.2", 8, 2));
        assert!(ok(">=8.2 !=8.3.1", 8, 3), "!= one patch rules out no minor");
        assert!(ok("8.2", 8, 2), "an exact X.Y");
        assert!(ok(">=8.1@dev", 8, 1), "a stability flag is ignored");
    }

    #[test]
    fn unreadable_constraints_are_reported_as_such() {
        assert_eq!(allows("", 8, 2), None);
        assert_eq!(allows("latest", 8, 2), None);
        assert_eq!(allows(">=", 8, 2), None);
    }

    #[test]
    fn the_constraint_is_read_from_composer_json() {
        let dir = std::env::temp_dir().join(format!("devcrate-composer-php-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("composer.json"), r#"{"require": {"php": " ^8.2 "}}"#).unwrap();
        assert_eq!(constraint(&dir).as_deref(), Some("^8.2"));
        std::fs::write(dir.join("composer.json"), r#"{"require": {"laravel/framework": "^11"}}"#).unwrap();
        assert_eq!(constraint(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
