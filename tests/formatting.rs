//! Formatting of the test data corpus (ported from tests/functional_tests/test_general_formatting.py).

mod common;

use sqlfmt::api::format_string;
use sqlfmt::mode::Mode;

use common::read_test_data;

/// The expected output of a data file. Some expected outputs in `tests/data` were produced with sqlfmt's optional
/// `jinjafmt` extra (black reformats the code inside the jinja tags); for those files, `tests/data/without_jinjafmt`
/// holds the same source with the output sqlfmt gives without that extra, which is what this port produces.
fn expected_output(path: &str) -> (String, String) {
    let without = format!("without_jinjafmt/{path}");
    if common::data_dir().join(&without).is_file() {
        read_test_data(&without)
    } else {
        read_test_data(path)
    }
}

fn data_files() -> Vec<String> {
    let mut files = Vec::new();
    for dir in ["preformatted", "unformatted"] {
        for entry in std::fs::read_dir(common::data_dir().join(dir)).expect("read a data directory") {
            let name = entry.expect("a directory entry").file_name().to_string_lossy().to_string();
            if name.ends_with(".sql") {
                files.push(format!("{dir}/{name}"));
            }
        }
    }
    files.sort();
    files
}

#[test]
fn formats_the_corpus() {
    let mode = Mode::default();
    let mut failures = Vec::new();
    for path in data_files() {
        let (source, expected) = expected_output(&path);
        let actual = format_string(&source, &mode).unwrap_or_else(|e| panic!("{path}: {e}"));
        let second = format_string(&actual, &mode).unwrap_or_else(|e| panic!("{path} (second pass): {e}"));
        if second != actual {
            failures.push(format!("{path}: formatting is not stable"));
        }
        if actual != expected {
            failures.push(format!("{path}: output differs from the expected output\n{actual}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn fast_mode_formats_without_the_safety_check() {
    let mode = Mode { fast: true, ..Mode::default() };
    let (source, expected) = read_test_data("unformatted/100_select_case.sql");
    assert_eq!(format_string(&source, &mode).expect("formats"), expected);
}

#[test]
fn clickhouse_keeps_the_case_of_names() {
    let mut mode = Mode::default();
    mode.set_dialect("clickhouse").expect("a known dialect");
    let formatted = format_string("SELECT myField, COUNT(*) FROM MyTable\n", &mode).expect("formats");
    assert_eq!(formatted, "select myField, COUNT(*) from MyTable\n");
    let polyglot = format_string("SELECT myField, COUNT(*) FROM MyTable\n", &Mode::default()).expect("formats");
    assert_eq!(polyglot, "select myfield, count(*) from mytable\n");
}

#[test]
fn line_length_controls_merging() {
    let source = "select aaaaaaaaaa, bbbbbbbbbb, cccccccccc from t\n";
    let wide = Mode { line_length: 88, ..Mode::default() };
    assert_eq!(format_string(source, &wide).expect("formats"), source);
    let narrow = Mode { line_length: 20, ..Mode::default() };
    assert_eq!(
        format_string(source, &narrow).expect("formats"),
        "select aaaaaaaaaa, bbbbbbbbbb, cccccccccc\nfrom t\n".replace(
            "select aaaaaaaaaa, bbbbbbbbbb, cccccccccc\n",
            "select\n    aaaaaaaaaa,\n    bbbbbbbbbb,\n    cccccccccc\n"
        )
    );
}

#[test]
fn errors_name_the_position() {
    let err = format_string("select (1))\n", &Mode::default()).expect_err("unbalanced brackets fail");
    assert_eq!(
        err.to_string(),
        "sqlfmt encountered an error: Closing bracket ')' found at 10 before bracket was opened."
    );
    let err = format_string("select ü\\\n", &Mode::default()).expect_err("a backslash does not lex");
    assert_eq!(err.to_string(), "sqlfmt encountered an error: Could not parse SQL at position 8: '\\'");
}
