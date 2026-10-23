//! Segments: runs of consecutive lines that are indented from their first line.
//!
//! Ported from sqlfmt/segment.py.

use crate::error::SqlfmtError;
use crate::line::Line;
use crate::node::Arena;

pub(crate) type Segment = Vec<Line>;

/// Splits lines into segments: each segment starts with a line and runs until a line
/// returns to that line's depth.
pub(crate) fn create_segments_from_lines(arena: &Arena, lines: &[Line]) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut j = 0;
    while j < lines.len() {
        let target_depth = lines[j].depth(arena);
        let start = if lines[j].is_standalone_operator(arena) { j + 2 } else { j + 1 };
        let next = (start..lines.len()).find(|&i| lines[i].starts_new_segment(arena, target_depth));
        match next {
            Some(i) => {
                segments.push(lines[j..i].to_vec());
                j = i;
            }
            None => {
                segments.push(lines[j..].to_vec());
                break;
            }
        }
    }
    segments
}

fn all_empty() -> SqlfmtError {
    SqlfmtError::segment("All lines in the segment are empty")
}

/// The index of the first non-blank line.
pub(crate) fn head(arena: &Arena, segment: &[Line]) -> Result<usize, SqlfmtError> {
    segment.iter().position(|l| !l.is_blank_line(arena)).ok_or_else(all_empty)
}

/// The index, counted from the bottom, of the last non-blank line.
pub(crate) fn tail(arena: &Arena, segment: &[Line]) -> Result<usize, SqlfmtError> {
    segment.iter().rev().position(|l| !l.is_blank_line(arena)).ok_or_else(all_empty)
}

/// `segment[start .. len - from_end]`, empty when the bounds cross.
fn middle(segment: &[Line], start: usize, from_end: usize) -> &[Line] {
    let end = segment.len().saturating_sub(from_end);
    if start >= end {
        &[]
    } else {
        &segment[start..end]
    }
}

/// True only if the last line of the segment closes a bracket or simple jinja block
/// opened by the first line of the segment.
pub(crate) fn tail_closes_head(arena: &Arena, segment: &[Line]) -> bool {
    if segment.len() <= 1 {
        return false;
    }
    let (Ok(i), Ok(j)) = (head(arena, segment), tail(arena, segment)) else {
        return false;
    };
    if i == segment.len() - 1 - j {
        return false;
    }
    let head_line = &segment[i];
    let tail_line = &segment[segment.len() - 1 - j];
    let between = middle(segment, i + 1, j + 1);
    let head_depth = head_line.depth(arena);
    tail_line.depth(arena) == head_depth
        && ((tail_line.closes_bracket_from_previous_line(arena)
            && between.iter().all(|l| l.depth(arena).0 > head_depth.0))
            || (tail_line.closes_simple_jinja_block_from_previous_line(arena)
                && between.iter().all(|l| l.depth(arena).1 > head_depth.1)))
}

/// The lines after `idx`: one segment, or two when the last line closes the bracket
/// opened by the first line (the lines in between, then the closing line and the
/// trailing blank lines).
pub(crate) fn split_after(arena: &Arena, segment: &[Line], idx: usize) -> Vec<Segment> {
    if tail_closes_head(arena, segment) {
        let j = tail(arena, segment).expect("tail exists when the tail closes the head");
        let split = segment.len() - (j + 1);
        vec![middle(segment, idx + 1, j + 1).to_vec(), segment[split..].to_vec()]
    } else {
        vec![segment.get(idx + 1..).map(|s| s.to_vec()).unwrap_or_default()]
    }
}
