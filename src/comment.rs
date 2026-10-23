//! Comments and how they are printed.
//!
//! Ported from sqlfmt/comment.py.

use std::sync::OnceLock;

use regex::Regex;

use crate::node::{Arena, NodeId};
use crate::pyre;
use crate::pystr;
use crate::tokens::{Token, TokenType};

fn comment_marker() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| pyre::compile_plain(r"(--|#|//|/\*|\{#-?)([^\S\n]*)", ""))
}

fn jinja_in_comment() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| pyre::compile_plain(r".*({{.*?}}|{%.*?%}).*", ""))
}

/// A Comment wraps a COMMENT token and knows how to print itself inline or on its
/// own line.
#[derive(Debug, Clone)]
pub(crate) struct Comment {
    pub(crate) token: Token,
    pub(crate) is_standalone: bool,
    pub(crate) previous_node: Option<NodeId>,
}

impl Comment {
    /// The comment without preceding whitespace, with a single space between the
    /// marker and the comment text.
    pub(crate) fn text(&self, arena: &Arena) -> String {
        if self.is_multiline() || self.formatting_disabled(arena) || self.is_databricks_query_hint() {
            self.token.token.clone()
        } else {
            let (marker, comment_text) = self.comment_parts();
            if comment_text.is_empty() {
                marker
            } else {
                format!("{marker} {comment_text}")
            }
        }
    }

    pub(crate) fn len(&self, arena: &Arena) -> usize {
        pystr::char_len(&self.text(arena))
    }

    /// The marker (e.g. "--" or "#") and the byte offset of the comment text.
    fn get_marker(&self) -> (String, usize) {
        // every comment token starts with one of the markers its lexing rule matched
        let caps = comment_marker()
            .captures(&self.token.token)
            .unwrap_or_else(|| panic!("{} does not match comment marker", self.token.token));
        let marker_end = caps.get(1).map_or(0, |m| m.end());
        let text_offset = caps.get(2).map_or(marker_end, |m| m.end());
        (self.token.token[..marker_end].to_string(), text_offset)
    }

    fn rewrite_marker(marker: String) -> String {
        if marker == "//" {
            "--".to_string()
        } else {
            marker
        }
    }

    /// The comment's (rewritten) marker and its contents without leading whitespace.
    pub(crate) fn comment_parts(&self) -> (String, String) {
        let (marker, skip) = self.get_marker();
        (Self::rewrite_marker(marker), self.token.token[skip..].to_string())
    }

    pub(crate) fn is_multiline(&self) -> bool {
        self.token.token.contains('\n')
    }

    pub(crate) fn is_c_style(&self) -> bool {
        self.token.token.starts_with("/*")
    }

    pub(crate) fn is_databricks_query_hint(&self) -> bool {
        self.token.token.starts_with("/*+")
    }

    pub(crate) fn is_inline(&self) -> bool {
        !self.is_standalone && !self.is_multiline() && !self.is_c_style()
    }

    /// The comment's text without markers or surrounding whitespace.
    pub(crate) fn body(&self) -> String {
        if self.is_multiline() {
            pystr::strip(&self.token.token).to_string()
        } else {
            pystr::strip(&self.comment_parts().1).to_string()
        }
    }

    /// Comment formatting is only disabled by an explicit FMT_OFF token (not by DATA
    /// nodes).
    pub(crate) fn formatting_disabled(&self, arena: &Arena) -> bool {
        match self.previous_node {
            None => false,
            Some(id) => arena.get(id).formatting_disabled.contains(&TokenType::FmtOff),
        }
    }

    /// Comments keep their original indentation when formatting is disabled or when
    /// they follow a run of DATA tokens.
    pub(crate) fn should_preserve_indentation(&self, arena: &Arena) -> bool {
        if self.formatting_disabled(arena) {
            return true;
        }
        let Some(prev) = self.previous_node else { return false };
        let prev = arena.get(prev);
        prev.is_newline() && prev.previous_node.is_some_and(|pp| arena.get(pp).token.ttype == TokenType::Data)
    }

    /// Renders the comment as an inline comment, assuming it fits.
    pub(crate) fn render_inline(&self, arena: &Arena) -> String {
        let prefix = if self.formatting_disabled(arena) { self.token.prefix.as_str() } else { "  " };
        format!("{prefix}{}", self.text(arena))
    }

    /// Renders the comment on its own line(s) at the given indentation, wrapping long
    /// single-line comments.
    pub(crate) fn render_standalone(&self, arena: &Arena, max_length: i64, prefix: &str) -> String {
        let prefix = if self.should_preserve_indentation(arena) { self.token.prefix.as_str() } else { prefix };
        let fits = (self.len(arena) + pystr::char_len(prefix)) as i64 <= max_length;
        let rendered = if self.formatting_disabled(arena) || self.is_multiline() || fits {
            format!("{prefix}{}", self.text(arena))
        } else {
            let (marker, comment_text) = self.comment_parts();
            if marker == "--" || marker == "#" {
                let available = max_length - pystr::char_len(prefix) as i64 - pystr::char_len(&marker) as i64 - 2;
                Self::split_before(&comment_text, available)
                    .iter()
                    .map(|txt| format!("{prefix}{marker} {}\n", pystr::strip(txt)))
                    .collect()
            } else {
                // block-style or jinja comment: long lines are not wrapped
                format!("{prefix}{}", self.text(arena))
            }
        };
        format!("{}\n", rendered.trim_end_matches('\n'))
    }

    /// Splits the text of a long comment at whitespace into pieces of at most
    /// `max_length` characters (jinja inside the comment is never split).
    fn split_before(text: &str, max_length: i64) -> Vec<String> {
        let mut out = Vec::new();
        let mut text = text.to_string();
        loop {
            if (pystr::char_len(&text) as i64) < max_length {
                out.push(pystr::rstrip(&text).to_string());
                return out;
            }
            if jinja_in_comment().is_match(&text) {
                out.push(pystr::rstrip(&text).to_string());
                return out;
            }
            let head: Vec<char> = py_prefix(&text, max_length);
            let split = head.iter().rposition(|c| pystr::is_space(*c));
            match split {
                Some(idx) => {
                    let cut = idx + 1;
                    let first: String = text.chars().take(cut).collect();
                    out.push(pystr::rstrip(&first).to_string());
                    text = text.chars().skip(cut).collect();
                }
                None => {
                    out.push(pystr::rstrip(&text).to_string());
                    return out;
                }
            }
        }
    }
}

/// `text[:n]` with the slice semantics of a possibly negative end index.
fn py_prefix(text: &str, n: i64) -> Vec<char> {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len() as i64;
    let end = if n < 0 { (len + n).max(0) } else { n.min(len) };
    chars[..end as usize].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(text: &str) -> Comment {
        Comment {
            token: Token::new(TokenType::Comment, "", text, 0, text.len()),
            is_standalone: true,
            previous_node: None,
        }
    }

    #[test]
    fn markers_are_normalised() {
        let arena = Arena::new("");
        assert_eq!(comment("--no space").text(&arena), "-- no space");
        assert_eq!(comment("//slashes").text(&arena), "-- slashes");
        assert_eq!(comment("#   hash").text(&arena), "# hash");
        assert_eq!(comment("/* block */").text(&arena), "/* block */");
    }

    #[test]
    fn long_comments_wrap_at_whitespace() {
        let arena = Arena::new("");
        let long = comment("-- one two three four five six seven eight nine ten");
        assert_eq!(
            long.render_standalone(&arena, 20, ""),
            "-- one two three\n-- four five six\n-- seven eight\n-- nine ten\n"
        );
    }
}
