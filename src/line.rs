//! Lines: the nodes and comments printed together on one line.
//!
//! Ported from sqlfmt/line.py.

use crate::comment::Comment;
use crate::node::{Arena, NodeId};
use crate::pystr;
use crate::tokens::TokenType;

/// A Line is a list of Nodes and Comments that are printed together, on a single line.
#[derive(Debug, Clone)]
pub(crate) struct Line {
    /// The last node of the prior line, if any.
    pub(crate) previous_node: Option<NodeId>,
    pub(crate) nodes: Vec<NodeId>,
    pub(crate) comments: Vec<Comment>,
    pub(crate) formatting_disabled: bool,
}

impl Line {
    /// Creates a line from nodes; formatting is disabled for the line if it is disabled
    /// for its first or last node (or for the previous node, for a line without nodes).
    pub(crate) fn from_nodes(
        arena: &Arena,
        previous_node: Option<NodeId>,
        nodes: Vec<NodeId>,
        comments: Vec<Comment>,
    ) -> Line {
        let formatting_disabled = if let (Some(&first), Some(&last)) = (nodes.first(), nodes.last()) {
            !arena.get(first).formatting_disabled.is_empty() || !arena.get(last).formatting_disabled.is_empty()
        } else {
            previous_node.is_some_and(|p| !arena.get(p).formatting_disabled.is_empty())
        };
        Line { previous_node, nodes, comments, formatting_disabled }
    }

    /// The line as printed, without comments: blank lines are bare newlines, lines
    /// with disabled formatting print their tokens as lexed, and other lines print
    /// their nodes after the indentation.
    pub(crate) fn render(&self, arena: &Arena) -> String {
        if self.is_blank_line(arena) {
            "\n".to_string()
        } else if self.formatting_disabled {
            self.nodes
                .iter()
                .map(|&n| {
                    let t = &arena.get(n).token;
                    format!("{}{}", t.prefix, t.token)
                })
                .collect()
        } else {
            let body: String = self.nodes.iter().map(|&n| arena.get(n).text()).collect();
            format!("{}{}", self.prefix(arena), body.trim_start_matches(' '))
        }
    }

    /// The length of the longest printed line (a line can contain multiline nodes).
    pub(crate) fn len(&self, arena: &Arena) -> usize {
        pystr::splitlines(&self.render(arena), false).iter().map(|s| pystr::char_len(s)).max().unwrap_or(0)
    }

    /// The brackets open at the start of this line.
    pub(crate) fn open_brackets<'a>(&self, arena: &'a Arena) -> &'a [NodeId] {
        if let Some(&first) = self.nodes.first() {
            &arena.get(first).open_brackets
        } else if let Some(p) = self.previous_node {
            &arena.get(p).open_brackets
        } else {
            &[]
        }
    }

    /// The jinja blocks open at the start of this line.
    pub(crate) fn open_jinja_blocks<'a>(&self, arena: &'a Arena) -> &'a [NodeId] {
        if let Some(&first) = self.nodes.first() {
            &arena.get(first).open_jinja_blocks
        } else if let Some(p) = self.previous_node {
            &arena.get(p).open_jinja_blocks
        } else {
            &[]
        }
    }

    /// The depth of the start of this line.
    pub(crate) fn depth(&self, arena: &Arena) -> (usize, usize) {
        if self.nodes.is_empty() {
            (0, 0)
        } else {
            (self.open_brackets(arena).len(), self.open_jinja_blocks(arena).len())
        }
    }

    /// The indentation printed at the start of this line.
    pub(crate) fn prefix(&self, arena: &Arena) -> String {
        let (sql, jinja) = self.depth(arena);
        " ".repeat(4 * (sql + jinja))
    }

    /// The line including its comments: standalone and multiline comments go on their
    /// own lines above it, inline comments after it.
    pub(crate) fn render_with_comments(&self, arena: &Arena, max_length: i64) -> String {
        let rendered = self.render(arena);
        let content = pystr::rstrip(&rendered);
        let mut out = String::new();
        let mut inline_comments: Vec<String> = Vec::new();
        for comment in &self.comments {
            if comment.is_multiline() || comment.is_standalone {
                out.push_str(&comment.render_standalone(arena, max_length, &self.prefix(arena)));
            } else {
                inline_comments.push(comment.render_inline(arena));
            }
        }
        if !inline_comments.is_empty() {
            out.push_str(content);
            out.push_str(&inline_comments.concat());
            out.push('\n');
        } else if !content.is_empty() || self.comments.is_empty() {
            out.push_str(&rendered);
        }
        out
    }

    pub(crate) fn previous_token_is_comma(&self, arena: &Arena) -> bool {
        arena.previous_token_type(self.previous_node) == Some(TokenType::Comma)
    }

    pub(crate) fn is_blank_line(&self, arena: &Arena) -> bool {
        self.nodes.len() == 1 && arena.get(self.nodes[0]).is_newline() && self.comments.is_empty()
    }

    pub(crate) fn starts_with_operator(&self, arena: &Arena) -> bool {
        self.nodes.first().is_some_and(|&n| arena.is_operator(n))
    }

    pub(crate) fn starts_with_comma(&self, arena: &Arena) -> bool {
        self.nodes.first().is_some_and(|&n| arena.get(n).is_comma())
    }

    pub(crate) fn starts_with_jinja_statement(&self, arena: &Arena) -> bool {
        self.nodes.first().is_some_and(|&n| arena.get(n).is_jinja_statement())
    }

    pub(crate) fn starts_with_bracket_operator(&self, arena: &Arena) -> bool {
        self.nodes.first().is_some_and(|&n| arena.is_bracket_operator(n))
    }

    pub(crate) fn contains_jinja(&self, arena: &Arena) -> bool {
        self.nodes.iter().any(|&n| arena.get(n).is_jinja())
    }

    pub(crate) fn is_standalone_jinja_statement(&self, arena: &Arena) -> bool {
        self.is_standalone_if(arena, self.starts_with_jinja_statement(arena))
    }

    pub(crate) fn is_standalone_operator(&self, arena: &Arena) -> bool {
        self.is_standalone_if(arena, self.starts_with_operator(arena) && !self.starts_with_bracket_operator(arena))
    }

    pub(crate) fn is_standalone_comma(&self, arena: &Arena) -> bool {
        self.is_standalone_if(arena, self.starts_with_comma(arena))
    }

    fn is_standalone_if(&self, arena: &Arena, starts_with_type: bool) -> bool {
        (self.nodes.len() == 1 && starts_with_type)
            || (self.nodes.len() == 2 && starts_with_type && arena.get(self.nodes[1]).is_newline())
    }

    /// True if the rendered line is longer than `max_length`.
    pub(crate) fn is_too_long(&self, arena: &Arena, max_length: i64) -> bool {
        self.len(arena) as i64 > max_length
    }

    /// True for a line with an explicit closing bracket (like ")" or "]") that matches
    /// a bracket opened on a preceding line.
    pub(crate) fn closes_bracket_from_previous_line(&self, arena: &Arena) -> bool {
        let (Some(prev), Some(&last)) = (self.previous_node, self.nodes.last()) else {
            return false;
        };
        let prev_open = &arena.get(prev).open_brackets;
        if prev_open.is_empty() {
            return false;
        }
        let explicit: Vec<NodeId> = prev_open.iter().copied().filter(|&b| arena.get(b).is_opening_bracket()).collect();
        match explicit.last() {
            Some(b) => !arena.get(last).open_brackets.contains(b),
            None => false,
        }
    }

    /// True if the previous line is inside a jinja block, but not after a block keyword
    /// like {% else %} or {% elif %}.
    pub(crate) fn previous_line_has_open_jinja_blocks_not_keywords(&self, arena: &Arena) -> bool {
        let Some(prev) = self.previous_node else { return false };
        match arena.get(prev).open_jinja_blocks.last() {
            Some(&b) => !arena.get(b).is_jinja_block_keyword(),
            None => false,
        }
    }

    /// True for a line that closes a jinja block opened on a previous line.
    pub(crate) fn closes_jinja_block_from_previous_line(&self, arena: &Arena) -> bool {
        let (Some(prev), Some(&last)) = (self.previous_node, self.nodes.last()) else {
            return false;
        };
        let Some(&prev_block) = arena.get(prev).open_jinja_blocks.last() else {
            return false;
        };
        let last_blocks = &arena.get(last).open_jinja_blocks;
        !last_blocks.contains(&prev_block)
            && match last_blocks.last() {
                None => true,
                Some(&b) => !arena.get(b).is_jinja_block_keyword(),
            }
    }

    /// Like [`Line::closes_jinja_block_from_previous_line`], but false for {% else %},
    /// {% elif %} and an {% endif %} that follows one of them.
    pub(crate) fn closes_simple_jinja_block_from_previous_line(&self, arena: &Arena) -> bool {
        self.previous_line_has_open_jinja_blocks_not_keywords(arena)
            && self.closes_jinja_block_from_previous_line(arena)
    }

    pub(crate) fn starts_new_segment(&self, arena: &Arena, prev_segment_depth: (usize, usize)) -> bool {
        let depth = self.depth(arena);
        if depth <= prev_segment_depth || depth.1 < prev_segment_depth.1 {
            // a line that starts with a closing bracket stays in the same segment as
            // the line that opened it
            !((self.closes_bracket_from_previous_line(arena)
                || self.closes_simple_jinja_block_from_previous_line(arena)
                || self.is_blank_line(arena))
                && depth == prev_segment_depth)
        } else {
            false
        }
    }
}
