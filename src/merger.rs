//! Merges lines to use as little vertical space as possible while keeping the syntax
//! hierarchy the splitter established.
//!
//! Ported from sqlfmt/merger.py.

use crate::comment::Comment;
use crate::error::{Fatal, FormatError, SqlfmtError};
use crate::line::Line;
use crate::node::{Arena, NodeId};
use crate::operator_precedence::OperatorPrecedence;
use crate::segment::{self, create_segments_from_lines, Segment};

/// Why lines could not be merged (a normal outcome), or an error that stops
/// formatting.
#[derive(Debug)]
enum MergeError {
    CannotMerge,
    Stop(FormatError),
}

impl From<SqlfmtError> for MergeError {
    fn from(e: SqlfmtError) -> MergeError {
        MergeError::Stop(FormatError::User(e))
    }
}

impl From<Fatal> for MergeError {
    fn from(e: Fatal) -> MergeError {
        MergeError::Stop(FormatError::Fatal(e))
    }
}

type MergeResult<T> = Result<T, MergeError>;

pub(crate) struct LineMerger<'a> {
    pub(crate) arena: &'a Arena,
    pub(crate) line_length: i64,
}

impl<'a> LineMerger<'a> {
    pub(crate) fn new(arena: &'a Arena, line_length: i64) -> LineMerger<'a> {
        LineMerger { arena, line_length }
    }

    /// Merges every node of `lines` onto a single line (keeping leading and trailing
    /// blank lines); fails if that line would be too long or the lines cannot merge.
    fn create_merged_line(&self, lines: &[Line]) -> MergeResult<Vec<Line>> {
        if lines.len() <= 1 {
            return Ok(lines.to_vec());
        }
        let (nodes, comments) = self.extract_components(lines)?;
        let merged = Line::from_nodes(self.arena, lines[0].previous_node, nodes, comments);
        if merged.is_too_long(self.arena, self.line_length) {
            return Err(MergeError::CannotMerge);
        }
        let leading = self.leading_blank_lines(lines.iter());
        let mut trailing = self.leading_blank_lines(lines.iter().rev());
        trailing.reverse();
        let mut out = leading;
        out.push(merged);
        out.extend(trailing);
        Ok(out)
    }

    fn safe_create_merged_line(&self, lines: &[Line]) -> MergeResult<Vec<Line>> {
        match self.create_merged_line(lines) {
            Err(MergeError::CannotMerge) => Ok(lines.to_vec()),
            other => other,
        }
    }

    /// Every node of the lines with a single trailing newline, and every comment; fails
    /// if the lines contain nodes or comments that prevent merging.
    fn extract_components(&self, lines: &[Line]) -> MergeResult<(Vec<NodeId>, Vec<Comment>)> {
        let arena = self.arena;
        let mut nodes: Vec<NodeId> = Vec::new();
        let mut comments: Vec<Comment> = Vec::new();
        let mut final_newline: Option<NodeId> = None;
        let mut allow_multiline_jinja = true;
        let mut has_multiline_jinja = false;
        let mut has_inline_comment_above = false;
        for line in lines {
            // only a standalone comment above the first line or an inline comment after
            // the last line can be merged
            if !line.comments.is_empty() {
                // nothing merges after an inline comment, or with a databricks query hint
                if has_inline_comment_above || line.comments.iter().any(|c| c.is_databricks_query_hint()) {
                    return Err(MergeError::CannotMerge);
                } else if line.comments.len() == 1
                    && line.nodes.len() > 1
                    && (line.comments[0].is_inline()
                        || line.comments[0].previous_node == Some(line.nodes[line.nodes.len() - 2]))
                {
                    // a comment that must be rendered inline blocks merging, unless the
                    // next line is just a comma
                    has_inline_comment_above = true;
                } else if nodes.len() == 1 && arena.is_operator(nodes[0]) {
                    // a standalone operator merges into the contents, even with a comment
                    // in the way
                } else if !nodes.is_empty() {
                    return Err(MergeError::CannotMerge);
                }
            } else if has_inline_comment_above && !(line.is_standalone_comma(arena) || line.is_blank_line(arena)) {
                // inline comments followed by a lonely comma (leading commas) can merge
                return Err(MergeError::CannotMerge);
            }

            if has_multiline_jinja && !(line.starts_with_operator(arena) || line.starts_with_comma(arena)) {
                return Err(MergeError::CannotMerge);
            }
            let mut content_nodes = Vec::new();
            for &n in &line.nodes {
                if arena.get(n).is_newline() {
                    continue;
                }
                self.raise_unmergeable(n, allow_multiline_jinja)?;
                content_nodes.push(n);
            }
            if !content_nodes.is_empty() {
                final_newline = line.nodes.last().copied();
                nodes.extend(&content_nodes);
                // a line with multiline jinja merges into the preceding line only when it
                // is the second line and follows a standalone operator
                if !(allow_multiline_jinja && content_nodes.len() == 1 && arena.is_operator(content_nodes[0])) {
                    allow_multiline_jinja = false;
                }
                // a line merges into a preceding line with multiline jinja only when it
                // starts with an operator or a comma
                has_multiline_jinja = content_nodes.iter().any(|&n| arena.get(n).is_multiline_jinja());
            }
            comments.extend(line.comments.iter().cloned());
        }
        match final_newline {
            Some(nl) if !nodes.is_empty() => {
                nodes.push(nl);
                Ok((nodes, comments))
            }
            _ => Err(MergeError::CannotMerge),
        }
    }

    fn raise_unmergeable(&self, id: NodeId, allow_multiline_jinja: bool) -> MergeResult<()> {
        let node = self.arena.get(id);
        if !node.formatting_disabled.is_empty()
            || node.divides_queries()
            || (node.is_multiline_jinja() && !allow_multiline_jinja)
        {
            Err(MergeError::CannotMerge)
        } else {
            Ok(())
        }
    }

    fn leading_blank_lines<'l>(&self, lines: impl Iterator<Item = &'l Line>) -> Vec<Line> {
        lines.take_while(|l| l.is_blank_line(self.arena)).cloned().collect()
    }

    /// Tries to merge the lines onto a single line; if that fails, splits them into
    /// segments of equal depth, merges runs of operators at that depth, and recurses
    /// into each segment.
    pub(crate) fn maybe_merge_lines(&self, lines: &[Line]) -> Result<Vec<Line>, FormatError> {
        match self.merge(lines) {
            Ok(lines) => Ok(lines),
            Err(MergeError::Stop(e)) => Err(e),
            Err(MergeError::CannotMerge) => unreachable!("merge handles CannotMerge"),
        }
    }

    fn merge(&self, lines: &[Line]) -> MergeResult<Vec<Line>> {
        if lines.is_empty() || lines.iter().all(|l| l.formatting_disabled) {
            return Ok(lines.to_vec());
        }
        match self.create_merged_line(lines) {
            Ok(merged) => Ok(merged),
            Err(MergeError::Stop(e)) => Err(MergeError::Stop(e)),
            Err(MergeError::CannotMerge) => {
                let mut merged_lines = Vec::new();
                // too long for one line: split into segments at the depth of lines[0]
                let segments = create_segments_from_lines(self.arena, lines);
                // a segment that starts with a standalone operator merges its first two
                // lines before anything else
                let segments = self.fix_standalone_operators(segments)?;
                if segments.len() > 1 {
                    let segments = self.maybe_merge_operators(segments, OperatorPrecedence::tiers())?;
                    let segments = self.maybe_stubbornly_merge(segments)?;
                    for seg in &segments {
                        merged_lines.extend(self.merge(seg)?);
                    }
                } else {
                    // a single segment: keep its first line and look one level deeper
                    let only = &segments[0];
                    match segment::head(self.arena, only) {
                        Err(_) => merged_lines.extend(only.iter().cloned()),
                        Ok(i) => {
                            merged_lines.extend(only[..=i].iter().cloned());
                            for seg in segment::split_after(self.arena, only, i) {
                                merged_lines.extend(self.merge(&seg)?);
                            }
                        }
                    }
                }
                Ok(merged_lines)
            }
        }
    }

    fn fix_standalone_operators(&self, mut segments: Vec<Segment>) -> MergeResult<Vec<Segment>> {
        for seg in segments.iter_mut() {
            let Ok(i) = segment::head(self.arena, seg) else { continue };
            if !seg[i].is_standalone_operator(self.arena) {
                continue;
            }
            let Ok(j) = segment::head(self.arena, &seg[i + 1..]) else { continue };
            let end = (i + j + 2).min(seg.len());
            match self.create_merged_line(&seg[..end]) {
                Ok(merged) => {
                    seg.splice(..end, merged);
                }
                Err(MergeError::CannotMerge) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(segments)
    }

    /// Merges runs of segments that start with operators into the previous segment;
    /// runs that cannot be merged are retried at tighter operator tiers.
    fn maybe_merge_operators(
        &self,
        segments: Vec<Segment>,
        mut tiers: Vec<OperatorPrecedence>,
    ) -> MergeResult<Vec<Segment>> {
        if segments.len() <= 1 || tiers.is_empty() {
            return Ok(segments);
        }
        let precedence = tiers.pop().expect("tiers is not empty");
        let mut head = 0;
        let mut new_segments = Vec::new();
        for i in 1..segments.len() {
            if !self.segment_continues_operator_sequence(&segments[i], precedence)? {
                new_segments.extend(self.try_merge_operator_segments(segments[head..i].to_vec(), tiers.clone())?);
                head = i;
            }
        }
        new_segments.extend(self.try_merge_operator_segments(segments[head..].to_vec(), tiers.clone())?);
        Ok(new_segments)
    }

    /// True if the first line of the segment continues a sequence of operators of
    /// precedence at most `max_precedence` (or starts with a comma).
    fn segment_continues_operator_sequence(
        &self,
        seg: &[Line],
        max_precedence: OperatorPrecedence,
    ) -> MergeResult<bool> {
        let Ok(i) = segment::head(self.arena, seg) else {
            // blank segments keep the sequence going
            return Ok(true);
        };
        let line = &seg[i];
        if line.starts_with_operator(self.arena) && !line.previous_token_is_comma(self.arena) {
            let precedence = OperatorPrecedence::from_node(self.arena, line.nodes[0])?;
            if precedence <= max_precedence {
                return Ok(true);
            }
        }
        Ok(line.starts_with_comma(self.arena))
    }

    fn try_merge_operator_segments(
        &self,
        segments: Vec<Segment>,
        tiers: Vec<OperatorPrecedence>,
    ) -> MergeResult<Vec<Segment>> {
        if segments.len() <= 1 {
            return Ok(segments);
        }
        let all: Vec<Line> = segments.iter().flatten().cloned().collect();
        match self.create_merged_line(&all) {
            Ok(merged) => Ok(vec![merged]),
            Err(MergeError::CannotMerge) => self.maybe_merge_operators(segments, tiers),
            Err(e) => Err(e),
        }
    }

    /// Some operators (`as`, `over()`, `exclude()`, `[]` indexing, and most operators
    /// that open a bracket, like `in ()`) are forced onto the prior line even when the
    /// contents of their brackets do not fit there.
    fn maybe_stubbornly_merge(&self, segments: Vec<Segment>) -> MergeResult<Vec<Segment>> {
        if segments.len() <= 1 {
            return Ok(segments);
        }
        let mut new_segments = vec![segments[0].clone()];
        // first, the tightest operators (e.g. `over`)
        for seg in &segments[1..] {
            if self.segment_continues_operator_sequence(seg, OperatorPrecedence::OtherTight)? {
                new_segments = self.stubbornly_merge(new_segments, seg)?;
            } else {
                new_segments.push(seg.clone());
            }
        }
        if new_segments.len() == 1 {
            return Ok(new_segments);
        }
        // next, comparison-tier operators that open a bracket closed in their segment
        let segments = new_segments;
        let mut new_segments = vec![segments[0].clone()];
        let mut starts_with_p1 = Vec::with_capacity(segments.len());
        for seg in &segments {
            starts_with_p1.push(self.segment_continues_operator_sequence(seg, OperatorPrecedence::Comparators)?);
        }
        for i in 1..segments.len() {
            let seg = &segments[i];
            if !starts_with_p1[i - 1]
                && starts_with_p1[i]
                && segment::tail_closes_head(self.arena, &self.safe_create_merged_line(seg)?)
            {
                new_segments = self.stubbornly_merge(new_segments, seg)?;
            } else {
                new_segments.push(seg.clone());
            }
        }
        Ok(new_segments)
    }

    /// Merges `seg` into the last of `prev_segments` in the best possible way: its
    /// first line onto the previous segment, the whole segment onto the previous
    /// segment's last line, or just its first line onto that last line.
    fn stubbornly_merge(&self, prev_segments: Vec<Segment>, seg: &[Line]) -> MergeResult<Vec<Segment>> {
        let mut new_segments = prev_segments;
        let mut prev_segment = new_segments.pop().expect("there is a previous segment");
        let Ok(i) = segment::head(self.arena, seg) else {
            new_segments.push(prev_segment);
            new_segments.push(seg.to_vec());
            return Ok(new_segments);
        };
        let head_line = seg[i].clone();

        // the first line of this segment onto the previous segment
        let mut candidate = prev_segment.clone();
        candidate.push(head_line.clone());
        match self.create_merged_line(&candidate) {
            Ok(mut merged) => {
                merged.extend(seg[i + 1..].iter().cloned());
                new_segments.push(merged);
                return Ok(new_segments);
            }
            Err(MergeError::CannotMerge) => {}
            Err(e) => return Err(e),
        }

        // this segment onto the last line of the previous segment
        let k = segment::tail(self.arena, &prev_segment)?;
        let last_index = prev_segment.len() - (k + 1);
        let last_line = prev_segment[last_index].clone();
        let mut candidate = vec![last_line.clone()];
        candidate.extend(seg.iter().cloned());
        match self.create_merged_line(&candidate) {
            Ok(new_last_lines) => {
                prev_segment.splice(last_index.., new_last_lines);
                new_segments.push(prev_segment);
                return Ok(new_segments);
            }
            Err(MergeError::CannotMerge) => {}
            Err(e) => return Err(e),
        }

        // just the first line of this segment onto the last line of the previous one
        match self.create_merged_line(&[last_line, head_line]) {
            Ok(new_last_lines) => {
                prev_segment.splice(last_index.., new_last_lines);
                prev_segment.extend(seg[i + 1..].iter().cloned());
                new_segments.push(prev_segment);
            }
            Err(MergeError::CannotMerge) => {
                new_segments.push(prev_segment);
                new_segments.push(seg.to_vec());
            }
            Err(e) => return Err(e),
        }
        Ok(new_segments)
    }
}
