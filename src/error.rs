//! Errors raised while reading, configuring and formatting.
//!
//! Ported from sqlfmt/exception.py.

use std::fmt;

/// The kinds of error sqlfmt reports for user code and configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Config,
    Unicode,
    Parsing,
    Bracket,
    Segment,
    Equivalence,
}

/// An error in user code (or configuration) that sqlfmt reports to the user. Displayed
/// as "sqlfmt encountered an error: <message>".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlfmtError {
    pub kind: ErrorKind,
    pub message: String,
}

impl SqlfmtError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> SqlfmtError {
        SqlfmtError { kind, message: message.into() }
    }

    pub fn config(message: impl Into<String>) -> SqlfmtError {
        SqlfmtError::new(ErrorKind::Config, message)
    }

    pub fn parsing(message: impl Into<String>) -> SqlfmtError {
        SqlfmtError::new(ErrorKind::Parsing, message)
    }

    pub fn bracket(message: impl Into<String>) -> SqlfmtError {
        SqlfmtError::new(ErrorKind::Bracket, message)
    }

    pub fn segment(message: impl Into<String>) -> SqlfmtError {
        SqlfmtError::new(ErrorKind::Segment, message)
    }

    pub fn equivalence(message: impl Into<String>) -> SqlfmtError {
        SqlfmtError::new(ErrorKind::Equivalence, message)
    }

    /// The name of the error class, as printed when the error stops the program.
    pub fn class_name(&self) -> &'static str {
        match self.kind {
            ErrorKind::Config => "SqlfmtConfigError",
            ErrorKind::Unicode => "SqlfmtUnicodeError",
            ErrorKind::Parsing => "SqlfmtParsingError",
            ErrorKind::Bracket => "SqlfmtBracketError",
            ErrorKind::Segment => "SqlfmtSegmentError",
            ErrorKind::Equivalence => "SqlfmtEquivalenceError",
        }
    }
}

impl fmt::Display for SqlfmtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sqlfmt encountered an error: {}", self.message)
    }
}

impl std::error::Error for SqlfmtError {}

/// A failure that stops the whole run instead of being reported per file: a defect
/// in sqlfmt's own handling of an input, or an error that sqlfmt does not catch.
/// The run ends with exit status 1 and nothing is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fatal {
    pub message: String,
}

impl Fatal {
    pub fn new(message: impl Into<String>) -> Fatal {
        Fatal { message: message.into() }
    }
}

impl fmt::Display for Fatal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Fatal {}

impl From<SqlfmtError> for Fatal {
    fn from(e: SqlfmtError) -> Fatal {
        Fatal::new(format!("{}: {}", e.class_name(), e))
    }
}

/// The outcome of formatting one source string: either a user-code error that is
/// reported for that file, or a fatal failure that stops the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    User(SqlfmtError),
    Fatal(Fatal),
}

impl From<SqlfmtError> for FormatError {
    fn from(e: SqlfmtError) -> FormatError {
        FormatError::User(e)
    }
}

impl From<Fatal> for FormatError {
    fn from(e: Fatal) -> FormatError {
        FormatError::Fatal(e)
    }
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatError::User(e) => e.fmt(f),
            FormatError::Fatal(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for FormatError {}
