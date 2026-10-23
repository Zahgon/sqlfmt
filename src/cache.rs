//! The cache of files that need no formatting.
//!
//! Ported from sqlfmt/cache.py. The cache maps each (resolved) file path to the file's
//! modification time and size after its last successful run; a file whose time and
//! size still match is not read again. It lives in the user cache directory
//! (`$XDG_CACHE_HOME/sqlfmt`, or `~/.cache/sqlfmt`), one file per sqlfmt version.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use crate::error::Fatal;
use crate::mode::Mode;
use crate::pathutil::{self, SourcePath};
use crate::report::SqlFormatResult;

/// (modification time in nanoseconds, size in bytes)
pub(crate) type CacheInfo = (i128, u64);

#[derive(Debug, Clone, Default)]
pub(crate) struct Cache {
    pub(crate) entries: HashMap<String, CacheInfo>,
}

/// The directory of the user's cache.
fn cache_dir() -> PathBuf {
    let xdg = std::env::var("XDG_CACHE_HOME").unwrap_or_default();
    let base = if xdg.trim().is_empty() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        PathBuf::from(home).join(".cache")
    } else {
        PathBuf::from(xdg)
    };
    base.join("sqlfmt")
}

/// The path of the cache file.
pub(crate) fn get_cache_file() -> PathBuf {
    cache_dir().join(format!("cache-{}.tsv", env!("CARGO_PKG_VERSION")))
}

/// Loads the cache. A missing, unreadable or malformed cache file is an empty cache
/// (sqlfmt then formats every file and writes a new cache), never an error.
pub(crate) fn load_cache() -> Cache {
    let Ok(text) = fs::read_to_string(get_cache_file()) else {
        return Cache::default();
    };
    let mut entries = HashMap::new();
    for line in text.lines() {
        let mut fields = line.splitn(3, '\t');
        let (Some(mtime), Some(size), Some(path)) = (fields.next(), fields.next(), fields.next()) else {
            return Cache::default();
        };
        let (Ok(mtime), Ok(size), Some(path)) = (mtime.parse::<i128>(), size.parse::<u64>(), unescape(path)) else {
            return Cache::default();
        };
        entries.insert(path, (mtime, size));
    }
    Cache { entries }
}

/// True if the path is cached and the file's time and size still match; a file whose
/// metadata cannot be read is not a cache hit, so it is read (and its error reported)
/// like any other file.
pub(crate) fn check_cache(cache: &Cache, path: &SourcePath) -> bool {
    let SourcePath::File(p) = path else { return false };
    if cache.entries.is_empty() {
        return false;
    }
    let resolved = pathutil::realpath(p);
    match cache.entries.get(&resolved) {
        Some(info) => cache_info(&resolved).as_ref() == Some(info),
        None => false,
    }
}

/// Adds the results that need no further formatting to the cache and saves it.
pub(crate) fn write_cache(cache: &Cache, results: &[SqlFormatResult], mode: &Mode) -> Result<(), Fatal> {
    let file = get_cache_file();
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|e| Fatal::new(format!("cannot create {}: {e}", dir.display())))?;
    }
    let mut entries = cache.entries.clone();
    for res in results {
        let SourcePath::File(p) = &res.source_path else { continue };
        if !should_update_cache(res, mode) {
            continue;
        }
        let resolved = pathutil::realpath(p);
        let info = cache_info(&resolved).ok_or_else(|| Fatal::new(format!("cannot stat {resolved}")))?;
        entries.insert(resolved, info);
    }
    let mut lines: Vec<String> =
        entries.iter().map(|(path, (mtime, size))| format!("{mtime}\t{size}\t{}", escape(path))).collect();
    lines.sort();
    let mut text = lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    fs::write(&file, text).map_err(|e| Fatal::new(format!("cannot write {}: {e}", file.display())))
}

/// Deletes the cache file, if it exists.
pub(crate) fn clear_cache() -> Result<(), Fatal> {
    match fs::remove_file(get_cache_file()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Fatal::new(format!("cannot remove the cache: {e}"))),
    }
}

fn cache_info(path: &str) -> Option<CacheInfo> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let mtime = match modified.duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_nanos() as i128,
        Err(before) => -(before.duration().as_nanos() as i128),
    };
    Some((mtime, meta.len()))
}

/// Files are cached when they had no error, were not themselves cache hits, and are
/// formatted on disk: unchanged, or changed and written back (not in check or diff
/// mode).
fn should_update_cache(result: &SqlFormatResult, mode: &Mode) -> bool {
    if result.exception.is_some() || result.from_cache {
        false
    } else if !result.has_changed() {
        true
    } else {
        !(mode.check || mode.diff)
    }
}

fn escape(path: &str) -> String {
    path.replace('\\', "\\\\").replace('\n', "\\n").replace('\t', "\\t")
}

fn unescape(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next()? {
                '\\' => out.push('\\'),
                'n' => out.push('\n'),
                't' => out.push('\t'),
                _ => return None,
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}
