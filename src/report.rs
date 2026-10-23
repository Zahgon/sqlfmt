//! Results of a run and the report printed to stderr.
//!
//! Ported from sqlfmt/report.py.

use std::io::{IsTerminal, Write};
use std::sync::OnceLock;

use fancy_regex::Regex;

use crate::difflib;
use crate::error::SqlfmtError;
use crate::mode::Mode;
use crate::pathutil::SourcePath;
use crate::pyre;
use crate::pystr;

/// ANSI color names used in the report.
#[derive(Debug, Clone, Copy)]
pub enum Color {
    Red,
    Green,
    Cyan,
    White,
    BrightBlack,
}

impl Color {
    fn code(self) -> u8 {
        match self {
            Color::Red => 31,
            Color::Green => 32,
            Color::Cyan => 36,
            Color::White => 37,
            Color::BrightBlack => 90,
        }
    }
}

/// Wraps text in ANSI style codes.
pub fn style_output(msg: &str, fg: Option<Color>, bg: Option<Color>, bold: bool) -> String {
    let mut out = String::new();
    if let Some(fg) = fg {
        out.push_str(&format!("\x1b[{}m", fg.code()));
    }
    if let Some(bg) = bg {
        out.push_str(&format!("\x1b[{}m", fg_to_bg(bg.code())));
    }
    out.push_str(if bold { "\x1b[1m" } else { "\x1b[22m" });
    out.push_str(msg);
    out.push_str("\x1b[0m");
    out
}

fn fg_to_bg(code: u8) -> u8 {
    code + 10
}

/// Removes ANSI escape sequences.
pub fn unstyle_output(msg: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| pyre::compile_with_flags(r"\x1b\[[;?0-9]*[a-zA-Z]", ""));
    re.replace_all(msg, "").into_owned()
}

/// Prints to stderr (or stdout), with a trailing newline unless `nl` is false. ANSI
/// styles are removed when the stream is not a terminal. A failed write (such as a
/// closed pipe) is returned to the caller, which ends the run with exit status 1.
pub fn display_output(msg: &str, err: bool, nl: bool) -> std::io::Result<()> {
    let mut text = msg.to_string();
    if nl {
        text.push('\n');
    }
    if err {
        let stderr = std::io::stderr();
        if !stderr.is_terminal() {
            text = unstyle_output(&text);
        }
        let mut lock = stderr.lock();
        lock.write_all(text.as_bytes()).and_then(|()| lock.flush())
    } else {
        let stdout = std::io::stdout();
        if !stdout.is_terminal() {
            text = unstyle_output(&text);
        }
        let mut lock = stdout.lock();
        lock.write_all(text.as_bytes()).and_then(|()| lock.flush())
    }
}

/// A summary of what sqlfmt did to a single file.
#[derive(Debug, Clone)]
pub struct SqlFormatResult {
    pub source_path: SourcePath,
    pub source_string: String,
    pub formatted_string: String,
    pub encoding: String,
    pub utf_bom: String,
    pub exception: Option<SqlfmtError>,
    pub from_cache: bool,
    pub display_path: String,
}

impl SqlFormatResult {
    pub fn new(
        source_path: SourcePath,
        source_string: String,
        formatted_string: String,
        encoding: String,
        utf_bom: String,
        exception: Option<SqlfmtError>,
    ) -> SqlFormatResult {
        let display_path = source_path.display();
        SqlFormatResult {
            source_path,
            source_string,
            formatted_string,
            encoding,
            utf_bom,
            exception,
            from_cache: false,
            display_path,
        }
    }

    /// The result for a file found unchanged in the cache.
    pub fn from_cache(source_path: SourcePath) -> SqlFormatResult {
        let mut res =
            SqlFormatResult::new(source_path, String::new(), String::new(), String::new(), String::new(), None);
        res.from_cache = true;
        res
    }

    /// Prints the formatted query to stdout if it came from stdin.
    pub fn maybe_print_to_stdout(&self) -> std::io::Result<()> {
        if self.source_path == SourcePath::Stdin {
            display_output(&self.formatted_string, false, false)?;
        }
        Ok(())
    }

    pub fn has_changed(&self) -> bool {
        self.source_string != self.formatted_string
    }

    pub fn has_error(&self) -> bool {
        self.exception.is_some()
    }
}

/// The results of a run, printed as a summary to stderr.
#[derive(Debug, Clone)]
pub struct Report {
    pub results: Vec<SqlFormatResult>,
    pub mode: Mode,
}

impl Report {
    pub fn new(results: Vec<SqlFormatResult>, mode: Mode) -> Report {
        Report { results, mode }
    }

    fn pluralize_file(n: usize) -> String {
        format!("{n} file{}", if n != 1 { "s" } else { "" })
    }

    /// The report text: counts, errors, changed files, unchanged files (verbose) and
    /// diffs (diff mode).
    pub fn render(&self) -> String {
        let (formatted, unchanged) = if self.mode.check {
            ("failed formatting check", "passed formatting check")
        } else if self.mode.diff {
            ("would be formatted", "would be left unchanged")
        } else {
            ("formatted", "left unchanged")
        };
        let mut report: Vec<String> = Vec::new();
        if !self.mode.quiet {
            if self.number_errored() > 0 {
                let msg = format!("{} had errors while formatting.", Self::pluralize_file(self.number_errored()));
                report.push(style_output(&msg, Some(Color::Red), None, true));
            }
            if self.number_changed() > 0 {
                let msg = format!("{} {formatted}.", Self::pluralize_file(self.number_changed()));
                report.push(style_output(&msg, None, None, true));
            }
            report.push(format!("{} {unchanged}.", Self::pluralize_file(self.number_unchanged())));
        }
        let max_errors = if self.mode.quiet { 1 } else { 50 };
        for res in self.errored_results().into_iter().take(max_errors) {
            let err = res.exception.as_ref().map(|e| e.to_string()).unwrap_or_default();
            report.push(format!("{}\n    {}", res.display_path, style_output(&err, Some(Color::Red), None, false)));
        }
        if !self.mode.quiet {
            for res in self.changed_results() {
                report.push(format!("{} {formatted}.", res.display_path));
            }
        }
        if self.mode.verbose {
            for res in self.unchanged_results() {
                report.push(format!("{} {unchanged}.", res.display_path));
            }
        }
        if self.mode.diff {
            for res in self.changed_results() {
                report.push(Self::generate_diff(res));
            }
        }
        let msg = report.join("\n");
        if self.mode.color() {
            msg
        } else {
            unstyle_output(&msg)
        }
    }

    /// A unified diff of the source and formatted strings of a result.
    fn generate_diff(result: &SqlFormatResult) -> String {
        let (from, to) = if result.source_path == SourcePath::Stdin {
            ("STDIN".to_string(), "STDOUT".to_string())
        } else {
            (result.display_path.clone(), result.display_path.clone())
        };
        let a = pystr::splitlines(&result.source_string, true);
        let b = pystr::splitlines(&result.formatted_string, true);
        let mut out = String::new();
        for line in difflib::unified_diff(&a, &b, &from, &to, "(Source Query)", "(Formatted Query)", 3) {
            if line.ends_with('\n') {
                out.push_str(&Self::style_diff_line(&line));
            } else {
                out.push_str(&Self::style_diff_line(&format!("{line}\n")));
                out.push_str(&Self::style_diff_line("\\ No newline at end of file\n"));
            }
        }
        out
    }

    fn style_diff_line(line: &str) -> String {
        if line.starts_with("@@") {
            style_output(line, Some(Color::Cyan), None, false)
        } else if line.starts_with('+') {
            style_output(line, Some(Color::Green), None, false)
        } else if line.starts_with('-') {
            style_output(line, Some(Color::Red), None, false)
        } else {
            line.to_string()
        }
    }

    /// Prints formatted stdin input to stdout (not in check or diff mode), then the
    /// report to stderr (nothing when the report is empty).
    pub fn display_report(&self) -> std::io::Result<()> {
        if !self.mode.check && !self.mode.diff {
            for res in &self.results {
                res.maybe_print_to_stdout()?;
            }
        }
        let report = self.render();
        if !report.is_empty() {
            display_output(&report, true, true)?;
        }
        Ok(())
    }

    fn filtered(&self, has_changed: bool, has_error: bool) -> Vec<&SqlFormatResult> {
        let mut out: Vec<&SqlFormatResult> =
            self.results.iter().filter(|r| r.has_changed() == has_changed && r.has_error() == has_error).collect();
        out.sort_by(|a, b| a.source_path.cmp(&b.source_path));
        out
    }

    pub fn changed_results(&self) -> Vec<&SqlFormatResult> {
        self.filtered(true, false)
    }

    pub fn unchanged_results(&self) -> Vec<&SqlFormatResult> {
        self.filtered(false, false)
    }

    pub fn errored_results(&self) -> Vec<&SqlFormatResult> {
        self.filtered(true, true)
    }

    pub fn number_changed(&self) -> usize {
        self.changed_results().len()
    }

    pub fn number_unchanged(&self) -> usize {
        self.unchanged_results().len()
    }

    pub fn number_errored(&self) -> usize {
        self.errored_results().len()
    }
}
