//! Normalises the whitespace of jinja tags.
//!
//! Ported from sqlfmt/jinjafmt.py. sqlfmt only reformats the code between the curlies
//! with the optional `jinjafmt` extra (black), which this port does not provide; tags
//! are formatted exactly as sqlfmt formats them without it: a single-line tag gets one
//! space inside each marker and after its verb, and a multiline tag is left unchanged.

use std::sync::OnceLock;

use regex::Regex;

use crate::line::Line;
use crate::node::{Arena, NodeId};
use crate::pyre;
use crate::pystr;

/// A jinja tag split into its parts; `{%- set my_var=4 %}` is
/// `("{%-", "set ", "my_var=4", "%}")`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JinjaTag {
    pub(crate) source_string: String,
    pub(crate) opening_marker: String,
    pub(crate) verb: String,
    pub(crate) code: String,
    pub(crate) closing_marker: String,
}

impl JinjaTag {
    /// Splits a jinja statement or expression into its parts.
    pub(crate) fn from_string(source: &str) -> JinjaTag {
        static VERB: OnceLock<Regex> = OnceLock::new();
        let verb_re = VERB.get_or_init(|| {
            pyre::compile_plain(r"\s*(set|do|for|if|elif|else|test|macro|materialization|call)\s+", "is")
        });
        let chars: Vec<char> = source.chars().collect();
        let opening_len = if chars.get(2) == Some(&'-') { 3 } else { 2 };
        let closing_len = if chars.len() >= 3 && chars[chars.len() - 3] == '-' { 3 } else { 2 };
        let opening_marker: String = chars.iter().take(opening_len).collect();
        let closing_marker: String = chars[chars.len().saturating_sub(closing_len)..].iter().collect();

        let after_opening: String = chars.iter().skip(opening_len).collect();
        let (verb, verb_len) = match verb_re.captures(&after_opening) {
            Some(caps) => {
                let whole = caps.get(0).expect("group 0");
                let verb = caps.get(1).map_or(String::new(), |m| pystr::lower(m.as_str()));
                (verb, pystr::char_len(whole.as_str()))
            }
            None => (String::new(), 0),
        };
        let code_start = opening_len + verb_len;
        let code_end = chars.len().saturating_sub(closing_len);
        let code: String =
            if code_start < code_end { chars[code_start..code_end].iter().collect() } else { String::new() };
        let code = pystr::strip(&code).to_string();
        let verb = if !verb.is_empty() && !code.is_empty() { format!("{verb} ") } else { verb };
        JinjaTag { source_string: source.to_string(), opening_marker, verb, code, closing_marker }
    }

    pub(crate) fn is_multiline_tag(&self) -> bool {
        self.code.contains('\n')
    }

    /// The tag as printed: multiline tags are unchanged.
    pub(crate) fn render(&self) -> String {
        if self.is_multiline_tag() {
            self.source_string.clone()
        } else {
            format!("{} {}{} {}", self.opening_marker, self.verb, self.code, self.closing_marker)
        }
    }
}

/// Formats every jinja tag on lines whose formatting is not disabled.
pub(crate) fn format_line(arena: &mut Arena, line: &Line) {
    if line.formatting_disabled || !line.contains_jinja(arena) {
        return;
    }
    let nodes: Vec<NodeId> = line.nodes.clone();
    for id in nodes {
        if arena.get(id).is_jinja() {
            let tag = JinjaTag::from_string(&arena.get(id).value);
            arena.get_mut(id).value = tag.render();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_tags_into_parts() {
        let tag = JinjaTag::from_string("{%- set my_var=4 %}");
        assert_eq!(tag.opening_marker, "{%-");
        assert_eq!(tag.verb, "set ");
        assert_eq!(tag.code, "my_var=4");
        assert_eq!(tag.closing_marker, "%}");
        assert_eq!(tag.render(), "{%- set my_var=4 %}");
    }

    #[test]
    fn normalises_whitespace_of_single_line_tags() {
        assert_eq!(JinjaTag::from_string("{{foo}}").render(), "{{ foo }}");
        assert_eq!(JinjaTag::from_string("{%  else  -%}").render(), "{% else -%}");
        assert_eq!(JinjaTag::from_string("{{ foo\n}}").render(), "{{ foo }}");
        let multi = "{{ foo(\n  a\n) }}";
        assert_eq!(JinjaTag::from_string(multi).render(), multi);
    }
}
