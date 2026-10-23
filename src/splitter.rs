//! Splits lines so that the depth of every line is consistent and the syntax apparent.
//!
//! Ported from sqlfmt/splitter.py.

use crate::comment::Comment;
use crate::error::SqlfmtError;
use crate::line::Line;
use crate::node::{Arena, NodeId};
use crate::node_manager::NodeManager;

pub(crate) struct LineSplitter {
    pub(crate) node_manager: NodeManager,
}

impl LineSplitter {
    pub(crate) fn new(node_manager: NodeManager) -> LineSplitter {
        LineSplitter { node_manager }
    }

    /// Splits a line before and after the nodes that change depth or start a new
    /// clause; returns the original line when nothing needs splitting.
    pub(crate) fn maybe_split(&self, arena: &mut Arena, line: Line) -> Result<Vec<Line>, SqlfmtError> {
        if line.formatting_disabled {
            return Ok(vec![line]);
        }
        let mut new_lines = Vec::new();
        let mut comments = line.comments.clone();
        let mut head = 0;
        let mut always_split_after = false;
        let mut never_split_after = false;
        for i in 0..line.nodes.len() {
            let node = line.nodes[i];
            if arena.get(node).is_newline() {
                if head == 0 {
                    new_lines.push(line);
                } else {
                    let (new_line, _rest) = self.split_at_index(arena, &line, head, Some(i), comments, true)?;
                    new_lines.push(new_line);
                }
                return Ok(new_lines);
            } else if i > head
                && !never_split_after
                && arena.get(node).formatting_disabled.is_empty()
                && (always_split_after || self.maybe_split_before(arena, node))
            {
                let (new_line, rest) = self.split_at_index(arena, &line, head, Some(i), comments, false)?;
                comments = rest;
                let last = *new_line.nodes.last().expect("a split line has nodes");
                new_lines.push(new_line);
                head = i;
                // the node now follows a new newline node
                arena.get_mut(node).previous_node = Some(last);
            }
            (always_split_after, never_split_after) = self.maybe_split_after(arena, node);
        }
        let (new_line, _rest) = self.split_at_index(arena, &line, head, None, comments, true)?;
        new_lines.push(new_line);
        Ok(new_lines)
    }

    /// True if the line should be split before this node.
    fn maybe_split_before(&self, arena: &Arena, id: NodeId) -> bool {
        let node = arena.get(id);
        // multiline nodes, unterminated keywords, opening jinja blocks, operators,
        // nodes that decrease depth and nodes that divide queries start a new line
        node.is_multiline_jinja()
            || node.is_unterm_keyword()
            || node.is_opening_jinja_block()
            || arena.is_operator(id)
            || node.is_closing_bracket()
            || node.is_closing_jinja_block()
            || node.divides_queries()
            || self.maybe_split_between_brackets(arena, id)
    }

    /// An opening bracket that immediately follows a closing bracket, as in BQ array
    /// indexing (`split(my_field)[offset(1)]`) or `my_json['outer']['inner']`.
    fn maybe_split_between_brackets(&self, arena: &Arena, id: NodeId) -> bool {
        let node = arena.get(id);
        node.is_opening_bracket() && node.previous_node.is_some_and(|p| arena.get(p).is_closing_bracket())
    }

    /// (always split after, never split after) for this node.
    fn maybe_split_after(&self, arena: &Arena, id: NodeId) -> (bool, bool) {
        let node = arena.get(id);
        if node.is_comma()
            || node.is_opening_bracket()
            || node.is_opening_jinja_block()
            || node.is_unterm_keyword()
            || node.divides_queries()
        {
            (true, false)
        } else if !node.formatting_disabled.is_empty() {
            (false, true)
        } else {
            (false, false)
        }
    }

    /// A new line made of `line.nodes[head..index]` plus a newline node, and the
    /// comments that stay with the rest of the line (inline comments, and every comment
    /// when the new line starts with a comma).
    pub(crate) fn split_at_index(
        &self,
        arena: &mut Arena,
        line: &Line,
        head: usize,
        index: Option<usize>,
        comments: Vec<Comment>,
        no_tail: bool,
    ) -> Result<(Line, Vec<Comment>), SqlfmtError> {
        let new_nodes: Vec<NodeId> = match index {
            None => line.nodes[head..].to_vec(),
            Some(index) => {
                assert!(index > head, "Cannot split at start of line!");
                line.nodes[head..index].to_vec()
            }
        };
        assert!(!new_nodes.is_empty(), "Cannot split a line without nodes!");

        let (head_comments, tail_comments) = if no_tail {
            (comments, Vec::new())
        } else if comments.is_empty() {
            (Vec::new(), Vec::new())
        } else if arena.get(new_nodes[0]).is_comma() {
            (Vec::new(), comments)
        } else if index.is_some_and(|i| i + 2 == line.nodes.len() && arena.is_operator(line.nodes[i])) {
            // only an operator and a newline follow the split: the comments stay with
            // the nodes before the operator
            (comments, Vec::new())
        } else {
            let mut head_comments = Vec::new();
            let mut tail_comments = Vec::new();
            for comment in comments {
                if comment.is_standalone || comment.is_multiline() || comment.previous_node.is_none() {
                    head_comments.push(comment);
                } else if comment.is_inline() {
                    tail_comments.push(comment);
                } else if comment.is_c_style() && comment.previous_node.is_some_and(|p| new_nodes.contains(&p)) {
                    head_comments.push(comment);
                } else {
                    tail_comments.push(comment);
                }
            }
            (head_comments, tail_comments)
        };

        let previous = arena.get(new_nodes[0]).previous_node;
        let mut new_line = Line::from_nodes(arena, previous, new_nodes, head_comments);
        let last = *new_line.nodes.last().expect("a split line has nodes");
        if !arena.get(last).is_newline() {
            self.node_manager.append_newline(arena, &mut new_line)?;
        }
        Ok((new_line, tail_comments))
    }
}
