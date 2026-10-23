//! Shell-style path globbing for `--exclude`.
//!
//! `*`, `?` and `[...]` match within one path component, `**` as a whole component
//! matches any number of directories (including none), names starting with a dot are
//! only matched by patterns that start with a dot, and a pattern without wildcards
//! matches a path that exists.

use std::fs;
use std::path::Path;

fn has_magic(s: &str) -> bool {
    s.contains(['*', '?', '['])
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

fn is_recursive(pattern: &str) -> bool {
    pattern == "**"
}

/// Splits a path into (head, tail) at the last separator; the head keeps a leading
/// root and loses trailing separators otherwise.
fn split(path: &str) -> (String, String) {
    match path.rfind('/') {
        None => (String::new(), path.to_string()),
        Some(i) => {
            let head = &path[..=i];
            let tail = &path[i + 1..];
            let trimmed = head.trim_end_matches('/');
            let head = if trimmed.is_empty() { head.to_string() } else { trimmed.to_string() };
            (head, tail.to_string())
        }
    }
}

fn join(a: &str, b: &str) -> String {
    if a.is_empty() || b.starts_with('/') {
        b.to_string()
    } else if a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

fn lexists(path: &str) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn is_dir(path: &str) -> bool {
    Path::new(path).is_dir()
}

/// The names in a directory. As in shell globbing, a directory that cannot be read
/// (or an entry that cannot be read) contributes no matches rather than an error.
fn list_dir(dir: &str, dironly: bool) -> Vec<String> {
    let dir = if dir.is_empty() { "." } else { dir };
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    entries
        .map_while(Result::ok)
        .filter(|e| !dironly || e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect()
}

/// The paths matching `pattern` (recursive `**` enabled, hidden names skipped).
pub(crate) fn glob(pattern: &str) -> Vec<String> {
    iglob(pattern, false)
}

fn iglob(pattern: &str, dironly: bool) -> Vec<String> {
    let (dirname, basename) = split(pattern);
    if !has_magic(pattern) {
        if !basename.is_empty() {
            if lexists(pattern) {
                return vec![pattern.to_string()];
            }
        } else if is_dir(&dirname) {
            return vec![pattern.to_string()];
        }
        return Vec::new();
    }
    if dirname.is_empty() {
        return if is_recursive(&basename) { glob2("", &basename, dironly) } else { glob1("", &basename, dironly) };
    }
    let dirs = if dirname != pattern && has_magic(&dirname) { iglob(&dirname, true) } else { vec![dirname.clone()] };
    let mut out = Vec::new();
    for dir in dirs {
        let names = if has_magic(&basename) {
            if is_recursive(&basename) {
                glob2(&dir, &basename, dironly)
            } else {
                glob1(&dir, &basename, dironly)
            }
        } else {
            glob0(&dir, &basename)
        };
        out.extend(names.into_iter().map(|name| join(&dir, &name)));
    }
    out
}

fn glob1(dir: &str, pattern: &str, dironly: bool) -> Vec<String> {
    let names = list_dir(dir, dironly);
    names.into_iter().filter(|n| (is_hidden(pattern) || !is_hidden(n)) && fnmatch(n, pattern)).collect()
}

fn glob0(dir: &str, basename: &str) -> Vec<String> {
    if !basename.is_empty() {
        if lexists(&join(dir, basename)) {
            return vec![basename.to_string()];
        }
    } else if is_dir(dir) {
        return vec![basename.to_string()];
    }
    Vec::new()
}

fn glob2(dir: &str, _pattern: &str, dironly: bool) -> Vec<String> {
    let mut out = Vec::new();
    if dir.is_empty() || is_dir(dir) {
        out.push(String::new());
    }
    rlistdir(dir, dironly, &mut out, "");
    out
}

fn rlistdir(dir: &str, dironly: bool, out: &mut Vec<String>, prefix: &str) {
    for name in list_dir(dir, dironly) {
        if is_hidden(&name) {
            continue;
        }
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        out.push(rel.clone());
        let path = if dir.is_empty() { name.clone() } else { join(dir, &name) };
        rlistdir(&path, dironly, out, &rel);
    }
}

/// Matches a name against a shell pattern (`*`, `?`, `[seq]`, `[!seq]`).
pub(crate) fn fnmatch(name: &str, pattern: &str) -> bool {
    let name: Vec<char> = name.chars().collect();
    let pattern = compile(pattern);
    matches(&name, &pattern)
}

#[derive(Debug, Clone)]
enum Item {
    Literal(char),
    Any,
    Star,
    Class { negated: bool, ranges: Vec<(char, char)> },
}

fn compile(pattern: &str) -> Vec<Item> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut items = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        match c {
            '*' => {
                if !matches!(items.last(), Some(Item::Star)) {
                    items.push(Item::Star);
                }
            }
            '?' => items.push(Item::Any),
            '[' => {
                let mut j = i;
                if j < chars.len() && chars[j] == '!' {
                    j += 1;
                }
                if j < chars.len() && chars[j] == ']' {
                    j += 1;
                }
                while j < chars.len() && chars[j] != ']' {
                    j += 1;
                }
                if j >= chars.len() {
                    items.push(Item::Literal('['));
                    continue;
                }
                let body = &chars[i..j];
                i = j + 1;
                let (negated, body) = if body.first() == Some(&'!') { (true, &body[1..]) } else { (false, body) };
                let mut ranges = Vec::new();
                let mut k = 0;
                while k < body.len() {
                    if k + 2 < body.len() && body[k + 1] == '-' {
                        if body[k] <= body[k + 2] {
                            ranges.push((body[k], body[k + 2]));
                        }
                        k += 3;
                    } else {
                        ranges.push((body[k], body[k]));
                        k += 1;
                    }
                }
                items.push(Item::Class { negated, ranges });
            }
            c => items.push(Item::Literal(c)),
        }
    }
    items
}

fn matches(name: &[char], pattern: &[Item]) -> bool {
    match pattern.first() {
        None => name.is_empty(),
        Some(Item::Star) => (0..=name.len()).any(|k| matches(&name[k..], &pattern[1..])),
        Some(item) => match name.first() {
            None => false,
            Some(&c) => {
                let ok = match item {
                    Item::Literal(l) => *l == c,
                    Item::Any => true,
                    Item::Class { negated, ranges } => {
                        ranges.iter().any(|(lo, hi)| (*lo..=*hi).contains(&c)) != *negated
                    }
                    Item::Star => unreachable!("handled above"),
                };
                ok && matches(&name[1..], &pattern[1..])
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnmatch_patterns() {
        assert!(fnmatch("a.sql", "*.sql"));
        assert!(!fnmatch("a.sqlx", "*.sql"));
        assert!(fnmatch("b1.sql", "b[0-9].sql"));
        assert!(fnmatch("bx.sql", "b[!0-9].sql"));
        assert!(fnmatch("[x", "[x"));
        assert!(fnmatch("a", "?"));
    }

    #[test]
    fn splits_like_paths() {
        assert_eq!(split("/a/b"), ("/a".to_string(), "b".to_string()));
        assert_eq!(split("/b"), ("/".to_string(), "b".to_string()));
        assert_eq!(split("b"), (String::new(), "b".to_string()));
    }
}
