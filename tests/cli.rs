//! The `sqlfmt` command line, run as a program (ported from tests/unit_tests/test_cli.py
//! and tests/functional_tests/test_end_to_end.py).

mod common;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use common::TempDir;

/// Runs sqlfmt in `cwd` with a private cache directory and no color settings.
fn sqlfmt(cwd: &Path, args: &[&str], stdin: Option<&str>, env: &[(&str, &str)]) -> Output {
    let home = cwd.join(".test-home");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sqlfmt"));
    cmd.args(args).current_dir(cwd).env("HOME", &home).env_remove("XDG_CACHE_HOME").env_remove("NO_COLOR");
    for (key, _) in std::env::vars() {
        if key.starts_with("SQLFMT_") || key == "COLUMNS" {
            cmd.env_remove(key);
        }
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("sqlfmt starts");
    {
        let mut pipe = child.stdin.take().expect("stdin is piped");
        pipe.write_all(stdin.unwrap_or("").as_bytes()).expect("stdin is written");
    }
    child.wait_with_output().expect("sqlfmt finishes")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn preformatted(dir: &TempDir) -> usize {
    dir.copy_data_dir("fast/preformatted", "pre");
    std::fs::read_dir(dir.path.join("pre")).expect("read").count()
}

fn unformatted(dir: &TempDir) -> usize {
    dir.copy_data_dir("fast/unformatted", "unf");
    std::fs::read_dir(dir.path.join("unf")).expect("read").count()
}

#[test]
fn help_and_version() {
    let dir = TempDir::new("help");
    let out = sqlfmt(&dir.path, &["--help"], None, &[]);
    assert_eq!(out.status.code(), Some(0), "--help exits 0");
    assert!(stdout(&out).starts_with("Usage: sqlfmt [OPTIONS] [FILES]...\n"), "{}", stdout(&out));
    assert!(stdout(&out).contains("  -l, --line-length INTEGER"), "{}", stdout(&out));
    let out = sqlfmt(&dir.path, &["--version"], None, &[]);
    assert_eq!(stdout(&out), "sqlfmt, version 0.32.0\n");
}

#[test]
fn no_arguments_prints_the_welcome_message() {
    let dir = TempDir::new("welcome");
    let out = sqlfmt(&dir.path, &[], None, &[]);
    assert_eq!(out.status.code(), Some(0));
    assert!(stderr(&out).contains("To get started, try:"), "{}", stderr(&out));
    assert!(stdout(&out).is_empty());
}

#[test]
fn stdin_is_formatted_to_stdout() {
    let dir = TempDir::new("stdin");
    let out = sqlfmt(&dir.path, &["-"], Some("SELECT 1\n"), &[]);
    assert_eq!(stdout(&out), "select 1\n");
    assert_eq!(stderr(&out), "1 file formatted.\n0 files left unchanged.\n- formatted.\n");
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn preformatted_files_pass_every_mode() {
    let dir = TempDir::new("pre");
    let count = preformatted(&dir);
    let cases: &[(&[&str], &str)] = &[
        (&[], "left unchanged"),
        (&["--verbose"], "left unchanged"),
        (&["-l", "88"], "left unchanged"),
        (&["--check"], "passed formatting check"),
        (&["--check", "-v"], "passed formatting check"),
        (&["--diff"], "would be left unchanged"),
        (&["--no-color"], "left unchanged"),
        (&["--no-jinjafmt"], "left unchanged"),
        (&["-k"], "left unchanged"),
        (&["--dialect", "clickhouse"], "left unchanged"),
        (&["--fast"], "left unchanged"),
        (&["--safe"], "left unchanged"),
        (&["--single-process"], "left unchanged"),
        (&["--no-progressbar"], "left unchanged"),
        (&["--encoding", "utf-8-sig"], "left unchanged"),
        (&["--encoding", "inherit"], "left unchanged"),
    ];
    for (options, expected) in cases {
        let mut args = vec!["pre"];
        args.extend_from_slice(options);
        let out = sqlfmt(&dir.path, &args, None, &[]);
        let err = stderr(&out);
        assert_eq!(out.status.code(), Some(0), "{options:?}: {err}");
        assert!(err.contains(&format!("{count} files {expected}.")), "{options:?}: {err}");
        if options.contains(&"-v") || options.contains(&"--verbose") {
            assert!(err.contains("pre/001_select_1.sql"), "{options:?}: {err}");
        }
    }
}

#[test]
fn quiet_runs_print_nothing() {
    let dir = TempDir::new("quiet");
    preformatted(&dir);
    for options in [&["-q"][..], &["--quiet"], &["--check", "-q"], &["--diff", "-q"]] {
        let mut args = vec!["pre"];
        args.extend_from_slice(options);
        let out = sqlfmt(&dir.path, &args, None, &[]);
        assert_eq!(stderr(&out), "", "{options:?}");
        assert_eq!(out.status.code(), Some(0), "{options:?}");
    }
}

#[test]
fn check_and_diff_fail_on_unformatted_files() {
    let dir = TempDir::new("unf");
    let count = unformatted(&dir);
    let cases: &[(&[&str], &str)] = &[
        (&["--check"], "failed formatting check"),
        (&["--check", "-v"], "failed formatting check"),
        (&["--diff"], "would be formatted"),
        (&["-v", "--diff"], "would be formatted"),
        (&["--diff", "--check"], "failed formatting check"),
        (&["--dialect", "clickhouse", "--check"], "failed formatting check"),
    ];
    for (options, expected) in cases {
        let mut args = vec!["unf"];
        args.extend_from_slice(options);
        let out = sqlfmt(&dir.path, &args, None, &[]);
        let err = stderr(&out);
        assert_eq!(out.status.code(), Some(1), "{options:?}: {err}");
        assert!(err.contains(&format!("{count} files {expected}.")), "{options:?}: {err}");
    }
    let out = sqlfmt(&dir.path, &["--check", "-q", "unf"], None, &[]);
    assert_eq!(stderr(&out), "");
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn formatting_rewrites_files_and_then_passes() {
    let dir = TempDir::new("rewrite");
    let path = dir.write("model.sql", "SELECT A,B FROM T\n");
    let out = sqlfmt(&dir.path, &["model.sql"], None, &[]);
    assert_eq!(stderr(&out), "1 file formatted.\n0 files left unchanged.\nmodel.sql formatted.\n");
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "select a, b from t\n");
    let out = sqlfmt(&dir.path, &["--check", "model.sql"], None, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

#[test]
fn errors_exit_two() {
    let dir = TempDir::new("errors");
    dir.write("bad.sql", "select (1))\n");
    let out = sqlfmt(&dir.path, &["bad.sql"], None, &[]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr(&out),
        "1 file had errors while formatting.\n0 files left unchanged.\nbad.sql\n    sqlfmt encountered an error: \
         Closing bracket ')' found at 10 before bracket was opened.\n"
    );
}

#[test]
fn diff_prints_a_unified_diff() {
    let dir = TempDir::new("diff");
    dir.write("a.sql", "SELECT 1");
    let out = sqlfmt(&dir.path, &["--diff", "a.sql"], None, &[]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        "1 file would be formatted.\n0 files would be left unchanged.\na.sql would be formatted.\n\
         --- a.sql\t(Source Query)\n+++ a.sql\t(Formatted Query)\n@@ -1 +1 @@\n-SELECT 1\n\\ No newline at end of \
         file\n+select 1\n\n"
    );
}

#[test]
fn usage_errors_exit_two() {
    let dir = TempDir::new("usage");
    dir.write("a.sql", "select 1\n");
    let out = sqlfmt(&dir.path, &["--bogus", "a.sql"], None, &[]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr(&out),
        "Usage: sqlfmt [OPTIONS] [FILES]...\nTry 'sqlfmt --help' for help.\n\nError: No such option '--bogus'. Did \
         you mean '--verbose'?\n"
    );
    let out = sqlfmt(&dir.path, &["-l", "abc", "a.sql"], None, &[]);
    assert!(stderr(&out).ends_with("Error: Invalid value for '-l' / '--line-length': 'abc' is not a valid integer.\n"));
    let out = sqlfmt(&dir.path, &["missing.sql"], None, &[]);
    assert!(stderr(&out).ends_with("Error: Invalid value for '[FILES]...': Path 'missing.sql' does not exist.\n"));
    let out = sqlfmt(&dir.path, &["-l"], None, &[]);
    assert_eq!(stderr(&out), "Error: Option '-l' requires an argument.\n");
}

#[test]
fn options_come_from_the_environment() {
    let dir = TempDir::new("env");
    dir.write("a.sql", "select aaaaaaaaaa, bbbbbbbbbb from t\n");
    let out = sqlfmt(&dir.path, &["--check", "a.sql"], None, &[("SQLFMT_LINE_LENGTH", "20")]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let out = sqlfmt(&dir.path, &["a.sql"], None, &[("SQLFMT_CHECK", "maybe")]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("'maybe' is not a valid boolean."));
}

#[test]
fn config_file_options_and_overrides() {
    let dir = TempDir::new("config");
    dir.write("proj/pyproject.toml", "[tool.sqlfmt]\nline_length = 20\nexclude = ['skip/*']\n");
    dir.write("proj/a.sql", "select aaaaaaaaaa, bbbbbbbbbb from t\n");
    dir.write("proj/skip/b.sql", "SELECT 1\n");
    let out = sqlfmt(&dir.path, &["--check", "proj"], None, &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("1 file failed formatting check."), "{}", stderr(&out));
    let out = sqlfmt(&dir.path, &["--check", "-l", "88", "proj"], None, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    dir.write("other.toml", "[tool.sqlfmt]\nline_length = 88\n");
    let config = dir.path.join("other.toml");
    let out = sqlfmt(&dir.path, &["--check", "--config", config.to_str().expect("utf-8 path"), "proj"], None, &[]);
    assert!(
        stderr(&out).contains("1 file failed formatting check.\n1 file passed formatting check."),
        "{}",
        stderr(&out)
    );
    let out = sqlfmt(&dir.path, &["--config", "nope.toml", "proj"], None, &[]);
    assert!(stderr(&out).ends_with("Error: Invalid value for '--config': File 'nope.toml' does not exist.\n"));
}

#[test]
fn unchanged_files_are_cached() {
    let dir = TempDir::new("cache");
    dir.write("a.sql", "select 1\n");
    dir.write("b.sql", "SELECT 1\n");
    dir.write("c.sql", "SELECT 2\n");
    let out = sqlfmt(&dir.path, &["--check", "."], None, &[]);
    assert_eq!(out.status.code(), Some(1));
    // a.sql is now cached; b.sql and c.sql still need formatting, so every file is
    // formatted again and the cached one is also counted as a cache hit
    let out = sqlfmt(&dir.path, &["--check", "-v", "."], None, &[]);
    assert_eq!(
        stderr(&out),
        "2 files failed formatting check.\n2 files passed formatting check.\nb.sql failed formatting check.\nc.sql \
         failed formatting check.\na.sql passed formatting check.\na.sql passed formatting check.\n"
    );
    let out = sqlfmt(&dir.path, &["--check", "-v", "--single-process", "."], None, &[]);
    assert!(stderr(&out).contains("1 file passed formatting check."), "{}", stderr(&out));
    let out = sqlfmt(&dir.path, &["--check", "-v", "-k", "--single-process", "."], None, &[]);
    assert!(stderr(&out).contains("1 file passed formatting check."), "{}", stderr(&out));
}

#[test]
fn excluded_files_are_not_formatted() {
    let dir = TempDir::new("exclude");
    dir.write("a.sql", "SELECT 1\n");
    dir.write("sub/b.sql", "SELECT 1\n");
    dir.write(".hidden/c.sql", "SELECT 1\n");
    let out = sqlfmt(&dir.path, &["--check", "--exclude", "**/*.sql", "."], None, &[]);
    // hidden directories are not matched by `**`
    assert!(stderr(&out).contains("1 file failed formatting check."), "{}", stderr(&out));
    assert!(stderr(&out).contains(".hidden/c.sql failed formatting check."), "{}", stderr(&out));
    let out = sqlfmt(&dir.path, &["--check", "--exclude", "sub", "."], None, &[]);
    // a directory glob matches the directory, not the files in it
    assert!(stderr(&out).contains("3 files failed formatting check."), "{}", stderr(&out));
}
