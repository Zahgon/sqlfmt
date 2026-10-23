//! A Query: the lines of a source string and the line length to format them to.
//!
//! Ported from sqlfmt/query.py.

use crate::line::Line;
use crate::node::Arena;
use crate::tokens::Token;

#[derive(Debug, Clone)]
pub(crate) struct Query {
    pub(crate) line_length: i64,
    pub(crate) lines: Vec<Line>,
    pub(crate) arena: Arena,
}

impl Query {
    /// The tokens of every node, in order.
    pub(crate) fn tokens(&self) -> Vec<&Token> {
        self.lines.iter().flat_map(|l| l.nodes.iter().map(|&n| &self.arena.get(n).token)).collect()
    }

    /// The formatted query text.
    pub(crate) fn render(&self) -> String {
        self.lines.iter().map(|l| l.render_with_comments(&self.arena, self.line_length)).collect()
    }
}
