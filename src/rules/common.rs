//! Pattern fragments shared by several rule sets.
//!
//! Ported from sqlfmt/rules/common.py.

/// Creates grouped alternatives: `(a|b|c)`.
pub(crate) fn group(choices: &[&str]) -> String {
    format!("({})", choices.join("|"))
}

pub(crate) const NEWLINE: &str = r"\r?\n";

pub(crate) fn eol() -> String {
    group(&[NEWLINE, r"$"])
}

pub(crate) fn jinja_start() -> String {
    group(&[r"\{[{%#]"])
}

pub(crate) fn sql_quoted_exp() -> String {
    group(&[
        // tripled single quotes (optionally raw/bytes). The body matches quote runs
        // explicitly and refuses a closer that is itself followed by another quote, so
        // that `select '''' || 'x' || ''''` is not read as one triple-quoted string;
        // the escaped-single-quote alternative below gets its turn instead.
        r"(rb?|b|br)?'''(?:[^'](?:[^']|'(?!''))*)?'''(?!')",
        // tripled double quotes follow the same quote-run rule as single quotes
        r#"(rb?|b|br)?"""(?:[^"](?:[^"]|"(?!""))*)?"""(?!")"#,
        // possibly escaped double quotes
        r#"(rb?|b|br|u&|@)?"([^"\\]*(\\.[^"\\]*|""[^"\\]*)*)""#,
        // possibly escaped single quotes
        r"(rb?|b|br|u&|x)?'([^'\\]*(\\.[^'\\]*|''[^'\\]*)*)'",
        // pg dollar-delimited strings
        r"\$(?P<tag>\w*)\$.*?\$(?P=tag)\$",
        // possibly escaped backtick
        r"`([^`\\]*(\\.[^`\\]*)*)`",
    ])
}

pub(crate) const SQL_COMMENT_START: &str = r"(?=--|#|//|/\*)";

pub(crate) fn sql_comment() -> String {
    group(&[
        r"--[^\r\n]*",
        r"#[^\r\n]*",
        // snowflake's js-style double-slash comment
        r"//[^\r\n]*",
        // simple block comment
        r"/\*[^*]*\*+(?:[^/*][^*]*\*+)*/",
    ])
}

pub(crate) const CREATE_FUNCTION: &str = concat!(
    r"create(\s+or\s+replace)?(\s+temp(orary)?)?(\s+secure)?",
    r"(\s+external)?(\s+table)?",
    r"\s+function(\s+if\s+not\s+exists)?"
);

pub(crate) const ALTER_DROP_FUNCTION: &str = r"(alter|drop)\s+function(\s+if\s+exists)?";

pub(crate) const CREATE_WAREHOUSE: &str = r"create(\s+or\s+replace)?\s+warehouse(\s+if\s+not\s+exists)?";

pub(crate) const ALTER_WAREHOUSE: &str = r"alter\s+warehouse(\s+if\s+exists)?";

pub(crate) fn create_clonable() -> String {
    format!(
        r"create(\s+or\s+replace)?\s+{}(\s+if\s+not\s+exists)?",
        group(&[r"database", r"schema", r"table", r"stage", r"file\s+format", r"sequence", r"stream", r"task",])
    )
}

pub(crate) fn pragma_set_call() -> String {
    group(&[r"pragma", r"set", r"call"])
}

/// Any whitespace except a newline; every rule's pattern is prefixed with it.
pub(crate) const MAYBE_WHITESPACES: &str = r"[^\S\n]*";
