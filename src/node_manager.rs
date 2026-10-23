//! Creates nodes from tokens: depth, whitespace and normalised values.
//!
//! Ported from sqlfmt/node_manager.py.

use crate::error::SqlfmtError;
use crate::line::Line;
use crate::node::{Arena, Node, NodeId};
use crate::pystr;
use crate::tokens::{Token, TokenType};

#[derive(Debug, Clone, Copy)]
pub(crate) struct NodeManager {
    pub(crate) case_sensitive_names: bool,
}

impl NodeManager {
    pub(crate) fn new(case_sensitive_names: bool) -> NodeManager {
        NodeManager { case_sensitive_names }
    }

    /// Creates a Node from a Token and the node that precedes it in the query. The
    /// node's depth and whitespace are calculated here (this does most of the
    /// formatting of the node), and simple names, keywords and statements are
    /// lowercased.
    pub(crate) fn create_node(
        &self,
        arena: &mut Arena,
        token: Token,
        previous_node: Option<NodeId>,
    ) -> Result<NodeId, SqlfmtError> {
        let (open_brackets, open_jinja_blocks) = self.open_brackets(arena, &token, previous_node)?;
        let formatting_disabled = self.disable_formatting(arena, &token, previous_node);
        let (prefix, value) = if !formatting_disabled.is_empty() {
            (token.prefix.clone(), token.token.clone())
        } else {
            let (prev, extra_whitespace) = arena.previous_token_node(previous_node);
            let prev_token = prev.map(|p| arena.get(p).token.clone());
            (self.whitespace(&token, prev_token.as_ref(), extra_whitespace).to_string(), self.standardize_value(&token))
        };
        Ok(arena.push(Node {
            token,
            previous_node,
            prefix,
            value,
            open_brackets,
            open_jinja_blocks,
            formatting_disabled,
        }))
    }

    fn raise_on_mismatched_bracket(
        &self,
        arena: &Arena,
        token: &Token,
        last_bracket: NodeId,
    ) -> Result<(), SqlfmtError> {
        let last = arena.get(last_bracket);
        let last_value = pystr::lower(&last.value);
        let expected = match last_value.as_str() {
            "{" => Some("}"),
            "(" => Some(")"),
            "[" => Some("]"),
            "case" => Some("end"),
            "array<" | "map<" | "table<" | "struct<" => Some(">"),
            _ => None,
        };
        let ok = matches!(last.token.ttype, TokenType::BracketOpen | TokenType::StatementStart)
            && expected.is_some_and(|e| e == pystr::lower(&token.token));
        if ok {
            Ok(())
        } else {
            Err(SqlfmtError::bracket(format!(
                "Closing bracket '{}' found at {} does not match last opened bracket '{}' found at {}.",
                token.token,
                arena.char_pos(token.spos),
                last.value,
                arena.char_pos(last.token.spos)
            )))
        }
    }

    fn raise_on_mismatched_jinja_tags(
        &self,
        arena: &Arena,
        token: &Token,
        start_tag: NodeId,
    ) -> Result<(), SqlfmtError> {
        let start = arena.get(start_tag);
        let lowered = pystr::lower(&token.token);
        let ok = if lowered.contains("endif") {
            ["if", "elif", "else"].iter().any(|s| start.value.contains(s))
        } else if lowered.contains("endfor") {
            ["for", "else"].iter().any(|s| start.value.contains(s))
        } else {
            let end_text: String =
                lowered.chars().filter(|c| !matches!(c, '{' | '}' | '%' | '-') && !pystr::is_space(*c)).collect();
            let start_value = end_text.replace("end", "");
            start.value.contains(&start_value)
        };
        if ok {
            Ok(())
        } else {
            Err(SqlfmtError::bracket(format!(
                "Closing jinja tag '{}' found at pos {} does not match last opened tag '{}' found at pos {}.",
                token.token,
                arena.char_pos(token.spos),
                start.value,
                arena.char_pos(start.token.spos)
            )))
        }
    }

    /// Computes the open brackets and jinja blocks of a new node from the previous node
    /// and the new token.
    fn open_brackets(
        &self,
        arena: &Arena,
        token: &Token,
        previous_node: Option<NodeId>,
    ) -> Result<(Vec<NodeId>, Vec<NodeId>), SqlfmtError> {
        let (mut open_brackets, mut open_jinja_blocks) = match previous_node {
            None => (Vec::new(), Vec::new()),
            Some(p) => {
                let prev = arena.get(p);
                let mut ob = prev.open_brackets.clone();
                let mut oj = prev.open_jinja_blocks.clone();
                if prev.is_unterm_keyword() || prev.is_opening_bracket() {
                    ob.push(p);
                } else if prev.is_opening_jinja_block() {
                    oj.push(p);
                }
                (ob, oj)
            }
        };

        match token.ttype {
            TokenType::UntermKeyword | TokenType::SetOperator => {
                if open_brackets.last().is_some_and(|&b| arena.get(b).is_unterm_keyword()) {
                    open_brackets.pop();
                }
            }
            TokenType::BracketClose | TokenType::StatementEnd => {
                let before_open = || {
                    SqlfmtError::bracket(format!(
                        "Closing bracket '{}' found at {} before bracket was opened.",
                        token.token,
                        arena.char_pos(token.spos)
                    ))
                };
                let mut last = open_brackets.pop().ok_or_else(before_open)?;
                if arena.get(last).is_unterm_keyword() {
                    last = open_brackets.pop().ok_or_else(before_open)?;
                }
                self.raise_on_mismatched_bracket(arena, token, last)?;
            }
            TokenType::JinjaBlockEnd => {
                let start = open_jinja_blocks.pop().ok_or_else(|| {
                    SqlfmtError::bracket(format!(
                        "Closing bracket '{}' found at {} before bracket was opened.",
                        token.token,
                        arena.char_pos(token.spos)
                    ))
                })?;
                self.raise_on_mismatched_jinja_tags(arena, token, start)?;
            }
            TokenType::Semicolon => open_brackets.clear(),
            _ => {}
        }
        Ok((open_brackets, open_jinja_blocks))
    }

    /// The whitespace (none or one space) printed before the token.
    fn whitespace(&self, token: &Token, previous: Option<&Token>, extra_whitespace: bool) -> &'static str {
        const NO_SPACE: &str = "";
        const SPACE: &str = " ";
        let prev_type = previous.map(|t| t.ttype);
        let tt = token.ttype;
        if tt.is_never_preceded_by_space() {
            NO_SPACE
        } else if matches!(prev_type, Some(TokenType::BracketOpen | TokenType::DoubleColon)) {
            // no spaces after an open bracket or a cast operator (::)
            NO_SPACE
        } else if tt.is_preceded_by_space_except_after_open_bracket() {
            SPACE
        } else if tt.is_possible_name() && matches!(prev_type, Some(TokenType::Dot | TokenType::Colon)) {
            // namespaced identifiers
            NO_SPACE
        } else if tt == TokenType::Number && prev_type == Some(TokenType::Colon) {
            // simple slices
            NO_SPACE
        } else if tt == TokenType::BracketOpen && token.token.contains('<') {
            // bq type definitions like `array<int64>`
            if prev_type.is_some_and(|p| p != TokenType::BracketOpen) {
                SPACE
            } else {
                NO_SPACE
            }
        } else if tt == TokenType::BracketOpen
            && matches!(
                prev_type,
                Some(TokenType::Name | TokenType::QuotedName | TokenType::BracketOpen | TokenType::BracketClose)
            )
        {
            // function calls, array indexes and nested brackets
            NO_SPACE
        } else if tt == TokenType::BracketOpen && token.token == "[" && prev_type == Some(TokenType::Colon) {
            // escaped databricks variant columns
            NO_SPACE
        } else if tt == TokenType::BracketOpen {
            SPACE
        } else if tt.is_jinja() || prev_type == Some(TokenType::JinjaExpression) {
            // what jinja evaluates to is unknown, so the original spacing is respected
            if !token.prefix.is_empty() || extra_whitespace {
                SPACE
            } else {
                NO_SPACE
            }
        } else {
            SPACE
        }
    }

    /// Words (not symbols) that are not jinja or comments are lowercased, with internal
    /// whitespace collapsed to single spaces.
    fn standardize_value(&self, token: &Token) -> String {
        if token.ttype.is_always_lowercased() {
            pystr::collapse_whitespace(&pystr::lower(&token.token))
        } else if token.ttype == TokenType::Name && !self.case_sensitive_names {
            pystr::lower(&token.token)
        } else {
            token.token.clone()
        }
    }

    /// The FMT_OFF/DATA tokens that disable formatting for the node to be created.
    fn disable_formatting(&self, arena: &Arena, token: &Token, previous_node: Option<NodeId>) -> Vec<TokenType> {
        let mut disabled = previous_node.map(|p| arena.get(p).formatting_disabled.clone()).unwrap_or_default();
        if matches!(token.ttype, TokenType::FmtOff | TokenType::Data) {
            disabled.push(token.ttype);
        }
        if let Some(p) = previous_node {
            if !disabled.is_empty() && matches!(arena.get(p).token.ttype, TokenType::FmtOn | TokenType::Data) {
                disabled.pop();
            }
        }
        disabled
    }

    /// Creates a NEWLINE node and appends it to the end of the line.
    pub(crate) fn append_newline(&self, arena: &mut Arena, line: &mut Line) -> Result<(), SqlfmtError> {
        let previous_node = if let Some(&last) = line.nodes.last() { Some(last) } else { line.previous_node };
        let pos = previous_node.map_or(0, |p| arena.get(p).token.epos);
        let nl = Token::new(TokenType::Newline, "", "\n", pos, pos);
        let id = self.create_node(arena, nl, previous_node)?;
        line.nodes.push(id);
        Ok(())
    }
}
