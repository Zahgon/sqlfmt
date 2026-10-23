//! Mode: every formatting and reporting option of a run, and the SQL dialects.
//!
//! Ported from sqlfmt/mode.py and sqlfmt/dialect.py.

use std::path::PathBuf;

use crate::analyzer::Analyzer;
use crate::error::SqlfmtError;
use crate::node_manager::NodeManager;
use crate::rules::{self, RuleSet};

/// A SQL dialect: the polyglot dialect covers nearly all SQL; ClickHouse is the
/// polyglot grammar with case-sensitive names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Polyglot,
    ClickHouse,
}

impl Dialect {
    /// The dialect for a name (case-insensitive).
    pub fn from_name(name: &str) -> Result<Dialect, SqlfmtError> {
        match name.to_lowercase().as_str() {
            "polyglot" => Ok(Dialect::Polyglot),
            "clickhouse" => Ok(Dialect::ClickHouse),
            _ => Err(SqlfmtError::config(format!(
                "Mode was created with dialect_name={name}, which is not supported. Did you mean 'polyglot'?"
            ))),
        }
    }

    pub fn case_sensitive_names(self) -> bool {
        self == Dialect::ClickHouse
    }

    /// An analyzer that lexes with the dialect's rules.
    pub(crate) fn initialize_analyzer(self, line_length: i64) -> Analyzer {
        Analyzer::new(line_length, rules::rules(RuleSet::Main), NodeManager::new(self.case_sensitive_names()))
    }
}

/// The options of a run (see the command-line help for what each one does).
#[derive(Debug, Clone)]
pub struct Mode {
    pub sql_extensions: Vec<String>,
    pub dialect_name: String,
    pub dialect: Dialect,
    pub line_length: i64,
    pub check: bool,
    pub diff: bool,
    pub exclude: Vec<String>,
    pub exclude_root: Option<PathBuf>,
    pub encoding: String,
    pub fast: bool,
    pub single_process: bool,
    pub no_jinjafmt: bool,
    pub reset_cache: bool,
    pub verbose: bool,
    pub quiet: bool,
    pub no_progressbar: bool,
    pub no_color: bool,
    pub force_color: bool,
}

impl Default for Mode {
    fn default() -> Mode {
        Mode {
            sql_extensions: vec![".sql".to_string(), ".sql.jinja".to_string()],
            dialect_name: "polyglot".to_string(),
            dialect: Dialect::Polyglot,
            line_length: 88,
            check: false,
            diff: false,
            exclude: Vec::new(),
            exclude_root: None,
            encoding: "utf-8".to_string(),
            fast: false,
            single_process: false,
            no_jinjafmt: false,
            reset_cache: false,
            verbose: false,
            quiet: false,
            no_progressbar: false,
            no_color: false,
            force_color: false,
        }
    }
}

impl Mode {
    /// Sets the dialect from its name; fails for an unsupported dialect.
    pub fn set_dialect(&mut self, name: &str) -> Result<(), SqlfmtError> {
        self.dialect = Dialect::from_name(name)?;
        self.dialect_name = name.to_string();
        Ok(())
    }

    /// Whether output is colorized: --force-color, then --no-color, then the
    /// NO_COLOR environment variable; colorized by default.
    pub fn color(&self) -> bool {
        if self.force_color {
            true
        } else if self.no_color {
            false
        } else {
            std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
        }
    }
}
