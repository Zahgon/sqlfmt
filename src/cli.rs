//! The `sqlfmt` command line.
//!
//! Ported from sqlfmt/cli.py. Options are parsed and validated as sqlfmt's command
//! line always has: options may come before or after paths, short options can be
//! grouped (`-vq`) and take their value attached (`-l100`), long options take `=value`,
//! `--` ends the options, each option can also be set from its `SQLFMT_*` environment
//! variable, and usage errors print the usage line, a hint and the error, with exit
//! status 2.

use std::collections::HashMap;
use std::path::Path;

use toml::Value;

use crate::api;
use crate::config::{self, Config, ConfigValue};
use crate::difflib;
use crate::error::Fatal;
use crate::mode::Mode;
use crate::pathutil::{self, SourcePath};
use crate::pyint;
use crate::pystr;
use crate::report::{display_output, style_output, Color};
use crate::textwrap;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const HELP_TEXT: &str = "sqlfmt formats your dbt SQL files so you don't have to.

The FILES argument can be one or many paths to sql files (or directories),
or use \"-\" to use stdin.

Exit codes: 0 indicates success, 1 indicates failed check,
2 indicates a handled exception caused by errors in one or more user code files.

https://sqlfmt.com for documentation and more information.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A boolean flag (with an optional secondary flag that sets it false).
    Flag,
    /// A text value; `multiple` options collect every occurrence.
    Text {
        multiple: bool,
    },
    Integer,
    Choice,
    ConfigFile,
}

#[derive(Debug)]
struct OptionSpec {
    name: &'static str,
    opts: &'static [&'static str],
    secondary: &'static [&'static str],
    kind: Kind,
    envvar: Option<&'static str>,
    eager: bool,
    help: &'static str,
}

const OPTIONS: &[OptionSpec] = &[
    OptionSpec {
        name: "version",
        opts: &["--version"],
        secondary: &[],
        kind: Kind::Flag,
        envvar: None,
        eager: true,
        help: "Show the version and exit.",
    },
    OptionSpec {
        name: "check",
        opts: &["--check"],
        secondary: &[],
        kind: Kind::Flag,
        envvar: Some("SQLFMT_CHECK"),
        eager: false,
        help: "Fail with an exit code of 1 if source files are not formatted to spec. Do not write formatted queries to files.",
    },
    OptionSpec {
        name: "diff",
        opts: &["--diff"],
        secondary: &[],
        kind: Kind::Flag,
        envvar: Some("SQLFMT_DIFF"),
        eager: false,
        help: "Print a diff of any formatting changes to stdout. Fails like --check on any changes. Do not write formatted queries to files.",
    },
    OptionSpec {
        name: "exclude",
        opts: &["--exclude"],
        secondary: &[],
        kind: Kind::Text { multiple: true },
        envvar: Some("SQLFMT_EXCLUDE"),
        eager: false,
        help: "A string that is passed to glob.glob as a pathname; any matching files returned by glob will be excluded from FILES and not formatted. Note that glob is relative to the current working directory when sqlfmt is called. To exclude multiple globs, repeat the --exclude option.",
    },
    OptionSpec {
        name: "encoding",
        opts: &["--encoding"],
        secondary: &[],
        kind: Kind::Text { multiple: false },
        envvar: Some("SQLFMT_ENCODING"),
        eager: false,
        help: "The encoding to use when reading and writing .sql files. Defaults to utf-8. Set to 'inherit' to read the system default encoding. utf encodings will detect and preserve the BOM if one is present.",
    },
    OptionSpec {
        name: "fast",
        opts: &["--fast"],
        secondary: &["--safe"],
        kind: Kind::Flag,
        envvar: Some("SQLFMT_FAST"),
        eager: false,
        help: "By default, sqlfmt re-processes the output it produces in order to run a safety check and ensure that all tokens from the input are present in the output. This can add 15-20% to the processing time for new files. To disable this safety check, use the --fast option. To force the safety check, use --safe.",
    },
    OptionSpec {
        name: "single_process",
        opts: &["--single-process"],
        secondary: &[],
        kind: Kind::Flag,
        envvar: Some("SQLFMT_SINGLE_PROCESS"),
        eager: false,
        help: "Run sqlfmt in a single process, even when formatting multiple files. If not set, defaults to multiprocessing using as many cores as possible.",
    },
    OptionSpec {
        name: "reset_cache",
        opts: &["-k", "--reset-cache"],
        secondary: &[],
        kind: Kind::Flag,
        envvar: Some("SQLFMT_RESET_CACHE"),
        eager: false,
        help: "Clear the sqlfmt cache before running, effectively forcing sqlfmt to operate on every file. Will slow down runs.",
    },
    OptionSpec {
        name: "no_jinjafmt",
        opts: &["--no-jinjafmt"],
        secondary: &[],
        kind: Kind::Flag,
        envvar: Some("SQLFMT_NO_JINJAFMT"),
        eager: false,
        help: "Do not format jinja tags (the code between the curlies). Only necessary to specify this flag if sqlfmt was installed with the jinjafmt extra, or if black was already available in this environment.",
    },
    OptionSpec {
        name: "line_length",
        opts: &["-l", "--line-length"],
        secondary: &[],
        kind: Kind::Integer,
        envvar: Some("SQLFMT_LINE_LENGTH"),
        eager: false,
        help: "The maximum line length allowed in output files. Default is 88.",
    },
    OptionSpec {
        name: "verbose",
        opts: &["-v", "--verbose"],
        secondary: &[],
        kind: Kind::Flag,
        envvar: Some("SQLFMT_VERBOSE"),
        eager: false,
        help: "Prints more information to stderr.",
    },
    OptionSpec {
        name: "quiet",
        opts: &["-q", "--quiet"],
        secondary: &[],
        kind: Kind::Flag,
        envvar: Some("SQLFMT_QUIET"),
        eager: false,
        help: "Stop emitting all non-critical output. Error messages will still be emitted (which can silenced by 2>/dev/null).",
    },
    OptionSpec {
        name: "no_progressbar",
        opts: &["--no-progressbar"],
        secondary: &[],
        kind: Kind::Flag,
        envvar: Some("SQLFMT_NO_PROGRESSBAR"),
        eager: false,
        help: "Never prints a progressbar to stderr.",
    },
    OptionSpec {
        name: "no_color",
        opts: &["--no-color"],
        secondary: &[],
        kind: Kind::Flag,
        envvar: Some("SQLFMT_NO_COLOR"),
        eager: false,
        help: "Removes color codes from all output, including diffs. Alternatively, set the NO_COLOR environment variable. See https://no-color.org/ for more details.",
    },
    OptionSpec {
        name: "force_color",
        opts: &["--force-color"],
        secondary: &[],
        kind: Kind::Flag,
        envvar: Some("SQLFMT_FORCE_COLOR"),
        eager: false,
        help: "sqlfmt output is colorized by default. However, if you have the NO_COLOR env var set, and still want sqlfmt to colorize output, you can use --force-color to override the env var.",
    },
    OptionSpec {
        name: "dialect_name",
        opts: &["-d", "--dialect"],
        secondary: &[],
        kind: Kind::Choice,
        envvar: Some("SQLFMT_DIALECT"),
        eager: false,
        help: "The SQL dialect for the target files. Nearly all dialects are supported by the default polyglot dialect. Select the ClickHouse dialect to respect case sensitivity in function, field, and alias names.",
    },
    OptionSpec {
        name: "config_path",
        opts: &["--config"],
        secondary: &[],
        kind: Kind::ConfigFile,
        envvar: Some("SQLFMT_CONFIG"),
        eager: false,
        help: "A path to a `pyproject.toml` file. Options passed at the command line will override settings in this file.",
    },
    OptionSpec {
        name: "help",
        opts: &["--help"],
        secondary: &[],
        kind: Kind::Flag,
        envvar: None,
        eager: true,
        help: "Show this message and exit.",
    },
];

const DIALECTS: [&str; 2] = ["polyglot", "clickhouse"];

/// A raw option value collected by the parser.
#[derive(Debug, Clone)]
enum Raw {
    Flag(bool),
    Text(String),
    Many(Vec<String>),
}

/// A processed option value.
#[derive(Debug, Clone)]
enum Param {
    Bool(bool),
    Int(i64),
    Text(Option<String>),
    Many(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    CommandLine,
    Environment,
    Default,
}

/// How a run ends.
enum Exit {
    Code(i32),
    /// A usage error: printed with the usage line and a hint (when `with_usage`).
    Usage {
        message: String,
        with_usage: bool,
    },
}

/// The parameter being processed: an option (by index) or the FILES argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParamRef {
    Opt(usize),
    Files,
}

struct Parsed {
    opts: HashMap<usize, Raw>,
    files: Vec<String>,
    order: Vec<ParamRef>,
}

fn split_opt(opt: &str) -> (&str, &str) {
    let Some(first) = opt.chars().next() else { return ("", opt) };
    if first.is_alphanumeric() {
        return ("", opt);
    }
    let first_len = first.len_utf8();
    if opt[first_len..].starts_with(first) {
        (&opt[..2 * first_len], &opt[2 * first_len..])
    } else {
        (&opt[..first_len], &opt[first_len..])
    }
}

fn takes_value(spec: &OptionSpec) -> bool {
    spec.kind != Kind::Flag
}

fn find_long(opt: &str) -> Option<(usize, bool)> {
    OPTIONS.iter().enumerate().find_map(|(i, s)| {
        if s.opts.iter().any(|o| *o == opt && o.starts_with("--")) {
            Some((i, true))
        } else if s.secondary.contains(&opt) {
            Some((i, false))
        } else {
            None
        }
    })
}

fn find_short(opt: &str) -> Option<usize> {
    OPTIONS.iter().position(|s| s.opts.iter().any(|o| *o == opt && !o.starts_with("--")))
}

fn long_options() -> Vec<&'static str> {
    OPTIONS
        .iter()
        .flat_map(|s| s.opts.iter().chain(s.secondary.iter()).copied())
        .filter(|o| o.starts_with("--"))
        .collect()
}

fn usage_error(message: String) -> Exit {
    Exit::Usage { message, with_usage: true }
}

fn bad_option_usage(message: String) -> Exit {
    Exit::Usage { message, with_usage: false }
}

fn store(parsed: &mut Parsed, index: usize, value: Raw) {
    match (OPTIONS[index].kind, value) {
        (Kind::Text { multiple: true }, Raw::Text(v)) => {
            match parsed.opts.entry(index).or_insert(Raw::Many(Vec::new())) {
                Raw::Many(list) => list.push(v),
                other => *other = Raw::Many(vec![v]),
            }
        }
        (_, value) => {
            parsed.opts.insert(index, value);
        }
    }
    parsed.order.push(ParamRef::Opt(index));
}

fn parse(args: &[String]) -> Result<Parsed, Exit> {
    let mut parsed = Parsed { opts: HashMap::new(), files: Vec::new(), order: Vec::new() };
    let mut rargs: Vec<String> = args.to_vec();
    rargs.reverse();
    while let Some(arg) = rargs.pop() {
        if arg == "--" {
            break;
        }
        if arg.starts_with('-') && arg.chars().count() > 1 {
            process_opts(&arg, &mut rargs, &mut parsed)?;
        } else {
            parsed.files.push(arg);
        }
    }
    while let Some(arg) = rargs.pop() {
        parsed.files.push(arg);
    }
    parsed.order.push(ParamRef::Files);
    Ok(parsed)
}

fn process_opts(arg: &str, rargs: &mut Vec<String>, parsed: &mut Parsed) -> Result<(), Exit> {
    let (long_opt, explicit) = match arg.split_once('=') {
        Some((o, v)) => (o, Some(v.to_string())),
        None => (arg, None),
    };
    match find_long(long_opt) {
        Some((index, primary)) => {
            let spec = &OPTIONS[index];
            if takes_value(spec) {
                if let Some(v) = explicit {
                    rargs.push(v);
                }
                let value = rargs
                    .pop()
                    .ok_or_else(|| bad_option_usage(format!("Option '{long_opt}' requires an argument.")))?;
                store(parsed, index, Raw::Text(value));
            } else if explicit.is_some() {
                return Err(bad_option_usage(format!("Option '{long_opt}' does not take a value.")));
            } else {
                store(parsed, index, Raw::Flag(primary));
            }
            Ok(())
        }
        None => {
            if !arg.starts_with("--") {
                return match_short_opt(arg, rargs, parsed);
            }
            let longs = long_options();
            let suggestions = difflib::get_close_matches(long_opt, &longs, 3, 0.6);
            Err(usage_error(no_such_option(long_opt, suggestions)))
        }
    }
}

fn no_such_option(opt: &str, mut suggestions: Vec<String>) -> String {
    let message = format!("No such option {}.", pystr::repr_str(opt));
    if suggestions.is_empty() {
        return message;
    }
    suggestions.sort();
    let joined = suggestions.iter().map(|s| pystr::repr_str(s)).collect::<Vec<_>>().join(", ");
    if suggestions.len() == 1 {
        format!("{message} Did you mean {joined}?")
    } else {
        format!("{message} (Did you mean one of: {joined}?)")
    }
}

fn match_short_opt(arg: &str, rargs: &mut Vec<String>, parsed: &mut Parsed) -> Result<(), Exit> {
    let chars: Vec<char> = arg.chars().collect();
    let prefix = chars[0];
    let mut i = 1;
    for &ch in &chars[1..] {
        let opt = format!("{prefix}{ch}");
        i += 1;
        let Some(index) = find_short(&opt) else {
            return Err(usage_error(no_such_option(&opt, Vec::new())));
        };
        if takes_value(&OPTIONS[index]) {
            if i < chars.len() {
                rargs.push(chars[i..].iter().collect());
            }
            let value = rargs.pop().ok_or_else(|| bad_option_usage(format!("Option '{opt}' requires an argument.")))?;
            store(parsed, index, Raw::Text(value));
            if i < chars.len() {
                break;
            }
        } else {
            store(parsed, index, Raw::Flag(true));
        }
    }
    Ok(())
}

fn error_hint(index: usize) -> String {
    OPTIONS[index].opts.iter().map(|o| format!("'{o}'")).collect::<Vec<_>>().join(" / ")
}

fn invalid_value(index: usize, message: String) -> Exit {
    usage_error(format!("Invalid value for {}: {message}", error_hint(index)))
}

fn str_to_bool(value: &str) -> Option<bool> {
    match pystr::strip(value).to_lowercase().as_str() {
        "1" | "yes" | "true" | "on" | "t" | "y" => Some(true),
        "0" | "no" | "false" | "off" | "f" | "n" | "" => Some(false),
        _ => None,
    }
}

/// Converts a raw value of an option.
fn convert(index: usize, raw: Raw) -> Result<Param, Exit> {
    let spec = &OPTIONS[index];
    match (spec.kind, raw) {
        (Kind::Flag, Raw::Flag(b)) => Ok(Param::Bool(b)),
        (Kind::Flag, Raw::Text(s)) => str_to_bool(&s).map(Param::Bool).ok_or_else(|| {
            invalid_value(
                index,
                format!(
                    "{} is not a valid boolean. Recognized values: , 0, 1, f, false, n, no, off, on, t, true, y, yes",
                    pystr::repr_str(&s)
                ),
            )
        }),
        (Kind::Integer, Raw::Text(s)) => pyint::parse(&s)
            .map(Param::Int)
            .ok_or_else(|| invalid_value(index, format!("{} is not a valid integer.", pystr::repr_str(&s)))),
        (Kind::Choice, Raw::Text(s)) => {
            let folded = pyint::casefold(&s);
            DIALECTS.iter().find(|d| **d == folded).map(|d| Param::Text(Some(d.to_string()))).ok_or_else(|| {
                invalid_value(index, format!("{} is not one of 'polyglot', 'clickhouse'.", pystr::repr_str(&s)))
            })
        }
        (Kind::ConfigFile, Raw::Text(s)) => {
            check_path(&s, false).map(|p| Param::Text(Some(p))).map_err(|m| invalid_value(index, m))
        }
        (Kind::Text { .. }, Raw::Text(s)) => Ok(Param::Text(Some(s))),
        (_, Raw::Many(list)) => Ok(Param::Many(list)),
        (_, Raw::Flag(b)) => Ok(Param::Bool(b)),
    }
}

/// Resolves and checks a path argument (`dir_ok` for FILES, false for --config).
fn check_path(value: &str, dir_ok: bool) -> Result<String, String> {
    let name = if dir_ok { "Path" } else { "File" };
    let shown = pystr::repr_str(value);
    let resolved = pathutil::realpath(value);
    let meta = std::fs::metadata(&resolved).map_err(|_| format!("{name} {shown} does not exist."))?;
    if !dir_ok && meta.is_dir() {
        return Err(format!("{name} {shown} is a directory."));
    }
    if !readable(&resolved) {
        return Err(format!("{name} {shown} is not readable."));
    }
    Ok(resolved)
}

fn readable(path: &str) -> bool {
    let p = Path::new(path);
    if p.is_dir() {
        std::fs::read_dir(p).is_ok()
    } else {
        std::fs::File::open(p).is_ok()
    }
}

fn env_value(index: usize) -> Option<Raw> {
    let var = OPTIONS[index].envvar?;
    let value = std::env::var(var).ok().filter(|v| !v.is_empty())?;
    Some(match OPTIONS[index].kind {
        Kind::Text { multiple: true } => {
            Raw::Many(pystr::split_whitespace(&value).into_iter().map(str::to_string).collect())
        }
        _ => Raw::Text(value),
    })
}

fn default_value(index: usize) -> Param {
    match OPTIONS[index].kind {
        Kind::Flag => Param::Bool(false),
        Kind::Integer => Param::Int(88),
        Kind::Choice => Param::Text(Some("polyglot".to_string())),
        Kind::Text { multiple: true } => Param::Many(Vec::new()),
        Kind::Text { multiple: false } | Kind::ConfigFile => Param::Text(None),
    }
}

/// The help screen.
fn get_help(prog: &str) -> String {
    let width = help_width();
    let mut out = format!("Usage: {prog} [OPTIONS] [FILES]...\n");
    out.push('\n');
    out.push_str(&textwrap::wrap_text(HELP_TEXT, width, "  ", "  ", true));
    out.push('\n');
    out.push('\n');
    out.push_str("Options:\n");
    let rows: Vec<(String, &str)> = OPTIONS
        .iter()
        .map(|spec| {
            let mut opts: Vec<&str> = spec.opts.to_vec();
            opts.sort_by_key(|o| split_opt(o).0.len());
            let metavar = match spec.kind {
                Kind::Flag => "",
                Kind::Text { .. } => " TEXT",
                Kind::Integer => " INTEGER",
                Kind::Choice => " [polyglot|clickhouse]",
                Kind::ConfigFile => " FILE",
            };
            let mut first = format!("{}{metavar}", opts.join(", "));
            if !spec.secondary.is_empty() {
                first = format!("{first} / {}", spec.secondary.join(", "));
            }
            (first, spec.help)
        })
        .collect();
    let indent = 2;
    let first_col = rows.iter().map(|(f, _)| f.chars().count()).max().unwrap_or(0).min(30) + 2;
    for (first, second) in &rows {
        out.push_str(&" ".repeat(indent));
        out.push_str(first);
        if first.chars().count() <= first_col - 2 {
            out.push_str(&" ".repeat(first_col - first.chars().count()));
        } else {
            out.push('\n');
            out.push_str(&" ".repeat(first_col + indent));
        }
        let text_width = width.saturating_sub(first_col + 2).max(10);
        let wrapped = textwrap::wrap_text(second, text_width, "", "", true);
        let mut lines = wrapped.lines();
        if let Some(line) = lines.next() {
            out.push_str(line);
            out.push('\n');
        }
        for line in lines {
            out.push_str(&" ".repeat(first_col + indent));
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim_end_matches('\n').to_string()
}

/// The width of the help text: the terminal width (or `COLUMNS`), at most 80 and at
/// least 52 columns, less two.
fn help_width() -> usize {
    let columns = std::env::var("COLUMNS").ok().and_then(|c| pyint::parse(&c)).filter(|c| *c > 0);
    let columns = columns.unwrap_or_else(terminal_columns);
    (columns.min(80) - 2).max(50) as usize
}

fn terminal_columns() -> i64 {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        return 80;
    }
    let output = std::process::Command::new("stty").arg("size").stdin(std::process::Stdio::inherit()).output();
    output
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.split_whitespace().nth(1).and_then(|c| c.parse::<i64>().ok()))
        .filter(|c| *c > 0)
        .unwrap_or(80)
}

fn program_name() -> String {
    std::env::args_os()
        .next()
        .map(|a| Path::new(&a).file_name().map_or_else(|| "sqlfmt".to_string(), |n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "sqlfmt".to_string())
}

/// Runs the command line and returns the exit status.
pub fn main(args: Vec<String>) -> i32 {
    let prog = program_name();
    match run_cli(&prog, &args) {
        Exit::Code(code) => code,
        Exit::Usage { message, with_usage } => {
            let mut text = String::new();
            if with_usage {
                text.push_str(&format!("Usage: {prog} [OPTIONS] [FILES]...\nTry '{prog} --help' for help.\n\n"));
            }
            text.push_str(&format!("Error: {message}"));
            match display_output(&text, true, true) {
                Ok(()) => 2,
                Err(_) => 1,
            }
        }
    }
}

fn run_cli(prog: &str, args: &[String]) -> Exit {
    let parsed = match parse(args) {
        Ok(p) => p,
        Err(e) => return e,
    };
    // eager options first, then the others in the order they were given, then the
    // rest in declaration order
    let mut order: Vec<ParamRef> = Vec::new();
    for spec_index in (0..OPTIONS.len()).filter(|&i| OPTIONS[i].eager) {
        order.push(ParamRef::Opt(spec_index));
    }
    let position = |p: ParamRef| parsed.order.iter().position(|x| *x == p).unwrap_or(usize::MAX);
    order.sort_by_key(|p| position(*p));
    let mut rest: Vec<ParamRef> = (0..OPTIONS.len()).filter(|&i| !OPTIONS[i].eager).map(ParamRef::Opt).collect();
    rest.push(ParamRef::Files);
    rest.sort_by_key(|p| position(*p));
    order.extend(rest);

    let mut values: HashMap<usize, (Param, Source)> = HashMap::new();
    let mut files: Vec<SourcePath> = Vec::new();
    for p in order {
        match p {
            ParamRef::Files => {
                for f in &parsed.files {
                    if f == "-" {
                        files.push(SourcePath::Stdin);
                        continue;
                    }
                    match check_path(f, true) {
                        Ok(resolved) => files.push(SourcePath::File(resolved)),
                        Err(m) => return usage_error(format!("Invalid value for '[FILES]...': {m}")),
                    }
                }
            }
            ParamRef::Opt(i) => {
                let (raw, source) = match parsed.opts.get(&i) {
                    Some(raw) => (Some(raw.clone()), Source::CommandLine),
                    None => match env_value(i) {
                        Some(raw) => (Some(raw), Source::Environment),
                        None => (None, Source::Default),
                    },
                };
                let value = match raw {
                    Some(raw) => match convert(i, raw) {
                        Ok(v) => v,
                        Err(e) => return e,
                    },
                    None => default_value(i),
                };
                if OPTIONS[i].name == "version" && matches!(value, Param::Bool(true)) {
                    return Exit::Code(written(display_output(&format!("{prog}, version {VERSION}"), false, true), 0));
                }
                if OPTIONS[i].name == "help" && matches!(value, Param::Bool(true)) {
                    return Exit::Code(written(display_output(&get_help(prog), false, true), 0));
                }
                values.insert(i, (value, source));
            }
        }
    }

    if files.is_empty() {
        return Exit::Code(written(show_welcome_message(), 0));
    }
    match format_files(&files, &values) {
        Ok(code) => Exit::Code(code),
        // the run stops with status 1 whether or not the message can be written
        Err(fatal) => Exit::Code(written(display_output(&format!("sqlfmt: {fatal}"), true, true), 1)),
    }
}

fn format_files(files: &[SourcePath], values: &HashMap<usize, (Param, Source)>) -> Result<i32, Fatal> {
    let get = |name: &str| -> (&Param, Source) {
        let index = OPTIONS.iter().position(|s| s.name == name).expect("a known option");
        let (p, s) = values.get(&index).expect("every option is processed");
        (p, *s)
    };
    let config_path = match get("config_path").0 {
        Param::Text(Some(p)) => Some(p.clone()),
        _ => None,
    };
    let mut config: Config = config::load_config_file(files, config_path.as_deref())?;
    for spec in OPTIONS.iter().filter(|s| !matches!(s.name, "version" | "help" | "config_path")) {
        let (param, source) = get(spec.name);
        if source == Source::Default {
            continue;
        }
        let value = match param {
            Param::Bool(b) => Value::Boolean(*b),
            Param::Int(i) => Value::Integer(*i),
            Param::Text(Some(s)) => Value::String(s.clone()),
            Param::Text(None) => continue,
            Param::Many(list) => Value::Array(list.iter().cloned().map(Value::String).collect()),
        };
        config::set(&mut config, spec.name.to_string(), ConfigValue::Toml(value));
    }
    let mode = build_mode(&config)?;
    let matched = api::get_matching_paths(files, &mode)?;
    let report = api::run(&matched, &mode)?;
    report.display_report().map_err(|e| Fatal::new(format!("cannot write output: {e}")))?;
    Ok(if report.number_errored() > 0 {
        2
    } else if (mode.check || mode.diff) && report.number_changed() > 0 {
        1
    } else {
        0
    })
}

fn type_error(key: &str) -> Fatal {
    Fatal::new(format!("TypeError: unsupported value for {key}"))
}

/// Builds the Mode from the merged config file and command-line options.
fn build_mode(config: &Config) -> Result<Mode, Fatal> {
    let mut mode = Mode::default();
    let mut dialect = mode.dialect_name.clone();
    for (key, value) in config {
        match (key.as_str(), value) {
            ("exclude_root", ConfigValue::ExcludeRoot(root)) => mode.exclude_root = Some(root.into()),
            ("exclude_root", ConfigValue::Toml(_)) => return Err(type_error(key)),
            (_, ConfigValue::ExcludeRoot(_)) => return Err(type_error(key)),
            ("dialect_name", ConfigValue::Toml(Value::String(s))) => dialect = s.clone(),
            ("line_length", ConfigValue::Toml(Value::Integer(i))) => mode.line_length = *i,
            ("line_length", ConfigValue::Toml(Value::Boolean(b))) => mode.line_length = i64::from(*b),
            ("encoding", ConfigValue::Toml(Value::String(s))) => mode.encoding = s.clone(),
            ("exclude", ConfigValue::Toml(Value::Array(items))) => {
                mode.exclude = items
                    .iter()
                    .map(|v| match v {
                        Value::String(s) => Ok(s.clone()),
                        _ => Err(type_error(key)),
                    })
                    .collect::<Result<_, _>>()?;
            }
            ("exclude", ConfigValue::Toml(Value::String(s))) => mode.exclude = s.chars().map(String::from).collect(),
            (flag, ConfigValue::Toml(v)) if is_flag_field(flag) => set_flag(&mut mode, flag, config::truthy(v)),
            _ => return Err(type_error(key)),
        }
    }
    mode.set_dialect(&dialect).map_err(Fatal::from)?;
    Ok(mode)
}

fn is_flag_field(name: &str) -> bool {
    matches!(
        name,
        "check"
            | "diff"
            | "fast"
            | "single_process"
            | "no_jinjafmt"
            | "reset_cache"
            | "verbose"
            | "quiet"
            | "no_progressbar"
            | "no_color"
            | "force_color"
    )
}

fn set_flag(mode: &mut Mode, name: &str, value: bool) {
    let field = match name {
        "check" => &mut mode.check,
        "diff" => &mut mode.diff,
        "fast" => &mut mode.fast,
        "single_process" => &mut mode.single_process,
        "no_jinjafmt" => &mut mode.no_jinjafmt,
        "reset_cache" => &mut mode.reset_cache,
        "verbose" => &mut mode.verbose,
        "quiet" => &mut mode.quiet,
        "no_progressbar" => &mut mode.no_progressbar,
        "no_color" => &mut mode.no_color,
        _ => &mut mode.force_color,
    };
    *field = value;
}

/// The exit status after printing: `code`, or 1 when the output could not be written.
fn written(result: std::io::Result<()>, code: i32) -> i32 {
    if result.is_ok() {
        code
    } else {
        1
    }
}

/// The welcome message for `sqlfmt` without arguments.
fn show_welcome_message() -> std::io::Result<()> {
    let art = r"
               _  __           _
              | |/ _|         | |
     ___  __ _| | |_ _ __ ___ | |_
    / __|/ _` | |  _| '_ ` _ \| __|
    \__ \ (_| | | | | | | | | | |_
    |___/\__, |_|_| |_| |_| |_|\__|
            | |
            |_|";
    display_output(art, true, true)?;
    let message = "
    sqlfmt formats your dbt SQL files so you don't have to.
    For more information, visit https://sqlfmt.com

    To get started, try:
    ";
    display_output(message, true, true)?;
    let commands = [
        ("sqlfmt .", "format all files nested in the current dir (note the '.')"),
        ("sqlfmt path/to/file.sql", "format file.sql only"),
        ("sqlfmt . --check", "check formatting of all files, exit with code 1 on changes"),
        ("sqlfmt . --diff", "print diff resulting from formatting all files"),
        ("sqlfmt -", "format text received through stdin, write result to stdout"),
        ("sqlfmt --help", "show more options and other usage information"),
    ];
    let margin = commands.iter().map(|(c, _)| c.len()).max().unwrap_or(0);
    for (cmd, desc) in commands {
        let padded = format!("{cmd}{}", " ".repeat(margin - cmd.len()));
        let styled = style_output(&padded, Some(Color::White), Some(Color::BrightBlack), true);
        display_output(&format!("    {styled} {desc}"), true, true)?;
    }
    display_output("\n", true, true)
}
