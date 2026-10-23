//! The Analyzer lexes a source string into lines of nodes and comments.
//!
//! Ported from sqlfmt/analyzer.py and sqlfmt/actions.py.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use fancy_regex::{Captures, Regex};

use crate::comment::Comment;
use crate::error::{ErrorKind, Fatal, SqlfmtError};
use crate::jinja_lexer;
use crate::line::Line;
use crate::node::{Arena, NodeId};
use crate::node_manager::NodeManager;
use crate::pyre;
use crate::pystr;
use crate::query::Query;
use crate::rules::common::{group, sql_comment, sql_quoted_exp, MAYBE_WHITESPACES};
use crate::rules::{self, get_rule, Action, Rule, RuleSet};
use crate::tokens::{Token, TokenType};

/// How lexing stops early: a nested rule set is finished (`Stop`), the source has an
/// error the user is told about (`User`), or lexing cannot continue at all (`Fatal`).
#[derive(Debug)]
pub(crate) enum Flow {
    Stop,
    User(SqlfmtError),
    Fatal(Fatal),
}

impl From<SqlfmtError> for Flow {
    fn from(e: SqlfmtError) -> Flow {
        Flow::User(e)
    }
}

impl From<Fatal> for Flow {
    fn from(e: Fatal) -> Flow {
        Flow::Fatal(e)
    }
}

type LexResult = Result<(), Flow>;

/// The analyzer keeps buffers of lexed nodes, comments and lines, the active rule set
/// and the stack of rule sets it returns to. The active rule set persists between
/// calls to [`Analyzer::parse_query`].
pub(crate) struct Analyzer {
    pub(crate) line_length: i64,
    pub(crate) rules: &'static [Rule],
    pub(crate) node_manager: NodeManager,
    pub(crate) node_buffer: Vec<NodeId>,
    pub(crate) comment_buffer: Vec<Comment>,
    pub(crate) line_buffer: Vec<Line>,
    pub(crate) rule_stack: Vec<&'static [Rule]>,
    pub(crate) pos: usize,
    pub(crate) arena: Arena,
}

impl Analyzer {
    pub(crate) fn new(line_length: i64, rules: &'static [Rule], node_manager: NodeManager) -> Analyzer {
        Analyzer {
            line_length,
            rules,
            node_manager,
            node_buffer: Vec::new(),
            comment_buffer: Vec::new(),
            line_buffer: Vec::new(),
            rule_stack: Vec::new(),
            pos: 0,
            arena: Arena::new(""),
        }
    }

    /// The most recently lexed node, or the last node of the last complete line.
    pub(crate) fn previous_node(&self) -> Option<NodeId> {
        self.node_buffer.last().copied().or_else(|| self.previous_line_node())
    }

    /// The last node of the last complete line.
    pub(crate) fn previous_line_node(&self) -> Option<NodeId> {
        self.line_buffer.last().and_then(|l| l.nodes.last().copied())
    }

    fn clear_buffers(&mut self, source: &str) {
        self.node_buffer.clear();
        self.comment_buffer.clear();
        self.line_buffer.clear();
        self.pos = 0;
        self.arena = Arena::new(source);
    }

    /// Flushes the buffers into the query's lines.
    fn write_buffers_to_query(&mut self) -> Result<Vec<Line>, Flow> {
        // a final line for a file that does not end with a newline
        if !self.node_buffer.is_empty() || !self.comment_buffer.is_empty() {
            let mut line = Line::from_nodes(
                &self.arena,
                self.previous_line_node(),
                std::mem::take(&mut self.node_buffer),
                std::mem::take(&mut self.comment_buffer),
            );
            self.node_manager.append_newline(&mut self.arena, &mut line)?;
            self.line_buffer.push(line);
        }
        // trailing jinja block end tags are formatted as if no brackets were open
        for line in self.line_buffer.iter().rev() {
            if line.is_standalone_jinja_statement(&self.arena)
                && line.closes_jinja_block_from_previous_line(&self.arena)
            {
                for &n in &line.nodes {
                    self.arena.get_mut(n).open_brackets.clear();
                }
            } else {
                break;
            }
        }
        Ok(std::mem::take(&mut self.line_buffer))
    }

    /// Lexes the source string into a structured Query.
    pub(crate) fn parse_query(&mut self, source: &str) -> Result<Query, Flow> {
        self.clear_buffers(source);
        self.lex(source)?;
        let lines = self.write_buffers_to_query()?;
        let arena = std::mem::take(&mut self.arena);
        Ok(Query { line_length: self.line_length, lines, arena })
    }

    fn push_rules(&mut self, set: RuleSet) {
        self.rule_stack.push(self.rules);
        self.rules = rules::rules(set);
    }

    fn pop_rules(&mut self) -> Result<(), Fatal> {
        self.rules = self.rule_stack.pop().ok_or_else(|| Fatal::new("IndexError: pop from empty list"))?;
        Ok(())
    }

    fn get_rule(&self, name: &str) -> Result<&'static Rule, Fatal> {
        get_rule(self.rules, name).ok_or_else(|| Fatal::new(format!("ValueError: No rule '{name}'")))
    }

    /// Applies the first rule that matches at the current position.
    fn lex_one(&mut self, source: &str) -> LexResult {
        let rules = self.rules;
        for rule in rules {
            let caps = rule
                .program
                .captures_from_pos(source, self.pos)
                .map_err(|e| Fatal::new(format!("regex error: {e}")))?;
            if let Some(caps) = caps {
                return self.apply(&rule.action, source, &caps);
            }
        }
        let start = pystr::char_index(source, self.pos);
        let context = pystr::char_slice(source, start, start + 50);
        Err(SqlfmtError::parsing(format!("Could not parse SQL at position {start}: '{}'", pystr::strip(context)))
            .into())
    }

    /// Lexes until the source is exhausted (trailing whitespace is not lexed).
    fn lex(&mut self, source: &str) -> LexResult {
        let eof = source.char_indices().rev().find(|(_, c)| !pystr::is_space(*c)).map(|(i, c)| i + c.len_utf8());
        let Some(eof) = eof else { return Ok(()) };
        let mut last_loop_pos: Option<usize> = None;
        while self.pos < eof && last_loop_pos.is_none_or(|l| self.pos > l) {
            last_loop_pos = Some(self.pos);
            self.lex_one(source)?;
        }
        Ok(())
    }

    /// The end position of the token that closes the bracket opened at `pos`
    /// (searching from `tail`), following nested openings.
    fn search_for_terminating_token(
        &self,
        start_rule: &str,
        program: &Regex,
        nesting_program: &Regex,
        source: &str,
        tail: usize,
        pos: usize,
    ) -> Result<usize, Flow> {
        let caps = program.captures_from_pos(source, tail).map_err(|e| Fatal::new(format!("regex error: {e}")))?;
        let Some(caps) = caps else {
            return Err(SqlfmtError::bracket(format!(
                "Unterminated multiline token '{start_rule}' started near position {}.",
                self.arena.char_pos(pos)
            ))
            .into());
        };
        let g = caps.get(1).expect("terminating token group");
        let (start, end) = (g.start(), g.end());
        let nested = nesting_program
            .find_from_pos(source, start)
            .map_err(|e| Fatal::new(format!("regex error: {e}")))?
            .is_some();
        if nested {
            let inner = self.search_for_terminating_token(start_rule, program, nesting_program, source, end, end)?;
            self.search_for_terminating_token(start_rule, program, nesting_program, source, inner, inner)
        } else {
            Ok(end)
        }
    }

    fn add_node(
        &mut self,
        source: &str,
        caps: &Captures<'_, str>,
        ttype: TokenType,
        previous_node: Option<Option<NodeId>>,
    ) -> LexResult {
        let previous_node = previous_node.unwrap_or_else(|| self.previous_node());
        let token = Token::from_match(source, caps, ttype);
        let epos = token.epos;
        let node = self.node_manager.create_node(&mut self.arena, token, previous_node)?;
        self.node_buffer.push(node);
        self.pos = epos;
        Ok(())
    }

    fn apply(&mut self, action: &Action, source: &str, caps: &Captures<'_, str>) -> LexResult {
        match action {
            Action::AddNode(t) => self.add_node(source, caps, *t, None),
            Action::SafeAddNode(t, fallback) => match self.add_node(source, caps, *t, None) {
                Err(Flow::User(e)) if e.kind == ErrorKind::Bracket => self.add_node(source, caps, *fallback, None),
                other => other,
            },
            Action::Reserved(inner) => self.handle_reserved_keyword(source, caps, inner),
            Action::NonreservedTopLevel(inner) => {
                let token = Token::from_match(source, caps, TokenType::Name);
                let epos = token.epos;
                let prev = self.previous_node();
                let node = self.node_manager.create_node(&mut self.arena, token, prev)?;
                if self.arena.get(node).depth().0 > 0 {
                    self.node_buffer.push(node);
                    self.pos = epos;
                    Ok(())
                } else {
                    self.handle_reserved_keyword(source, caps, inner)
                }
            }
            Action::LexRuleset(set) => self.lex_ruleset(source, *set),
            Action::RaiseBracketError => Err(self.bracket_error(source, caps).into()),
            Action::AddComment => self.add_comment(source, caps),
            Action::AddJinjaComment => {
                self.add_comment(source, caps)?;
                Err(Flow::Stop)
            }
            Action::Newline => self.handle_newline(source, caps),
            Action::Semicolon => {
                if !self.rule_stack.is_empty() {
                    self.rules = self.rule_stack[0];
                    self.rule_stack.clear();
                }
                self.add_node(source, caps, TokenType::Semicolon, None)
            }
            Action::DdlAs => self.handle_ddl_as(source, caps),
            Action::ClosingAngleBracket => self.handle_closing_angle_bracket(source, caps),
            Action::SetOperator => self.handle_set_operator(source, caps),
            Action::Number => self.handle_number(source, caps),
            Action::JinjaBlockStart => {
                self.add_node(source, caps, TokenType::JinjaBlockStart, None)?;
                Err(Flow::Stop)
            }
            Action::JinjaBlockKeyword => match self.previous_node() {
                Some(prev) => {
                    let start_tag = self.arena.get(prev).open_jinja_blocks.last().copied().unwrap_or(prev);
                    let before_start = self.arena.get(start_tag).previous_node;
                    self.add_node(source, caps, TokenType::JinjaBlockKeyword, Some(before_start))?;
                    Err(Flow::Stop)
                }
                None => Err(self.bracket_error(source, caps).into()),
            },
            Action::JinjaDataBlockStart { ruleset, raises } => {
                self.add_node(source, caps, TokenType::JinjaBlockStart, None)?;
                match ruleset {
                    Some(set) => self.lex_ruleset(source, *set)?,
                    None => {
                        let current = self.rules;
                        self.lex_rules(source, current)?;
                    }
                }
                if *raises {
                    Err(Flow::Stop)
                } else {
                    Ok(())
                }
            }
            Action::JinjaBlockEnd { reset_sql_depth } => match self.previous_node() {
                Some(prev) => {
                    let start_tag = self.arena.get(prev).open_jinja_blocks.last().copied().unwrap_or(prev);
                    self.add_node(source, caps, TokenType::JinjaBlockEnd, None)?;
                    if *reset_sql_depth {
                        let brackets = self.arena.get(start_tag).open_brackets.clone();
                        if let Some(new) = self.previous_node() {
                            self.arena.get_mut(new).open_brackets = brackets;
                        }
                    }
                    Err(Flow::Stop)
                }
                None => Err(self.bracket_error(source, caps).into()),
            },
            Action::Jinja { start, end, ttype } => {
                self.handle_potentially_nested_tokens(source, caps, start, end, *ttype)?;
                Err(Flow::Stop)
            }
            Action::PotentiallyNested { start, end, ttype } => {
                self.handle_potentially_nested_tokens(source, caps, start, end, *ttype)
            }
        }
    }

    fn bracket_error(&self, source: &str, caps: &Captures<'_, str>) -> SqlfmtError {
        let g = caps.get(1).expect("bracket group");
        let spos = pystr::char_index(source, g.start());
        SqlfmtError::bracket(format!(
            "Encountered closing bracket '{}' at position {spos}, before matching opening bracket. Context: {}",
            g.as_str(),
            pystr::char_slice(source, spos, spos + 50)
        ))
    }

    /// Lexes the rest of the source with another rule set active; a nested rule set
    /// that finishes (StopRulesetLexing) returns to the previous rule set.
    fn lex_ruleset(&mut self, source: &str, set: RuleSet) -> LexResult {
        self.push_rules(set);
        self.lex_after_push(source)
    }

    fn lex_rules(&mut self, source: &str, rules: &'static [Rule]) -> LexResult {
        self.rule_stack.push(self.rules);
        self.rules = rules;
        self.lex_after_push(source)
    }

    fn lex_after_push(&mut self, source: &str) -> LexResult {
        match self.lex(source) {
            Ok(()) => Ok(()),
            Err(Flow::Stop) => Ok(self.pop_rules()?),
            Err(e) => Err(e),
        }
    }

    /// Reserved keywords that follow a dot are qualified names (`my_table.from`); all
    /// other matches run the keyword's action.
    fn handle_reserved_keyword(&mut self, source: &str, caps: &Captures<'_, str>, action: &Action) -> LexResult {
        let Some(prev) = self.previous_node() else {
            return self.apply(action, source, caps);
        };
        if self.arena.previous_token_type(Some(prev)) == Some(TokenType::Dot) {
            let token = Token::from_match(source, caps, TokenType::Name);
            if token.prefix.is_empty() {
                let epos = token.epos;
                let node = self.node_manager.create_node(&mut self.arena, token, Some(prev))?;
                self.node_buffer.push(node);
                self.pos = epos;
                return Ok(());
            }
        }
        self.apply(action, source, caps)
    }

    fn add_comment(&mut self, source: &str, caps: &Captures<'_, str>) -> LexResult {
        let token = Token::from_match(source, caps, TokenType::Comment);
        let is_standalone = self.node_buffer.is_empty() || token.token.contains('\n');
        let epos = token.epos;
        let comment = Comment { token, is_standalone, previous_node: self.previous_node() };
        self.comment_buffer.push(comment);
        self.pos = epos;
        Ok(())
    }

    /// A newline usually flushes the node and comment buffers into a new line. Standalone
    /// comments are instead attached to the next line with contents, except where
    /// formatting is disabled.
    fn handle_newline(&mut self, source: &str, caps: &Captures<'_, str>) -> LexResult {
        let token = Token::from_match(source, caps, TokenType::Newline);
        let epos = token.epos;
        let prev = self.previous_node();
        let nl = self.node_manager.create_node(&mut self.arena, token, prev)?;
        if !self.node_buffer.is_empty()
            || self.comment_buffer.is_empty()
            || !self.arena.get(nl).formatting_disabled.is_empty()
        {
            self.node_buffer.push(nl);
            let line = Line::from_nodes(
                &self.arena,
                self.previous_line_node(),
                std::mem::take(&mut self.node_buffer),
                std::mem::take(&mut self.comment_buffer),
            );
            self.line_buffer.push(line);
        }
        self.pos = epos;
        Ok(())
    }

    /// "as" in a create function statement: what follows is parsed with the previous
    /// (select) rules, unless the next token is a quoted name.
    fn handle_ddl_as(&mut self, source: &str, caps: &Captures<'_, str>) -> LexResult {
        self.add_node(source, caps, TokenType::UntermKeyword, None)?;
        // the quoted name and comment patterns of every rule set that has this action
        self.get_rule("quoted_name")?;
        self.get_rule("comment")?;
        static RE: OnceLock<Regex> = OnceLock::new();
        let re =
            RE.get_or_init(|| pyre::compile_anchored(&format!(r"({}|\s)*{}", sql_comment(), sql_quoted_exp()), "is"));
        let matched =
            re.find_from_pos(source, self.pos).map_err(|e| Fatal::new(format!("regex error: {e}")))?.is_some();
        if !matched {
            if self.rule_stack.is_empty() {
                return Err(
                    Fatal::new("AssertionError: Internal Error! Open an issue. Could not parse DDL 'as'").into()
                );
            }
            self.pop_rules()?;
        }
        Ok(())
    }

    /// ">" is a closing angle bracket, or the ">" operator or the start of another
    /// operator (like ">>") when there is no angle bracket to close.
    fn handle_closing_angle_bracket(&mut self, source: &str, caps: &Captures<'_, str>) -> LexResult {
        match self.add_node(source, caps, TokenType::BracketClose, None) {
            Err(Flow::User(e)) if e.kind == ErrorKind::Bracket => {
                let operator = self.get_rule("operator")?;
                let pattern = format!(r"\s*{}", operator.pattern);
                let re = operator_after_angle(&pattern);
                let op = re.captures_from_pos(source, self.pos).map_err(|e| Fatal::new(format!("regex error: {e}")))?;
                let Some(op) = op else {
                    return Err(
                        Fatal::new("AssertionError: Internal Error! Could not parse closing bracket '>'").into()
                    );
                };
                self.add_node(source, &op, TokenType::Operator, None)
            }
            other => other,
        }
    }

    /// EXCEPT is a set operator, except after a star (`select * except (fields)`),
    /// where it is a word operator.
    fn handle_set_operator(&mut self, source: &str, caps: &Captures<'_, str>) -> LexResult {
        let prev = self.previous_node();
        let mut token = Token::from_match(source, caps, TokenType::SetOperator);
        if pystr::lower(&token.token) == "except" && self.arena.previous_token_type(prev) == Some(TokenType::Star) {
            token.ttype = TokenType::WordOperator;
        }
        let epos = token.epos;
        let node = self.node_manager.create_node(&mut self.arena, token, prev)?;
        self.node_buffer.push(node);
        self.pos = epos;
        Ok(())
    }

    /// A signed number after a name, number, closing bracket or "end" is a binary
    /// operator followed by a number; otherwise the sign belongs to the number.
    fn handle_number(&mut self, source: &str, caps: &Captures<'_, str>) -> LexResult {
        let first = caps.get(1).and_then(|g| g.as_str().chars().next());
        if matches!(first, Some('+' | '-')) && self.previous_node().is_some() {
            let prev_type = self.arena.previous_token_type(self.previous_node());
            if matches!(
                prev_type,
                Some(
                    TokenType::Number
                        | TokenType::Name
                        | TokenType::QuotedName
                        | TokenType::StatementEnd
                        | TokenType::BracketClose
                )
            ) {
                static OP: OnceLock<Regex> = OnceLock::new();
                let re = OP.get_or_init(|| pyre::compile_anchored(r"\s*(\+|-)", ""));
                let op = re.captures_from_pos(source, self.pos).map_err(|e| Fatal::new(format!("regex error: {e}")))?;
                let Some(op) = op else {
                    return Err(Fatal::new(
                        "AssertionError: Internal error! Could not match symbol of binary operator",
                    )
                    .into());
                };
                return self.add_node(source, &op, TokenType::Operator, None);
            }
        }
        self.add_node(source, caps, TokenType::Number, None)
    }

    /// Lexes potentially nested tokens, like jinja tags or c-style block comments.
    fn handle_potentially_nested_tokens(
        &mut self,
        source: &str,
        caps: &Captures<'_, str>,
        start_name: &str,
        end_name: &str,
        ttype: TokenType,
    ) -> LexResult {
        let start_rule = self.get_rule(start_name)?;
        let end_rule = self.get_rule(end_name)?;
        let whole = caps.get(0).expect("group 0");
        let g = caps.get(1).expect("group 1");
        let (pos, spos) = (whole.start(), g.start());
        let mut epos = g.end();
        let program = search_program(&start_rule.pattern, &end_rule.pattern);
        loop {
            epos = self.search_for_terminating_token(start_name, &program, &start_rule.program, source, epos, epos)?;
            if start_name != "jinja_expression_start" {
                break;
            }
            // jinja expressions can contain nested dictionaries whose brackets might
            // match too early ({{ {'a': {'b': 1}} }}): the tag ends only where the
            // jinja lexer's last token is the end of the expression
            if jinja_lexer::final_token_type(&source[spos..epos])? == "variable_end" {
                break;
            }
        }
        let token = Token::new(ttype, &source[pos..spos], &source[spos..epos], pos, epos);
        let prev = self.previous_node();
        let node = self.node_manager.create_node(&mut self.arena, token, prev)?;
        self.node_buffer.push(node);
        self.pos = epos;
        Ok(())
    }
}

/// The unanchored search for either the start or the end of a nested token.
fn search_program(start: &str, end: &str) -> std::sync::Arc<Regex> {
    type Programs = Mutex<HashMap<(String, String), Arc<Regex>>>;
    static CACHE: OnceLock<Programs> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    cache
        .entry((start.to_string(), end.to_string()))
        .or_insert_with(|| {
            Arc::new(pyre::compile_with_flags(&format!("{MAYBE_WHITESPACES}{}", group(&[start, end])), "is"))
        })
        .clone()
}

fn operator_after_angle(pattern: &str) -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| pyre::compile_anchored(pattern, "is"))
}
