//! Configuration from the `[tool.sqlfmt]` table of a `pyproject.toml` file.
//!
//! Ported from sqlfmt/config.py. Values keep the meaning they have in sqlfmt's
//! configuration model: a flag is on for any "truthy" value (true, a non-zero number,
//! a non-empty string, array or table), `true`/`false` count as 1/0 for the line length,
//! and a string `exclude` is a list of one-character globs.

use std::fs;
use std::path::Path;

use toml::Value;

use crate::error::{Fatal, SqlfmtError};
use crate::pathutil::{self, SourcePath};

/// The options read from a config file, in file order, keyed by option name.
pub(crate) type Config = Vec<(String, ConfigValue)>;

/// A config value: a TOML value, or the directory that relative exclude globs are
/// resolved against (added for files that set `exclude`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ConfigValue {
    Toml(Value),
    ExcludeRoot(String),
}

/// The options sqlfmt accepts, in the order they are listed in error messages.
pub(crate) const MODE_FIELDS: [&str; 17] = [
    "SQL_EXTENSIONS",
    "dialect_name",
    "line_length",
    "check",
    "diff",
    "exclude",
    "exclude_root",
    "encoding",
    "fast",
    "single_process",
    "no_jinjafmt",
    "reset_cache",
    "verbose",
    "quiet",
    "no_progressbar",
    "no_color",
    "force_color",
];

/// Finds and loads the config: `config_path` if given, otherwise the `pyproject.toml`
/// in the deepest common parent directory of the files, or in one of its parents.
pub(crate) fn load_config_file(files: &[SourcePath], config_path: Option<&str>) -> Result<Config, Fatal> {
    let path = match config_path {
        Some(p) => Some(p.to_string()),
        None => find_config_file(&common_parents(files)?),
    };
    match path {
        None => Ok(Vec::new()),
        Some(p) => load_config_from_path(&p).map_err(Fatal::from),
    }
}

/// The search path for a config file: the deepest directory that contains every file
/// (files after a "-" are not considered), then its parents.
fn common_parents(files: &[SourcePath]) -> Result<Vec<String>, Fatal> {
    let mut common: Option<Vec<String>> = None;
    for f in files {
        let SourcePath::File(p) = f else { break };
        let mut parents = pathutil::parents(p);
        if Path::new(p).is_dir() {
            parents.push(p.clone());
        }
        common = Some(match common {
            None => parents,
            Some(c) => c.into_iter().filter(|x| parents.contains(x)).collect(),
        });
    }
    let root = match common.as_ref().and_then(|c| c.iter().max_by_key(|p| pathutil::parents(p).len())) {
        Some(root) => root.clone(),
        None => std::env::current_dir()
            .map(|d| d.to_string_lossy().to_string())
            .map_err(|e| Fatal::new(format!("cannot read the working directory: {e}")))?,
    };
    let mut search = vec![root.clone()];
    search.extend(pathutil::parents(&root));
    Ok(search)
}

fn find_config_file(search_paths: &[String]) -> Option<String> {
    search_paths
        .iter()
        .map(|dir| if dir == "/" { "/pyproject.toml".to_string() } else { format!("{dir}/pyproject.toml") })
        .find(|f| Path::new(f).exists())
}

/// Reads the `[tool.sqlfmt]` table of a TOML file.
fn load_config_from_path(path: &str) -> Result<Config, SqlfmtError> {
    let text = fs::read(path)
        .map_err(|e| SqlfmtError::config(format!("Error opening pyproject.toml config file at {path}. {e}")))?;
    let text = String::from_utf8(text).map_err(|_| {
        SqlfmtError::config(format!("Error decoding pyproject.toml config file at {path}. Check for invalid TOML."))
    })?;
    let doc: toml::Table = text.parse().map_err(|e| {
        SqlfmtError::config(format!("Error decoding pyproject.toml config file at {path}. Check for invalid TOML. {e}"))
    })?;
    let raw: Vec<(String, Value)> = match doc.get("tool") {
        None => Vec::new(),
        Some(Value::Table(tool)) => match tool.get("sqlfmt") {
            None => Vec::new(),
            Some(Value::Table(sqlfmt)) => sqlfmt.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            Some(_) => return Err(SqlfmtError::config("[tool.sqlfmt] must be a table")),
        },
        Some(_) => return Err(SqlfmtError::config("[tool] must be a table")),
    };
    let mut config: Config = raw.into_iter().map(|(k, v)| (k, ConfigValue::Toml(v))).collect();
    let has = |config: &Config, key: &str| config.iter().any(|(k, _)| k == key);
    if has(&config, "exclude") && !has(&config, "exclude_root") {
        let parent = pathutil::parents(path).into_iter().next().unwrap_or_else(|| "/".to_string());
        config.push(("exclude_root".to_string(), ConfigValue::ExcludeRoot(parent)));
    }
    validate_config(config)
}

/// Lowercases keys, renames `dialect` to `dialect_name` and rejects unknown keys.
fn validate_config(raw: Config) -> Result<Config, SqlfmtError> {
    let mut config: Config = Vec::new();
    for (key, value) in raw {
        let key = key.to_lowercase();
        let key = if key == "dialect" { "dialect_name".to_string() } else { key };
        if !MODE_FIELDS.contains(&key.as_str()) {
            let fields: Vec<String> = MODE_FIELDS.iter().map(|f| format!("'{f}'")).collect();
            return Err(SqlfmtError::config(format!(
                "Config file contains key {key}, which is not a supported option. Must be one of [{}]",
                fields.join(", ")
            )));
        }
        set(&mut config, key, value);
    }
    Ok(config)
}

/// Sets a key, replacing an earlier value in place.
pub(crate) fn set(config: &mut Config, key: String, value: ConfigValue) {
    match config.iter_mut().find(|(k, _)| *k == key) {
        Some(entry) => entry.1 = value,
        None => config.push((key, value)),
    }
}

/// Whether a value is "truthy".
pub(crate) fn truthy(value: &Value) -> bool {
    match value {
        Value::Boolean(b) => *b,
        Value::Integer(i) => *i != 0,
        Value::Float(f) => *f != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Table(t) => !t.is_empty(),
        Value::Datetime(_) => true,
    }
}
