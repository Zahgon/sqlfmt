//! The formatting pipeline applied to a lexed query.
//!
//! Ported from sqlfmt/query_formatter.py.

use crate::error::FormatError;
use crate::jinjafmt;
use crate::line::Line;
use crate::merger::LineMerger;
use crate::node::NodeId;
use crate::node_manager::NodeManager;
use crate::query::Query;
use crate::splitter::LineSplitter;

pub(crate) struct QueryFormatter {
    pub(crate) line_length: i64,
    pub(crate) case_sensitive_names: bool,
}

impl QueryFormatter {
    pub(crate) fn new(line_length: i64, case_sensitive_names: bool) -> QueryFormatter {
        QueryFormatter { line_length, case_sensitive_names }
    }

    /// Splits lines, formats jinja tags, dedents jinja block tags to their least
    /// indented contents, merges lines and removes extra blank lines.
    pub(crate) fn format(&self, mut query: Query) -> Result<Query, FormatError> {
        let lines = std::mem::take(&mut query.lines);
        let lines = self.split_lines(&mut query, lines)?;
        self.format_jinja(&mut query, &lines);
        self.dedent_jinja_blocks(&mut query, &lines);
        let lines = LineMerger::new(&query.arena, self.line_length).maybe_merge_lines(&lines)?;
        let lines = self.remove_extra_blank_lines(&query, lines);
        query.lines = lines;
        query.line_length = self.line_length;
        Ok(query)
    }

    fn split_lines(&self, query: &mut Query, lines: Vec<Line>) -> Result<Vec<Line>, FormatError> {
        let splitter = LineSplitter::new(NodeManager::new(self.case_sensitive_names));
        let mut new_lines = Vec::new();
        for line in lines {
            new_lines.extend(splitter.maybe_split(&mut query.arena, line)?);
        }
        Ok(new_lines)
    }

    fn format_jinja(&self, query: &mut Query, lines: &[Line]) {
        for line in lines {
            jinjafmt::format_line(&mut query.arena, line);
        }
    }

    /// Jinja block tags ({% if foo %}, {% endif %}) are printed at the depth of their
    /// least indented contents rather than at their own depth.
    fn dedent_jinja_blocks(&self, query: &mut Query, lines: &[Line]) {
        let arena = &mut query.arena;
        let mut start_node: Option<NodeId> = None;
        for line in lines {
            if line.is_standalone_jinja_statement(arena)
                && arena.get(line.nodes[0]).is_closing_jinja_block()
                && !line.formatting_disabled
            {
                let start = start_node.expect("a closing jinja block follows its opening block");
                let brackets = arena.get(start).open_brackets.clone();
                arena.get_mut(line.nodes[0]).open_brackets = brackets;
            }
            if let Some(&last) = line.nodes.last() {
                if let Some(&start) = arena.get(last).open_jinja_blocks.last() {
                    start_node = Some(start);
                    let line_brackets = line.open_brackets(arena).to_vec();
                    if line_brackets.len() < arena.get(start).open_brackets.len()
                        && arena.get(start).formatting_disabled.is_empty()
                    {
                        arena.get_mut(start).open_brackets = line_brackets;
                    }
                }
            }
        }
    }

    /// At most two consecutive blank lines at depth (0, 0) and one at any other depth;
    /// blank lines at the start of the query are removed.
    fn remove_extra_blank_lines(&self, query: &Query, lines: Vec<Line>) -> Vec<Line> {
        let mut new_lines = Vec::new();
        let mut count = 2;
        for line in lines {
            if line.is_blank_line(&query.arena) {
                let max = if line.depth(&query.arena) == (0, 0) { 2 } else { 1 };
                if count < max || line.formatting_disabled {
                    new_lines.push(line);
                }
                count += 1;
            } else {
                new_lines.push(line);
                count = 0;
            }
        }
        new_lines
    }
}
