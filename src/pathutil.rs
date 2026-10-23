//! Paths as sqlfmt handles them: normalised path strings, ordered component by
//! component, and the "-" path that stands for standard input.

use std::cmp::Ordering;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// A path to format: a file (an absolute, normalised path) or standard input.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SourcePath {
    File(String),
    Stdin,
}

impl SourcePath {
    /// The components of the path ("/a/b" has the components "", "a" and "b").
    fn parts(&self) -> Vec<&str> {
        match self {
            SourcePath::File(p) => p.split('/').collect(),
            SourcePath::Stdin => vec!["-"],
        }
    }

    /// The path as shown in reports: relative to the working directory when it is
    /// inside it, "-" for standard input.
    pub fn display(&self) -> String {
        match self {
            SourcePath::Stdin => "-".to_string(),
            SourcePath::File(p) => match std::env::current_dir() {
                Ok(cwd) => relative_to(p, &cwd.to_string_lossy()).unwrap_or_else(|| p.clone()),
                Err(_) => p.clone(),
            },
        }
    }
}

impl Ord for SourcePath {
    /// Paths sort by their components, so "a/b" sorts before "a-b".
    fn cmp(&self, other: &SourcePath) -> Ordering {
        self.parts().cmp(&other.parts())
    }
}

impl PartialOrd for SourcePath {
    fn partial_cmp(&self, other: &SourcePath) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// `path` relative to `base`, if `path` is `base` or inside it.
fn relative_to(path: &str, base: &str) -> Option<String> {
    let base_parts: Vec<&str> = split_parts(base);
    let parts: Vec<&str> = split_parts(path);
    if parts.len() < base_parts.len() || parts[..base_parts.len()] != base_parts[..] {
        return None;
    }
    let rest = &parts[base_parts.len()..];
    Some(if rest.is_empty() { ".".to_string() } else { rest.join("/") })
}

fn split_parts(path: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    if path.starts_with('/') {
        parts.insert(0, "/");
    }
    parts
}

/// Normalises a path string the way path objects do: repeated separators and "."
/// components are removed, as is a trailing separator; ".." is kept.
pub fn normalize(path: &str) -> String {
    let absolute = path.starts_with('/');
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    let joined = parts.join("/");
    match (absolute, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_string(),
        (false, false) => joined,
    }
}

/// Joins a relative path onto a directory and normalises the result (an absolute
/// `rel` replaces the directory).
pub fn join(dir: &Path, rel: &str) -> String {
    if rel.starts_with('/') {
        return normalize(rel);
    }
    normalize(&format!("{}/{rel}", dir.to_string_lossy()))
}

/// The path of a directory entry.
pub fn join_name(dir: &str, name: &OsStr) -> String {
    let name = name.to_string_lossy();
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// The real path: absolute, with symbolic links resolved as far as the path exists.
pub fn realpath(path: &str) -> String {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let absolute = if path.starts_with('/') { path.to_string() } else { format!("{}/{path}", cwd.to_string_lossy()) };
    let mut resolved = String::from("/");
    let mut pending: Vec<String> = absolute.split('/').filter(|p| !p.is_empty()).map(str::to_string).rev().collect();
    let mut links_followed = 0;
    while let Some(part) = pending.pop() {
        match part.as_str() {
            "." => continue,
            ".." => {
                resolved = parent_of(&resolved);
                continue;
            }
            _ => {}
        }
        let candidate = if resolved == "/" { format!("/{part}") } else { format!("{resolved}/{part}") };
        match std::fs::read_link(&candidate) {
            Ok(target) if links_followed < 40 => {
                links_followed += 1;
                let target = target.to_string_lossy().to_string();
                if target.starts_with('/') {
                    resolved = "/".to_string();
                }
                for p in target.split('/').filter(|p| !p.is_empty()).rev() {
                    pending.push(p.to_string());
                }
            }
            _ => resolved = candidate,
        }
    }
    resolved
}

fn parent_of(path: &str) -> String {
    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => path[..i].to_string(),
    }
}

/// The parents of an absolute path, nearest first ("/a/b" has "/a" and "/").
pub fn parents(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = normalize(path);
    while current != "/" {
        current = parent_of(&current);
        out.push(current.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_by_components() {
        let a = SourcePath::File("/x/b/c.sql".into());
        let b = SourcePath::File("/x/b-c.sql".into());
        assert!(a < b);
        assert!(b < SourcePath::Stdin);
    }

    #[test]
    fn normalizes_like_path_objects() {
        assert_eq!(normalize("/a//b/./c/"), "/a/b/c");
        assert_eq!(normalize("/a/../b"), "/a/../b");
        assert_eq!(join(Path::new("/root"), "./sub/*.sql"), "/root/sub/*.sql");
        assert_eq!(parents("/a/b"), vec!["/a".to_string(), "/".to_string()]);
    }

    #[test]
    fn relative_paths() {
        assert_eq!(relative_to("/w/a/b.sql", "/w"), Some("a/b.sql".into()));
        assert_eq!(relative_to("/v/b.sql", "/w"), None);
    }
}
