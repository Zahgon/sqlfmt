//! The library API: lexing, formatting, file discovery and reports (ported from
//! tests/unit_tests/test_api.py, test_analyzer.py, test_mode.py and test_report.py).

mod common;

use std::collections::BTreeSet;

use sqlfmt::api::{format_string, get_matching_paths};
use sqlfmt::error::{ErrorKind, FormatError};
use sqlfmt::mode::{Dialect, Mode};
use sqlfmt::pathutil::SourcePath;
use sqlfmt::report::{Report, SqlFormatResult};

use common::TempDir;

fn user_error(source: &str) -> ErrorKind {
    match format_string(source, &Mode::default()) {
        Err(FormatError::User(e)) => e.kind,
        other => panic!("expected an error for {source:?}, got {other:?}"),
    }
}

#[test]
fn empty_and_whitespace_strings_format_to_nothing() {
    for mode in [Mode::default(), Mode { check: true, ..Mode::default() }, Mode { fast: true, ..Mode::default() }] {
        assert_eq!(format_string("", &mode).expect("formats"), "");
        assert_eq!(format_string("   \n\n  ", &mode).expect("formats"), "");
    }
}

#[test]
fn bad_strings_report_their_error_kind() {
    assert_eq!(user_error("select )"), ErrorKind::Bracket);
    assert_eq!(user_error("select {{ foo"), ErrorKind::Bracket);
    assert_eq!(user_error("select \\"), ErrorKind::Parsing);
    assert_eq!(user_error("{% endif %}"), ErrorKind::Bracket);
}

#[test]
fn the_safety_check_compares_tokens_after_reformatting() {
    // the create function rule set is still active when the result is re-lexed
    let source = "select 1;\ncreate function f() returns int as 'select 1'\n";
    let err = format_string(source, &Mode::default()).expect_err("the safety check fails");
    assert_eq!(
        err.to_string(),
        "sqlfmt encountered an error: There was a problem formatting your query that caused the safety check to \
         fail. Please open an issue. Raw query was 11 tokens; formatted query was 11 tokens. First mismatching token \
         at position 0: raw: TokenType.UNTERM_KEYWORD; result: TokenType.NAME."
    );
    let fast = Mode { fast: true, ..Mode::default() };
    assert!(format_string(source, &fast).is_ok());
}

#[test]
fn jinja_tags_are_normalised() {
    let formatted = format_string("select {{x}} from {{ref('t')}}\n", &Mode::default()).expect("formats");
    assert_eq!(formatted, "select {{ x }} from {{ ref('t') }}\n");
    let nested = format_string("select {{ {'a': {'b': 1}} }}\n", &Mode::default()).expect("formats");
    assert_eq!(nested, "select {{ {'a': {'b': 1}} }}\n");
}

#[test]
fn file_discovery_finds_sql_files() {
    let dir = TempDir::new("discovery");
    dir.write("top_level_file.sql", "select 1\n");
    dir.write("top_level_file.two.sql", "select 1\n");
    dir.write("a_directory/one_file.sql", "select 1\n");
    dir.write("a_directory/nested_directory/j2_extension.sql.jinja", "select 1\n");
    dir.write("a_directory/nested_directory/not_sql.txt", "select 1\n");
    dir.write("a_directory/UPPER.SQL", "select 1\n");
    let root = SourcePath::File(dir.path.to_string_lossy().to_string());
    let mode = Mode::default();
    let found = get_matching_paths(std::slice::from_ref(&root), &mode).expect("discovery works");
    let names: BTreeSet<String> = found
        .iter()
        .map(|p| match p {
            SourcePath::File(f) => f.trim_start_matches(&*dir.path.to_string_lossy()).to_string(),
            SourcePath::Stdin => "-".to_string(),
        })
        .collect();
    let expected: BTreeSet<String> = [
        "/top_level_file.sql",
        "/top_level_file.two.sql",
        "/a_directory/one_file.sql",
        "/a_directory/nested_directory/j2_extension.sql.jinja",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(names, expected);

    let excluded =
        Mode { exclude: vec!["**/*_file*".to_string()], exclude_root: Some(dir.path.clone()), ..Mode::default() };
    let found = get_matching_paths(&[root], &excluded).expect("discovery works");
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn paths_sort_by_component() {
    let a = SourcePath::File("/w/a/b.sql".to_string());
    let b = SourcePath::File("/w/a-b.sql".to_string());
    let c = SourcePath::Stdin;
    let mut paths = vec![c.clone(), b.clone(), a.clone()];
    paths.sort();
    assert_eq!(paths, vec![a, b, c]);
}

fn result(path: &str, source: &str, formatted: &str) -> SqlFormatResult {
    SqlFormatResult::new(
        SourcePath::File(path.to_string()),
        source.to_string(),
        formatted.to_string(),
        "utf_8".to_string(),
        String::new(),
        None,
    )
}

#[test]
fn reports_summarise_results() {
    let results = vec![
        result("/nowhere/b.sql", "SELECT 1\n", "select 1\n"),
        result("/nowhere/a.sql", "select 1\n", "select 1\n"),
    ];
    let report = Report::new(results.clone(), Mode::default());
    assert_eq!(
        report.render(),
        "\u{1b}[1m1 file formatted.\u{1b}[0m\n1 file left unchanged.\n/nowhere/b.sql formatted."
    );
    let verbose_check =
        Report::new(results.clone(), Mode { check: true, verbose: true, no_color: true, ..Mode::default() });
    assert_eq!(
        verbose_check.render(),
        "1 file failed formatting check.\n1 file passed formatting check.\n/nowhere/b.sql failed formatting check.\n\
         /nowhere/a.sql passed formatting check."
    );
    let diff = Report::new(results, Mode { diff: true, no_color: true, ..Mode::default() });
    assert!(diff.render().ends_with("--- /nowhere/b.sql\t(Source Query)\n+++ /nowhere/b.sql\t(Formatted Query)\n@@ -1 +1 @@\n-SELECT 1\n+select 1\n"));
    let quiet = Report::new(Vec::new(), Mode { quiet: true, ..Mode::default() });
    assert_eq!(quiet.render(), "");
}

#[test]
fn dialects_are_looked_up_by_name() {
    assert_eq!(Dialect::from_name("ClickHouse").expect("known"), Dialect::ClickHouse);
    assert_eq!(
        Dialect::from_name("oracle").expect_err("unknown").to_string(),
        "sqlfmt encountered an error: Mode was created with dialect_name=oracle, which is not supported. Did you mean 'polyglot'?"
    );
    let mode = Mode { force_color: true, no_color: true, ..Mode::default() };
    assert!(mode.color());
}
