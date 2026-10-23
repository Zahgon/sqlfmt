//! Nodes wrap lexed tokens with the formatting properties computed for them.
//!
//! Ported from sqlfmt/node.py. Nodes refer to each other (the previous node, the open
//! brackets and jinja blocks that set a node's depth), so they live in an [`Arena`]
//! and are addressed by [`NodeId`].

use crate::pystr;
use crate::tokens::{Token, TokenType};

pub(crate) type NodeId = usize;

/// A Node wraps a lexed Token and adds the calculated properties used to format it.
///
/// * `previous_node`: the node that immediately precedes this one in the query
///   (on the same or a previous line);
/// * `prefix`: the whitespace (0 or 1 spaces) printed before the node;
/// * `value`: the token contents as printed (keywords lowercased and so on);
/// * `open_brackets` / `open_jinja_blocks`: the brackets (keywords and parens) and
///   jinja blocks open before this node, which set its depth;
/// * `formatting_disabled`: the `FMT_OFF`/`DATA` tokens that disable formatting of
///   this node.
#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub(crate) token: Token,
    pub(crate) previous_node: Option<NodeId>,
    pub(crate) prefix: String,
    pub(crate) value: String,
    pub(crate) open_brackets: Vec<NodeId>,
    pub(crate) open_jinja_blocks: Vec<NodeId>,
    pub(crate) formatting_disabled: Vec<TokenType>,
}

impl Node {
    /// The formatted text of this node, including its prefix.
    pub(crate) fn text(&self) -> String {
        format!("{}{}", self.prefix, self.value)
    }

    /// SQL depth and jinja depth, tracked separately since SQL depth can change
    /// within jinja blocks.
    pub(crate) fn depth(&self) -> (usize, usize) {
        (self.open_brackets.len(), self.open_jinja_blocks.len())
    }

    pub(crate) fn is_unterm_keyword(&self) -> bool {
        self.token.ttype == TokenType::UntermKeyword
    }

    pub(crate) fn is_comma(&self) -> bool {
        self.token.ttype == TokenType::Comma
    }

    pub(crate) fn divides_queries(&self) -> bool {
        self.token.ttype.divides_queries()
    }

    pub(crate) fn is_opening_bracket(&self) -> bool {
        self.token.ttype.is_opening_bracket()
    }

    pub(crate) fn is_closing_bracket(&self) -> bool {
        matches!(self.token.ttype, TokenType::BracketClose | TokenType::StatementEnd)
    }

    pub(crate) fn is_opening_jinja_block(&self) -> bool {
        matches!(self.token.ttype, TokenType::JinjaBlockStart | TokenType::JinjaBlockKeyword)
    }

    pub(crate) fn is_jinja(&self) -> bool {
        self.token.ttype.is_jinja()
    }

    pub(crate) fn is_closing_jinja_block(&self) -> bool {
        self.token.ttype == TokenType::JinjaBlockEnd
    }

    pub(crate) fn is_jinja_block_keyword(&self) -> bool {
        self.token.ttype == TokenType::JinjaBlockKeyword
    }

    pub(crate) fn is_jinja_statement(&self) -> bool {
        self.token.ttype.is_jinja_statement()
    }

    pub(crate) fn is_boolean_operator(&self) -> bool {
        self.token.ttype == TokenType::BooleanOperator
    }

    /// A WORD_OPERATOR whose value is "between".
    pub(crate) fn is_the_between_operator(&self) -> bool {
        self.token.ttype == TokenType::WordOperator && self.value == "between"
    }

    pub(crate) fn is_newline(&self) -> bool {
        self.token.ttype == TokenType::Newline
    }

    pub(crate) fn is_multiline_jinja(&self) -> bool {
        self.token.ttype.is_jinja() && self.value.contains('\n')
    }
}

/// Every node of a query, addressed by [`NodeId`], and the source text the nodes'
/// token positions refer to.
#[derive(Debug, Clone, Default)]
pub(crate) struct Arena {
    nodes: Vec<Node>,
    source: String,
}

impl Arena {
    pub(crate) fn new(source: &str) -> Arena {
        Arena { nodes: Vec::new(), source: source.to_string() }
    }

    /// The character position (as reported to users) of a byte offset in the source.
    pub(crate) fn char_pos(&self, byte: usize) -> usize {
        if byte <= self.source.len() && self.source.is_char_boundary(byte) {
            pystr::char_index(&self.source, byte)
        } else {
            byte
        }
    }

    pub(crate) fn push(&mut self, node: Node) -> NodeId {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    pub(crate) fn get(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub(crate) fn get_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id]
    }

    /// The node whose token sets the SQL context before `prev`: newlines and jinja
    /// statements are skipped. The flag is true when any node was skipped.
    pub(crate) fn previous_token_node(&self, prev: Option<NodeId>) -> (Option<NodeId>, bool) {
        let mut current = prev;
        let mut skipped = false;
        while let Some(id) = current {
            let node = self.get(id);
            if node.token.ttype.does_not_set_prev_sql_context() {
                current = node.previous_node;
                skipped = true;
            } else {
                return (Some(id), skipped);
            }
        }
        (None, skipped)
    }

    /// The token type of the node found by [`Arena::previous_token_node`].
    pub(crate) fn previous_token_type(&self, prev: Option<NodeId>) -> Option<TokenType> {
        self.previous_token_node(prev).0.map(|id| self.get(id).token.ttype)
    }

    /// An opening square bracket that follows a token that could be a name, or an
    /// opening paren that follows a closing angle bracket (BQ struct literals).
    pub(crate) fn is_bracket_operator(&self, id: NodeId) -> bool {
        let node = self.get(id);
        if node.token.ttype != TokenType::BracketOpen {
            return false;
        }
        let Some(prev_id) = self.previous_token_node(node.previous_node).0 else {
            return false;
        };
        let prev = &self.get(prev_id).token;
        if node.value == "[" {
            matches!(prev.ttype, TokenType::Name | TokenType::QuotedName | TokenType::BracketClose)
        } else {
            node.value == "(" && prev.ttype == TokenType::BracketClose && prev.token.contains('>')
        }
    }

    /// A STAR token is the multiplication operator unless it follows a keyword, a comma
    /// or a dot (or nothing).
    pub(crate) fn is_multiplication_star(&self, id: NodeId) -> bool {
        let node = self.get(id);
        if node.token.ttype != TokenType::Star {
            return false;
        }
        match self.previous_token_type(node.previous_node) {
            None => false,
            Some(t) => !matches!(t, TokenType::UntermKeyword | TokenType::Comma | TokenType::Dot),
        }
    }

    pub(crate) fn is_operator(&self, id: NodeId) -> bool {
        let node = self.get(id);
        node.token.ttype.is_always_operator() || self.is_multiplication_star(id) || self.is_bracket_operator(id)
    }

    /// True if a "between" operator precedes this node at the same depth (before any
    /// boolean operator at that depth).
    pub(crate) fn has_preceding_between_operator(&self, id: NodeId) -> bool {
        let node = self.get(id);
        let depth = node.depth();
        let mut prev = node.previous_node.and_then(|p| self.get(p).previous_node);
        while let Some(p) = prev {
            let pn = self.get(p);
            if pn.depth() < depth {
                break;
            }
            if pn.depth() == depth && pn.is_the_between_operator() {
                return true;
            } else if pn.depth() == depth && pn.is_boolean_operator() {
                break;
            }
            prev = pn.previous_node;
        }
        false
    }

    /// A BOOLEAN_OPERATOR "and" that follows a "between" operator.
    pub(crate) fn is_the_and_after_the_between_operator(&self, id: NodeId) -> bool {
        let node = self.get(id);
        if !node.is_boolean_operator() || node.value != "and" {
            return false;
        }
        self.has_preceding_between_operator(id)
    }
}
