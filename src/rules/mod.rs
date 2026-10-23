//! The lexing rules of every rule set.
//!
//! Ported from sqlfmt/rule.py and sqlfmt/rules/*.py. When the analyzer lexes a source
//! string it tries each rule of the active rule set in ascending order of priority
//! and applies the action of the first rule whose pattern matches at the current
//! position.

pub(crate) mod common;

use std::sync::OnceLock;

use fancy_regex::Regex;

use crate::pyre;
use crate::tokens::TokenType;
use common::*;

/// What the analyzer does when a rule matches (the `action` of a Python `Rule`).
#[derive(Debug, Clone)]
pub(crate) enum Action {
    /// Add a node of the given type.
    AddNode(TokenType),
    /// Add a node of the first type; on a bracket error, of the fallback type.
    SafeAddNode(TokenType, TokenType),
    /// Lex a qualified keyword (one that follows a dot) as a name, otherwise run the
    /// inner action.
    Reserved(Box<Action>),
    /// Lex the keyword as a name inside brackets, otherwise as a reserved keyword.
    NonreservedTopLevel(Box<Action>),
    /// Lex the rest of the source with another rule set.
    LexRuleset(RuleSet),
    RaiseBracketError,
    AddComment,
    AddJinjaComment,
    Newline,
    Semicolon,
    DdlAs,
    ClosingAngleBracket,
    SetOperator,
    Number,
    JinjaBlockStart,
    JinjaBlockKeyword,
    JinjaDataBlockStart {
        ruleset: Option<RuleSet>,
        raises: bool,
    },
    JinjaBlockEnd {
        reset_sql_depth: bool,
    },
    Jinja {
        start: &'static str,
        end: &'static str,
        ttype: TokenType,
    },
    PotentiallyNested {
        start: &'static str,
        end: &'static str,
        ttype: TokenType,
    },
}

/// The rule sets (Python's MAIN, JINJA, JINJA_DATA, FUNCTION, GRANT, CLONE, PRAGMA,
/// UNSUPPORTED and WAREHOUSE lists).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuleSet {
    Main,
    Jinja,
    JinjaData,
    Function,
    Grant,
    Clone,
    Pragma,
    Unsupported,
    Warehouse,
}

/// A lex rule: a named pattern with a priority (lower is tried first) and an action.
#[derive(Debug)]
pub(crate) struct Rule {
    pub(crate) name: &'static str,
    pub(crate) priority: u32,
    pub(crate) pattern: String,
    pub(crate) action: Action,
    /// The pattern, prefixed with optional non-newline whitespace, anchored at the
    /// position where it is tried.
    pub(crate) program: Regex,
}

impl Rule {
    fn new(name: &'static str, priority: u32, pattern: String, action: Action) -> Rule {
        let program = pyre::compile_anchored(&format!("{MAYBE_WHITESPACES}{pattern}"), "is");
        Rule { name, priority, pattern, action, program }
    }
}

fn add(t: TokenType) -> Action {
    Action::AddNode(t)
}

fn reserved(a: Action) -> Action {
    Action::Reserved(Box::new(a))
}

fn top_level(a: Action) -> Action {
    Action::NonreservedTopLevel(Box::new(a))
}

fn word_boundary(pattern: String) -> String {
    format!("{pattern}{}", group(&[r"\W", r"$"]))
}

fn always() -> Vec<Rule> {
    vec![
        Rule::new("fmt_off", 0, format!("{}{}", group(&[r"(--|#) ?fmt: ?off ?"]), eol()), add(TokenType::FmtOff)),
        Rule::new("fmt_on", 1, format!("{}{}", group(&[r"(--|#) ?fmt: ?on ?"]), eol()), add(TokenType::FmtOn)),
        // these match just the start of jinja tags, which lets the analyzer deal with
        // nesting in a more powerful way than a regex could
        Rule::new("jinja_start", 120, jinja_start(), Action::LexRuleset(RuleSet::Jinja)),
        // the end of a jinja tag never matches by itself
        Rule::new("jinja_end", 130, group(&[r"[#}%]\}"]), Action::RaiseBracketError),
        Rule::new("quoted_name", 200, sql_quoted_exp(), add(TokenType::QuotedName)),
        Rule::new("comment", 300, sql_comment(), Action::AddComment),
        Rule::new(
            "comment_start",
            310,
            group(&[r"/\*"]),
            Action::PotentiallyNested { start: "comment_start", end: "comment_end", ttype: TokenType::Comment },
        ),
        Rule::new("comment_end", 320, group(&[r"\*/"]), Action::RaiseBracketError),
        Rule::new("semicolon", 350, group(&[r";"]), Action::Semicolon),
        Rule::new("newline", 9000, group(&[NEWLINE]), Action::Newline),
    ]
}

fn core() -> Vec<Rule> {
    let mut rules = always();
    rules.extend([
        // see https://github.com/tconbeer/sqlfmt/issues/461
        Rule::new("pg_operator", 299, group(&[r"#(>>?|-|#)"]), add(TokenType::Operator)),
        // https://spark.apache.org/docs/latest/sql-ref-literals.html#integral-literal-syntax
        Rule::new("spark_int_literals", 400, group(&[r"(\+|-)?\d+(l|s|y)"]), Action::Number),
        // the (bd|d|f) groups add support for Spark fractional literals
        Rule::new(
            "number",
            401,
            group(&[
                r"0x[0-9a-f]+",
                r"0o[0-7]+",
                r"0b[0-1]+",
                r"(\+|-)?\d+(_\d+)*(\.\d*(_\d+)*)?(e(\+|-)?\d+)?(bd|d|f)?",
                r"(\+|-)?\.\d+(_\d+)*(e(\+|-)?\d+)?(bd|d|f)?",
            ]),
            Action::Number,
        ),
        Rule::new("star", 410, group(&[r"\*"]), add(TokenType::Star)),
        Rule::new("double_colon", 420, group(&[r"::"]), add(TokenType::DoubleColon)),
        Rule::new("walrus", 421, group(&[r":="]), add(TokenType::Operator)),
        Rule::new("colon", 430, group(&[r":"]), add(TokenType::Colon)),
        Rule::new("comma", 440, group(&[r","]), add(TokenType::Comma)),
        Rule::new("dot", 450, group(&[r"\."]), add(TokenType::Dot)),
        Rule::new(
            "bracket_open",
            500,
            group(&[
                r"\[",
                r"\(",
                r"\{",
                // bq/athena use angle brackets for compound type definitions
                r"(array|map|table|struct)\s*<",
            ]),
            add(TokenType::BracketOpen),
        ),
        Rule::new("bracket_close", 510, group(&[r"\]", r"\)", r"\}"]), add(TokenType::BracketClose)),
        Rule::new(
            "other_identifiers",
            600,
            group(&[
                r"@\w+",             // stages
                r"metadata\$\w+",    // Snowflake metadata columns
                r"\$\d+",            // pg placeholders
                r"\$\w+",            // variables
                r"%(\([^%()]+\))?s", // psycopg placeholders
                r"\?\d+",            // bun placeholders
            ]),
            add(TokenType::Name),
        ),
        Rule::new("angle_bracket_close", 790, group(&[r">"]), Action::ClosingAngleBracket),
        Rule::new(
            "operator",
            800,
            group(&[
                r"\|\|?\/",           // square or cube root ||/
                r"~=",                // geo compare
                r"!?~{1,2}\*?",       // posix like/not like
                r"\?(=|!|<=|<!)",     // regex lookahead/behind
                r"\?(-\||\|\||-|\|)", // regex lookahead/behind
                r"@-@",               // length operator
                r"@@@?",              // center point operator; also text match
                r"<->",               // distance operator
                r"<=>",               // null-safe equals (mysql)
                r"@>",                // contains
                r"<@",                // contained by
                r"<>",
                r"\|?>>=?",
                r"<<(=|\|)?",
                r"=>",
                r"->>?",     // json extraction
                r"\?(&|\|)", // pg json contains keys
                r"&&",
                r"&<\|?", // not extends
                r"\|?&>", // not extends
                r"<\^",   // below
                r">\^",   // above
                r"\?#",   // intersect
                r"\|\|",
                r"-\|-",
                r"[*+?]?\?",          // regex greedy/non-greedy, also ?
                r"!!",                // negate text match
                r"%%",                // psycopg escaped mod operator
                r"[+\-*/%&|^=<>!]=?", // singles
            ]),
            add(TokenType::Operator),
        ),
        Rule::new("name", 5000, group(&[r"\w+"]), add(TokenType::Name)),
    ]);
    rules
}

fn main_rules() -> Vec<Rule> {
    let mut rules = core();
    rules.extend([
        Rule::new(
            "star_columns",
            409,
            format!("{}{}", group(&[r"\*columns"]), group(&[r"\(", r"$"])),
            add(TokenType::Name),
        ),
        Rule::new("statement_start", 1000, word_boundary(group(&[r"case"])), reserved(add(TokenType::StatementStart))),
        Rule::new(
            "statement_end",
            1010,
            word_boundary(group(&[r"end"])),
            reserved(Action::SafeAddNode(TokenType::StatementEnd, TokenType::Name)),
        ),
        // a join's using word operator must be followed by parens; otherwise, it's
        // probably a delete's USING, which is an unterminated keyword
        Rule::new(
            "join_using",
            1049,
            format!("{}{}", group(&[r"using"]), group(&[r"\s*\("])),
            reserved(add(TokenType::WordOperator)),
        ),
        Rule::new(
            "unterm_keyword",
            1050,
            word_boundary(group(&[
                r"with(\s+recursive)?",
                concat!(
                    r"select(\s+(as\s+struct|as\s+value))?",
                    r"(\s+(all|top\s+\d+|distinct))?",
                    // select into is ddl that needs additional handling
                    r"(?!\s+into)"
                ),
                r"delete\s+from",
                r"from",
                concat!(
                    r"((global|natural|asof|any)\s+)?",
                    r"((inner|left|right|full|cross)\s+)?",
                    r"((outer|semi|anti|any|all|asof|cross|positional|array|paste)\s+)?join"
                ),
                // the USING following DELETE, not the join operator (see above)
                r"using",
                r"lateral\s+view(\s+outer)?",
                r"(pre)?where",
                r"group\s+by",
                r"cluster\s+by",
                r"distribute\s+by",
                r"sort\s+by",
                r"having",
                r"qualify",
                r"window",
                r"order\s+by",
                r"limit",
                r"fetch\s+(first|next)",
                r"for\s+(update|no\s+key\s+update|share|key\s+share)",
                r"when",
                r"then",
                r"else",
                r"partition\s+by",
                r"values",
                // in pg, RETURNING can be the last clause of a DELETE statement
                r"returning",
            ])),
            reserved(add(TokenType::UntermKeyword)),
        ),
        // some function names in some dialects are word operators in other dialects;
        // they are function names iff immediately followed by `(`
        Rule::new(
            "functions_that_overlap_with_word_operators",
            1099,
            format!("{}{}", group(&[r"filter", r"isnull", r"(r|i)?like"]), group(&[r"\("])),
            reserved(add(TokenType::Name)),
        ),
        Rule::new(
            "word_operator",
            1100,
            word_boundary(group(&[
                r"as",
                r"(not\s+)?between",
                r"cube",
                r"(not\s+)?exists",
                r"filter",
                r"grouping sets",
                r"(global)?(not\s+)?in",
                r"interval",
                r"is(\s+not)?(\s+distinct\s+from)?",
                r"isnull",
                r"(not\s+)?i?like(\s+(any|all))?",
                r"over",
                r"(un)?pivot",
                r"notnull",
                r"(not\s+)?regexp",
                r"(not\s+)?rlike",
                r"rollup",
                r"some",
                r"(not\s+)?similar\s+to",
                r"tablesample",
                r"within\s+group",
            ])),
            reserved(add(TokenType::WordOperator)),
        ),
        Rule::new(
            "star_replace_exclude",
            1101,
            format!("{}{}", group(&[r"exclude", r"replace"]), group(&[r"\s+\("])),
            reserved(add(TokenType::WordOperator)),
        ),
        Rule::new("on", 1120, word_boundary(group(&[r"on"])), reserved(add(TokenType::On))),
        Rule::new(
            "boolean_operator",
            1200,
            word_boundary(group(&[r"and", r"or", r"not"])),
            reserved(add(TokenType::BooleanOperator)),
        ),
        Rule::new(
            "frame_clause",
            1305,
            word_boundary(format!(
                "{}{}",
                group(&[r"(range|rows|groups)\s+"]),
                group(&[r"(between\s+)?((unbounded|\d+)\s+(preceding|following)|current\s+row)"])
            )),
            reserved(add(TokenType::UntermKeyword)),
        ),
        // BQ arrays use an offset(n) function for indexing that must not match; this
        // is the offset in limit ... offset, which must be followed by a space
        Rule::new(
            "offset_keyword",
            1310,
            format!("{}{}", group(&[r"offset"]), group(&[r"\s+", r"$"])),
            reserved(add(TokenType::UntermKeyword)),
        ),
        Rule::new(
            "set_operator",
            1320,
            word_boundary(group(&[concat!(
                r"((inner|((full|left)(\s+outer)?))\s+)?",
                r"(union|intersect|except|minus)",
                r"(\s+(all|distinct))?",
                r"(\s+(by\s+name|((strict\s+)?corresponding(\s+by)?)))?"
            )])),
            reserved(Action::SetOperator),
        ),
        Rule::new(
            "explain",
            2000,
            word_boundary(group(&[r"explain(\s+(analyze|verbose|using\s+(tabular|json|text)))?"])),
            top_level(add(TokenType::UntermKeyword)),
        ),
        Rule::new(
            "pragma",
            2005,
            word_boundary(group(&[&pragma_set_call()])),
            top_level(Action::LexRuleset(RuleSet::Pragma)),
        ),
        Rule::new(
            "grant",
            2010,
            word_boundary(group(&[r"grant", r"revoke"])),
            top_level(Action::LexRuleset(RuleSet::Grant)),
        ),
        Rule::new(
            "create_clone",
            2015,
            word_boundary(group(&[&format!(r"{}\s+.+?\s+clone", create_clonable())])),
            top_level(Action::LexRuleset(RuleSet::Clone)),
        ),
        Rule::new(
            "create_function",
            2020,
            word_boundary(group(&[CREATE_FUNCTION, ALTER_DROP_FUNCTION])),
            top_level(Action::LexRuleset(RuleSet::Function)),
        ),
        Rule::new(
            "create_warehouse",
            2030,
            word_boundary(group(&[CREATE_WAREHOUSE, ALTER_WAREHOUSE])),
            top_level(Action::LexRuleset(RuleSet::Warehouse)),
        ),
        Rule::new(
            "unsupported_ddl",
            2999,
            format!(
                "{}{}{}",
                group(&[
                    r"alter",
                    r"attach\s+rls\s+policy",
                    r"cache\s+table",
                    r"clear\s+cache",
                    r"cluster",
                    r"comment",
                    r"copy",
                    r"create",
                    r"deallocate",
                    r"declare",
                    r"describe",
                    r"desc\s+datashare",
                    r"desc\s+identity\s+provider",
                    r"delete",
                    r"detach\s+rls\s+policy",
                    r"discard",
                    r"do",
                    r"drop",
                    r"execute",
                    r"export",
                    r"fetch",
                    r"get",
                    r"handler",
                    r"import\s+foreign\s+schema",
                    r"import\s+table",
                    // snowflake: "insert into" or "insert overwrite into"; spark:
                    // "insert overwrite" without "into"; bigquery: bare "insert"
                    r"insert(\s+overwrite)?(\s+into)?",
                    r"list",
                    r"lock",
                    r"merge",
                    r"move",
                    // prepare transaction statements are simple enough to allow
                    r"prepare(?!\s+transaction)",
                    r"put",
                    r"reassign\s+owned",
                    r"remove",
                    r"rename\s+table",
                    r"repair",
                    r"security\s+label",
                    r"select\s+into",
                    r"truncate",
                    r"unload",
                    r"update",
                    r"validate",
                ]),
                r"(?!\()",
                group(&[r"\W", r"$"])
            ),
            top_level(Action::LexRuleset(RuleSet::Unsupported)),
        ),
    ]);
    rules
}

const JINJA_SET: &str = r"\{%-?\s*set\s+[^=]+?-?%\}";
const JINJA_ENDSET: &str = r"\{%-?\s*endset\s*-?%\}";
const JINJA_CALL: &str = r"\{%-?\s*call(\(.*?\))?\s+\w+.*?-?%\}";
const JINJA_ENDCALL: &str = r"\{%-?\s*endcall\s*-?%\}";

fn jinja_data_rules() -> Vec<Rule> {
    vec![
        Rule::new(
            "jinja_set_block_start",
            100,
            group(&[JINJA_SET]),
            Action::JinjaDataBlockStart { ruleset: None, raises: false },
        ),
        Rule::new("jinja_set_block_end", 101, group(&[JINJA_ENDSET]), Action::JinjaBlockEnd { reset_sql_depth: true }),
        Rule::new(
            "jinja_call_block_start",
            261,
            group(&[JINJA_CALL]),
            Action::JinjaDataBlockStart { ruleset: None, raises: false },
        ),
        Rule::new(
            "jinja_call_block_end",
            265,
            group(&[JINJA_ENDCALL]),
            Action::JinjaBlockEnd { reset_sql_depth: true },
        ),
        Rule::new("jinja_newline", 500, group(&[NEWLINE]), Action::Newline),
        Rule::new(
            "jinja_data",
            600,
            format!(r"{}\s*{}", group(&[r".*?"]), group(&[JINJA_SET, JINJA_ENDSET, JINJA_CALL, JINJA_ENDCALL, r"$"])),
            add(TokenType::Data),
        ),
    ]
}

fn jinja_rules() -> Vec<Rule> {
    let block_start = || Action::JinjaBlockStart;
    let block_end = |reset| Action::JinjaBlockEnd { reset_sql_depth: reset };
    vec![
        Rule::new("jinja_comment", 0, group(&[r"\{\#.*?\#\}"]), Action::AddJinjaComment),
        Rule::new(
            "jinja_set_block_start",
            100,
            group(&[JINJA_SET]),
            Action::JinjaDataBlockStart { ruleset: Some(RuleSet::JinjaData), raises: true },
        ),
        Rule::new("jinja_set_block_end", 101, group(&[JINJA_ENDSET]), Action::RaiseBracketError),
        Rule::new("jinja_if_block_start", 200, group(&[r"\{%-?\s*if.*?-?%\}"]), block_start()),
        Rule::new("jinja_elif_block_start", 201, group(&[r"\{%-?\s*elif\s+\w+.*?-?%\}"]), Action::JinjaBlockKeyword),
        Rule::new("jinja_else_block_start", 202, group(&[r"\{%-?\s*else\s*-?%\}"]), Action::JinjaBlockKeyword),
        Rule::new("jinja_if_block_end", 203, group(&[r"\{%-?\s*endif\s*-?%\}"]), block_end(false)),
        Rule::new("jinja_for_block_start", 210, group(&[r"\{%-?\s*for\s+.*?-?%\}"]), block_start()),
        Rule::new("jinja_for_block_end", 211, group(&[r"\{%-?\s*endfor\s*-?%\}"]), block_end(false)),
        Rule::new("jinja_macro_block_start", 220, group(&[r"\{%-?\s*macro\s+\w+.*?-?%\}"]), block_start()),
        Rule::new("jinja_macro_block_end", 221, group(&[r"\{%-?\s*endmacro\s*-?%\}"]), block_end(true)),
        Rule::new("jinja_test_block_start", 230, group(&[r"\{%-?\s*test\s+\w+.*?-?%\}"]), block_start()),
        Rule::new("jinja_test_block_end", 231, group(&[r"\{%-?\s*endtest\s*-?%\}"]), block_end(true)),
        Rule::new("jinja_snapshot_block_start", 240, group(&[r"\{%-?\s*snapshot\s+\w+.*?-?%\}"]), block_start()),
        Rule::new("jinja_snapshot_block_end", 241, group(&[r"\{%-?\s*endsnapshot\s*-?%\}"]), block_end(true)),
        Rule::new(
            "jinja_materialization_block_start",
            250,
            group(&[r"\{%-?\s*materialization\s+\w+\s*,.*?-?%\}"]),
            block_start(),
        ),
        Rule::new(
            "jinja_materialization_block_end",
            251,
            group(&[r"\{%-?\s*endmaterialization\s*-?%\}"]),
            block_end(true),
        ),
        // call blocks that call dbt's statement macro are guaranteed to contain SQL, so
        // they are parsed like ordinary jinja blocks and their contents formatted
        Rule::new(
            "jinja_call_statement_block_start",
            260,
            group(&[r"\{%-?\s*call\s+(noop_)?statement\(.*?\)\s*-?%\}"]),
            block_start(),
        ),
        // call blocks that call other macros may contain arbitrary text or data, so the
        // whole block is lexed as DATA and not formatted
        Rule::new(
            "jinja_call_block_start",
            261,
            group(&[JINJA_CALL]),
            Action::JinjaDataBlockStart { ruleset: Some(RuleSet::JinjaData), raises: true },
        ),
        Rule::new("jinja_call_block_end", 265, group(&[JINJA_ENDCALL]), block_end(true)),
        Rule::new(
            "jinja_statement_start",
            500,
            group(&[r"\{%-?"]),
            Action::Jinja {
                start: "jinja_statement_start",
                end: "jinja_statement_end",
                ttype: TokenType::JinjaStatement,
            },
        ),
        Rule::new(
            "jinja_expression_start",
            510,
            group(&[r"\{\{-?"]),
            Action::Jinja {
                start: "jinja_expression_start",
                end: "jinja_expression_end",
                ttype: TokenType::JinjaExpression,
            },
        ),
        Rule::new("jinja_statement_end", 600, group(&[r"-?%\}"]), Action::RaiseBracketError),
        Rule::new("jinja_expression_end", 610, group(&[r"-?\}\}"]), Action::RaiseBracketError),
    ]
}

fn function_rules() -> Vec<Rule> {
    let mut rules = core();
    rules.extend([
        Rule::new("function_as", 1100, word_boundary(group(&[r"as"])), reserved(Action::DdlAs)),
        Rule::new(
            "word_operator",
            1200,
            word_boundary(group(&[r"to", r"from", r"runtime_version"])),
            reserved(add(TokenType::WordOperator)),
        ),
        Rule::new(
            "unterm_keyword",
            1300,
            word_boundary(group(&[
                CREATE_FUNCTION,
                ALTER_DROP_FUNCTION,
                r"language",
                r"transform",
                r"immutable",
                r"stable",
                r"volatile",
                r"(not\s+)?leakproof",
                r"volatile",
                r"called\s+on\s+null\s+input",
                r"returns\s+null\s+on\s+null\s+input",
                r"return(s)?(?!\s+null)",
                r"strict",
                r"(external\s+)?security\s+(invoker|definer)",
                r"parallel\s+(unsafe|restricted|safe)",
                r"cost",
                r"rows",
                r"support",
                // snowflake
                r"((un)?set\s+)?comment",
                r"imports",
                r"packages",
                r"handler",
                r"target_path",
                r"(not\s+)?null",
                // snowflake external functions
                &format!(
                    r"((un)?set\s+)?{}",
                    group(&[
                        r"api_integration",
                        r"headers",
                        r"context_headers",
                        r"max_batch_rows",
                        r"compression",
                        r"request_translator",
                        r"response_translator",
                    ])
                ),
                // bq
                r"options",
                r"remote\s+with\s+connection",
                // ALTER
                r"rename\s+to",
                r"owner\s+to",
                r"set\s+schema",
                r"(no\s+)?depends\s+on\s+extension",
                r"cascade",
                r"restrict",
                // alter snowflake
                r"(un)?set\s+secure",
                // pg catchall for set
                r"(re)?set(\s+all)?",
            ])),
            reserved(add(TokenType::UntermKeyword)),
        ),
    ]);
    rules
}

fn grant_rules() -> Vec<Rule> {
    let mut rules = core();
    rules.push(Rule::new(
        "unterm_keyword",
        1300,
        word_boundary(group(&[
            r"grant",
            r"revoke(\s+grant\s+option\s+for)?",
            r"on",
            r"to",
            r"from",
            r"with\s+grant\s+option",
            r"granted\s+by",
            r"cascade",
            r"restrict",
        ])),
        reserved(add(TokenType::UntermKeyword)),
    ));
    rules
}

fn clone_rules() -> Vec<Rule> {
    let mut rules = core();
    rules.extend([
        Rule::new(
            "unterm_keyword",
            1300,
            word_boundary(group(&[&create_clonable(), r"clone"])),
            reserved(add(TokenType::UntermKeyword)),
        ),
        Rule::new(
            "word_operator",
            1500,
            word_boundary(group(&[r"at", r"before"])),
            reserved(add(TokenType::WordOperator)),
        ),
    ]);
    rules
}

fn pragma_rules() -> Vec<Rule> {
    let mut rules = core();
    rules.push(Rule::new(
        "unterm_keyword",
        1300,
        word_boundary(group(&[&pragma_set_call()])),
        reserved(add(TokenType::UntermKeyword)),
    ));
    rules
}

fn unsupported_rules() -> Vec<Rule> {
    let mut rules = always();
    rules.extend([
        // quoted names are lexed as DATA so they are not formatted; this rule must
        // come before the quoted_name rule in ALWAYS
        Rule::new("quoted_name_in_unsupported", 199, sql_quoted_exp(), add(TokenType::Data)),
        // a semicolon inside a string literal or a comment does not end the line, so
        // quoted expressions are consumed whole and lexing stops at a comment
        Rule::new(
            "unsupported_line",
            1000,
            format!(
                "{}{}{}",
                group(&[&format!(r"(?:{}|[^;\n])+?", sql_quoted_exp())]),
                group(&[r"\s*"]),
                group(&[r";", NEWLINE, SQL_COMMENT_START, &jinja_start(), r"$"])
            ),
            reserved(add(TokenType::Data)),
        ),
    ]);
    rules
}

fn warehouse_rules() -> Vec<Rule> {
    let mut rules = core();
    rules.push(Rule::new(
        "unterm_keyword",
        1300,
        word_boundary(group(&[
            CREATE_WAREHOUSE,
            ALTER_WAREHOUSE,
            // objectProperties
            &format!(
                r"(with\s+|(un)?set\s+)?{}",
                group(&[
                    r"warehouse_type",
                    r"warehouse_size",
                    r"max_cluster_count",
                    r"min_cluster_count",
                    r"scaling_policy",
                    r"auto_suspend",
                    r"auto_resume",
                    r"initially_suspended",
                    r"resource_monitor",
                    r"comment",
                    r"enable_query_acceleration",
                    r"query_acceleration_max_scale_factor",
                    r"tag",
                ])
            ),
            // objectParams
            &format!(
                r"(set\s+)?{}",
                group(&[
                    r"max_concurrency_level",
                    r"statement_queued_timeout_in_seconds",
                    r"statement_timeout_in_seconds",
                ])
            ),
            // alter
            r"suspend",
            r"resume(\s+if\s+suspended)?",
            r"abort\s+all\s+queries",
            r"rename\s+to",
        ])),
        reserved(add(TokenType::UntermKeyword)),
    ));
    rules
}

fn sorted(mut rules: Vec<Rule>) -> Vec<Rule> {
    rules.sort_by_key(|r| r.priority);
    rules
}

/// The rules of a rule set, sorted by priority.
pub(crate) fn rules(set: RuleSet) -> &'static [Rule] {
    macro_rules! cached {
        ($build:expr) => {{
            static CELL: OnceLock<Vec<Rule>> = OnceLock::new();
            CELL.get_or_init(|| sorted($build)).as_slice()
        }};
    }
    match set {
        RuleSet::Main => cached!(main_rules()),
        RuleSet::Jinja => cached!(jinja_rules()),
        RuleSet::JinjaData => cached!(jinja_data_rules()),
        RuleSet::Function => cached!(function_rules()),
        RuleSet::Grant => cached!(grant_rules()),
        RuleSet::Clone => cached!(clone_rules()),
        RuleSet::Pragma => cached!(pragma_rules()),
        RuleSet::Unsupported => cached!(unsupported_rules()),
        RuleSet::Warehouse => cached!(warehouse_rules()),
    }
}

/// Finds the first rule named `name` in a list of rules.
pub(crate) fn get_rule<'a>(rules: &'a [Rule], name: &str) -> Option<&'a Rule> {
    rules.iter().find(|r| r.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rule_set_compiles_sorted() {
        for set in [
            RuleSet::Main,
            RuleSet::Jinja,
            RuleSet::JinjaData,
            RuleSet::Function,
            RuleSet::Grant,
            RuleSet::Clone,
            RuleSet::Pragma,
            RuleSet::Unsupported,
            RuleSet::Warehouse,
        ] {
            let rules = rules(set);
            assert!(!rules.is_empty(), "{set:?} has no rules");
            assert!(rules.windows(2).all(|w| w[0].priority <= w[1].priority), "{set:?} is not sorted");
        }
    }

    #[test]
    fn main_rules_match_keywords() {
        let main = rules(RuleSet::Main);
        let unterm = get_rule(main, "unterm_keyword").unwrap();
        assert!(unterm.program.is_match("  select\n").unwrap());
        assert!(!unterm.program.is_match("selected").unwrap());
        let name = get_rule(main, "name").unwrap();
        assert!(name.program.is_match("foo_bar").unwrap());
    }
}
