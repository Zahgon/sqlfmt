//! The public API: format a string, find the files to format, and run sqlfmt over
//! them.
//!
//! Ported from sqlfmt/api.py.

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::Path;

use crate::analyzer::{Analyzer, Flow};
use crate::cache;
use crate::encoding::{self, Codec};
use crate::error::{Fatal, FormatError, SqlfmtError};
use crate::glob;
use crate::mode::Mode;
use crate::pathutil::{self, SourcePath};
use crate::pystr;
use crate::query::Query;
use crate::query_formatter::QueryFormatter;
use crate::report::{Report, SqlFormatResult};
use crate::tokens::TokenType;

fn flow_to_error(flow: Flow) -> FormatError {
    match flow {
        Flow::User(e) => FormatError::User(e),
        Flow::Fatal(e) => FormatError::Fatal(e),
        Flow::Stop => FormatError::Fatal(Fatal::new("StopRulesetLexing")),
    }
}

/// Formats a query string. Fails with a user-code error if the string cannot be
/// formatted; unless the mode is fast (or check/diff), the result is re-lexed and
/// compared with the input as a safety check.
pub fn format_string(source: &str, mode: &Mode) -> Result<String, FormatError> {
    let mut analyzer = mode.dialect.initialize_analyzer(mode.line_length);
    let raw_query = analyzer.parse_query(source).map_err(flow_to_error)?;
    let raw_types = equivalent_token_types(&raw_query);
    let raw_comments = comment_bodies(&raw_query);
    let formatter = QueryFormatter::new(mode.line_length, mode.dialect.case_sensitive_names());
    let formatted = formatter.format(raw_query)?;
    let result = formatted.render();
    if !mode.fast && !mode.check && !mode.diff {
        perform_safety_check(&mut analyzer, &raw_types, &raw_comments, &result)?;
    }
    Ok(result)
}

fn equivalent_token_types(query: &Query) -> Vec<TokenType> {
    query.tokens().iter().map(|t| t.ttype).filter(|t| t.is_equivalent_in_output()).collect()
}

fn comment_bodies(query: &Query) -> Vec<String> {
    query.lines.iter().flat_map(|l| l.comments.iter().map(|c| c.body())).collect()
}

/// Re-lexes the formatted result and fails if its tokens (other than newlines and
/// comments) or the text of its comments differ from the input's.
fn perform_safety_check(
    analyzer: &mut Analyzer,
    raw_types: &[TokenType],
    raw_comments: &[String],
    result: &str,
) -> Result<(), FormatError> {
    let result_query = analyzer.parse_query(result).map_err(flow_to_error)?;
    let result_types = equivalent_token_types(&result_query);
    if raw_types != result_types.as_slice() {
        let longest = raw_types.len().max(result_types.len());
        let show = |t: Option<&TokenType>| t.map_or("None".to_string(), |t| t.to_string());
        let (pos, raw, res) = (0..longest)
            .find(|&i| raw_types.get(i) != result_types.get(i))
            .map(|i| (i, show(raw_types.get(i)), show(result_types.get(i))))
            .unwrap_or((0, String::new(), String::new()));
        return Err(SqlfmtError::equivalence(format!(
            "There was a problem formatting your query that caused the safety check to fail. Please open an \
             issue. Raw query was {} tokens; formatted query was {} tokens. First mismatching token at position \
             {pos}: raw: {raw}; result: {res}.",
            raw_types.len(),
            result_types.len()
        ))
        .into());
    }
    let stripped_raw: Vec<char> =
        raw_comments.iter().flat_map(|c| pystr::remove_whitespace(c).chars().collect::<Vec<_>>()).collect();
    let result_comments = comment_bodies(&result_query);
    let stripped_res: Vec<char> =
        result_comments.iter().flat_map(|c| pystr::remove_whitespace(c).chars().collect::<Vec<_>>()).collect();
    if stripped_raw != stripped_res {
        let longest = stripped_raw.len().max(stripped_res.len());
        // characters are compared as distinct objects: equal characters outside
        // Latin-1 still count as a mismatch
        let differs = |i: usize| match (stripped_raw.get(i), stripped_res.get(i)) {
            (Some(a), Some(b)) => a != b || u32::from(*a) > 0xff,
            (None, None) => false,
            _ => true,
        };
        let window = |s: &[char], i: usize| s.iter().skip(i).take(25).collect::<String>();
        let (pos, raw, res) = (0..longest)
            .find(|&i| differs(i))
            .map(|i| (i, window(&stripped_raw, i), window(&stripped_res, i)))
            .unwrap_or((0, String::new(), String::new()));
        return Err(SqlfmtError::equivalence(format!(
            "There was a problem formatting your query that caused the safety check to fail. Please open an \
             issue. Raw query had {} comment characters; formatted query had {} comment characters. First \
             mismatching character at position {pos}: raw: {raw}; result: {res}.",
            stripped_raw.len(),
            stripped_res.len()
        ))
        .into());
    }
    Ok(())
}

/// The files to format: the given paths ("-" for stdin), the `.sql` and `.sql.jinja`
/// files in the given directories, minus the files matched by the exclude globs.
pub fn get_matching_paths(paths: &[SourcePath], mode: &Mode) -> Result<BTreeSet<SourcePath>, Fatal> {
    let mut include = BTreeSet::new();
    included_paths(paths.iter().cloned(), mode, &mut include)?;
    let mut exclude = BTreeSet::new();
    for pattern in &mode.exclude {
        let full = if Path::new(pattern).is_absolute() {
            pattern.clone()
        } else {
            let root = match &mode.exclude_root {
                Some(root) => root.clone(),
                None => std::env::current_dir()
                    .map_err(|e| Fatal::new(format!("cannot read the working directory: {e}")))?,
            };
            pathutil::join(&root, pattern)
        };
        for hit in glob::glob(&full) {
            exclude.insert(SourcePath::File(pathutil::normalize(&hit)));
        }
    }
    Ok(include.difference(&exclude).cloned().collect())
}

fn included_paths(
    paths: impl Iterator<Item = SourcePath>,
    mode: &Mode,
    include: &mut BTreeSet<SourcePath>,
) -> Result<(), Fatal> {
    for p in paths {
        match &p {
            SourcePath::Stdin => {
                include.insert(p);
            }
            SourcePath::File(path) => {
                let as_path = Path::new(path);
                if as_path.is_file() && mode.sql_extensions.iter().any(|ext| path.ends_with(ext.as_str())) {
                    include.insert(p.clone());
                } else if as_path.is_dir() {
                    let entries = fs::read_dir(as_path)
                        .and_then(|dir| {
                            dir.map(|e| e.map(|e| SourcePath::File(pathutil::join_name(path, &e.file_name()))))
                                .collect::<std::io::Result<Vec<_>>>()
                        })
                        .map_err(|e| Fatal::new(format!("cannot list {path}: {e}")))?;
                    included_paths(entries.into_iter(), mode, include)?;
                }
            }
        }
    }
    Ok(())
}

/// Formats every path (reading the cache unless it is reset), writes changed files
/// back unless the mode is check or diff, updates the cache and returns the report.
pub fn run(files: &BTreeSet<SourcePath>, mode: &Mode) -> Result<Report, Fatal> {
    let cache = if mode.reset_cache {
        cache::clear_cache()?;
        cache::Cache::default()
    } else {
        cache::load_cache()
    };
    let results = format_many(files, &cache, mode)?;
    if !(mode.check || mode.diff) {
        update_source_files(&results)?;
    }
    cache::write_cache(&cache, &results, mode)?;
    Ok(Report::new(results, mode.clone()))
}

/// Formats the paths that are not cached as unchanged. When more than one path needs
/// formatting (and the mode is not single-process), every path is formatted, cached
/// ones included, in worker processes whose standard input is empty.
fn format_many(paths: &BTreeSet<SourcePath>, cache: &cache::Cache, mode: &Mode) -> Result<Vec<SqlFormatResult>, Fatal> {
    let mut results = Vec::new();
    let mut misses = Vec::new();
    for path in paths {
        if cache::check_cache(cache, path) {
            results.push(SqlFormatResult::from_cache(path.clone()));
        } else {
            misses.push(path.clone());
        }
    }
    if misses.len() > 1 && !mode.single_process {
        for path in paths {
            results.push(format_one(path, mode, false)?);
        }
    } else {
        for path in &misses {
            results.push(format_one(path, mode, true)?);
        }
    }
    Ok(results)
}

fn format_one(path: &SourcePath, mode: &Mode, stdin_available: bool) -> Result<SqlFormatResult, Fatal> {
    let (source, encoding, bom) = read_path_or_stdin(path, mode, stdin_available)?;
    match format_string(&source, mode) {
        Ok(formatted) => Ok(SqlFormatResult::new(path.clone(), source, formatted, encoding, bom, None)),
        Err(FormatError::User(e)) => {
            Ok(SqlFormatResult::new(path.clone(), source, String::new(), encoding, bom, Some(e)))
        }
        Err(FormatError::Fatal(e)) => Err(e),
    }
}

/// Writes the formatted text of every changed file back (not stdin, not errors).
fn update_source_files(results: &[SqlFormatResult]) -> Result<(), Fatal> {
    for res in results {
        if let SourcePath::File(path) = &res.source_path {
            if res.has_changed() && !res.formatted_string.is_empty() {
                let codec = Codec::lookup(&res.encoding).ok_or_else(|| unknown_encoding(&res.encoding))?;
                let bytes = codec.encode(&res.formatted_string).map_err(|c| {
                    Fatal::new(format!("UnicodeEncodeError: '{}' codec can't encode character {c:?}", res.encoding))
                })?;
                fs::write(path, bytes).map_err(|e| Fatal::new(format!("cannot write {path}: {e}")))?;
            }
        }
    }
    Ok(())
}

fn unknown_encoding(name: &str) -> Fatal {
    Fatal::new(format!("LookupError: unknown encoding: {name}"))
}

/// The encoding name as sqlfmt uses it: "inherit" means the locale's encoding, and
/// names are lowercased with hyphens as underscores.
fn effective_encoding(mode: &Mode) -> String {
    let name = if mode.encoding.to_lowercase() == "inherit" { "UTF-8".to_string() } else { mode.encoding.clone() };
    name.to_lowercase().replace('-', "_")
}

/// The source text, the encoding used and the byte order mark that was removed.
fn read_path_or_stdin(
    path: &SourcePath,
    mode: &Mode,
    stdin_available: bool,
) -> Result<(String, String, String), Fatal> {
    let encoding = effective_encoding(mode);
    match path {
        SourcePath::Stdin => {
            let mut bytes = Vec::new();
            if stdin_available {
                std::io::stdin().read_to_end(&mut bytes).map_err(|e| Fatal::new(format!("cannot read stdin: {e}")))?;
            }
            let text = String::from_utf8(bytes).map_err(|e| {
                Fatal::new(format!(
                    "UnicodeDecodeError: 'utf-8' codec can't decode stdin at position {}",
                    e.utf8_error().valid_up_to()
                ))
            })?;
            // standard input is read as is: its line endings are not translated
            Ok((text, encoding, String::new()))
        }
        SourcePath::File(p) => {
            let codec = Codec::lookup(&encoding).ok_or_else(|| unknown_encoding(&encoding))?;
            let bytes = fs::read(p).map_err(|e| Fatal::new(format!("cannot read {p}: {e}")))?;
            let decoded = codec.decode(&bytes).map_err(|(pos, reason)| {
                let err = SqlfmtError::new(
                    crate::error::ErrorKind::Unicode,
                    format!(
                        "Error reading file {p}\nFile could not be decoded using {encoding}. Specifically, {} at position \
                         {pos} failed with: {reason}.\nYou can specify a different encoding by running sqlfmt with the \
                         --encoding option. Or set --encoding to 'none' to use the system default encoding. We suggest \
                         always using utf-8 for all files.",
                        pystr::repr_bytes(&bytes)
                    ),
                );
                Fatal::from(err)
            })?;
            let mut source = encoding::translate_newlines(&decoded);
            let mut bom = String::new();
            if encoding.starts_with("utf") && encoding != "utf_8_sig" {
                let key = encoding.replace('_', "");
                let has_bom_key =
                    matches!(key.as_str(), "utf" | "utf8" | "utf16le" | "utf16be" | "utf32le" | "utf32be");
                if has_bom_key && source.starts_with('\u{feff}') {
                    bom = "\u{feff}".to_string();
                    source = source['\u{feff}'.len_utf8()..].to_string();
                }
            }
            Ok((source, encoding, bom))
        }
    }
}
