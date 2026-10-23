//! The jinja tokenizer that decides where a jinja expression ends.
//!
//! A `{{ ... }}` tag ends at the first `}}` after which the jinja tokenizer's last
//! token is the end of the expression: braces inside the expression must be balanced
//! first, as in `{{ {'a': {'b': 1}} }}`. This module reproduces the default jinja
//! tokenizer closely enough to report that last token, and the syntax errors that make
//! a tag impossible to tokenize.

use std::sync::OnceLock;

use fancy_regex::Regex;

use crate::error::Fatal;
use crate::pyre;

struct Patterns {
    root: Regex,
    root_rest: Regex,
    whitespace: Regex,
    float: Regex,
    integer: Regex,
    name: Regex,
    string: Regex,
    operator: Regex,
    variable_end: Regex,
    block_end: Regex,
    comment: Regex,
    raw: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| {
        let root_parts = concat!(
            r"(?P<raw_begin>\{%(\-|\+|)\s*raw\s*(?:\-%\}\s*|%\}))",
            r"|(?P<variable_begin>\{\{(\-|\+|))",
            r"|(?P<comment_begin>\{#(\-|\+|))",
            r"|(?P<block_begin>\{%(\-|\+|))"
        );
        let operators = [
            "//", "**", "==", "!=", ">=", "<=", "+", "-", "/", "*", "%", "~", "[", "]", "(", ")", "{", "}", ">", "<",
            "=", ".", ":", "|", ",", ";",
        ];
        let escaped: Vec<String> = operators.iter().map(|o| fancy_regex::escape(o).into_owned()).collect();
        Patterns {
            root: pyre::compile_anchored(&format!(r"(.*?)(?:{root_parts})"), "ms"),
            root_rest: pyre::compile_anchored(r".+", "ms"),
            whitespace: pyre::compile_anchored(r"\s+", ""),
            float: pyre::compile_anchored(r"(?<!\.)(\d+_)*\d+((\.(\d+_)*\d+)?e[+\-]?(\d+_)*\d+|\.(\d+_)*\d+)", "i"),
            integer: pyre::compile_anchored(r"(0b(_?[0-1])+|0o(_?[0-7])+|0x(_?[\da-f])+|[1-9](_?\d)*|0(_?0)*)", "i"),
            name: pyre::compile_anchored(r"[\p{L}\p{N}_\p{Mn}\p{Mc}\p{Pc}\u{b7}\u{2118}\u{212e}\u{309b}\u{309c}]+", ""),
            string: pyre::compile_anchored(r#"('([^'\\]*(?:\\.[^'\\]*)*)'|"([^"\\]*(?:\\.[^"\\]*)*)")"#, "s"),
            operator: pyre::compile_anchored(&format!("({})", escaped.join("|")), ""),
            variable_end: pyre::compile_anchored(r"\-\}\}\s*|\}\}", "ms"),
            block_end: pyre::compile_anchored(r"(?:\+%\}|\-%\}\s*|%\})", "ms"),
            comment: pyre::compile_anchored(r"(.*?)((?:\+#\}|\-#\}\s*|#\}))", "ms"),
            raw: pyre::compile_anchored(r"(.*?)((?:\{%(\-|\+|))\s*endraw\s*(?:\+%\}|\-%\}\s*|%\}))", "ms"),
        }
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Root,
    Variable,
    Block,
    Comment,
    Raw,
}

fn syntax_error(message: String) -> Fatal {
    Fatal::new(format!("jinja2.exceptions.TemplateSyntaxError: {message}"))
}

fn regex_error(e: fancy_regex::Error) -> Fatal {
    Fatal::new(format!("regex error: {e}"))
}

/// Tokenizes `text` as a jinja template and returns the type of its last token
/// ("variable_end", "rbrace", "data", ...). Fails like the jinja tokenizer does on text
/// it cannot tokenize.
pub(crate) fn final_token_type(text: &str) -> Result<&'static str, Fatal> {
    let p = patterns();
    // newlines are normalised and a single trailing newline is dropped
    let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
    let source = normalised.strip_suffix('\n').unwrap_or(&normalised);
    let mut pos = 0;
    let mut stack = vec![State::Root];
    let mut balancing: Vec<char> = Vec::new();
    let mut last: Option<&'static str> = None;
    loop {
        let state = *stack.last().expect("state stack is never empty");
        if pos >= source.len() && state == State::Root {
            break;
        }
        match state {
            State::Root => {
                if let Some(caps) = p.root.captures_from_pos(source, pos).map_err(regex_error)? {
                    // any data before the tag is followed by the tag itself, so the tag is
                    // the last token so far
                    let names = ["raw_begin", "variable_begin", "comment_begin", "block_begin"];
                    let name = names.iter().copied().find(|n| caps.name(n).is_some()).expect("a root tag matched");
                    last = Some(name);
                    stack.push(match name {
                        "raw_begin" => State::Raw,
                        "variable_begin" => State::Variable,
                        "comment_begin" => State::Comment,
                        _ => State::Block,
                    });
                    pos = caps.get(0).expect("group 0").end();
                } else if let Some(m) = p.root_rest.find_from_pos(source, pos).map_err(regex_error)? {
                    last = Some("data");
                    pos = m.end();
                } else {
                    break;
                }
            }
            State::Comment => {
                if let Some(caps) = p.comment.captures_from_pos(source, pos).map_err(regex_error)? {
                    last = Some("comment_end");
                    stack.pop();
                    pos = caps.get(0).expect("group 0").end();
                } else if pos < source.len() {
                    return Err(syntax_error("Missing end of comment tag".to_string()));
                } else {
                    break;
                }
            }
            State::Raw => {
                if let Some(caps) = p.raw.captures_from_pos(source, pos).map_err(regex_error)? {
                    last = Some("raw_end");
                    stack.pop();
                    pos = caps.get(0).expect("group 0").end();
                } else if pos < source.len() {
                    return Err(syntax_error("Missing end of raw directive".to_string()));
                } else {
                    break;
                }
            }
            State::Variable | State::Block => {
                let end_re = if state == State::Variable { &p.variable_end } else { &p.block_end };
                if balancing.is_empty() {
                    if let Some(m) = end_re.find_from_pos(source, pos).map_err(regex_error)? {
                        last = Some(if state == State::Variable { "variable_end" } else { "block_end" });
                        stack.pop();
                        pos = m.end();
                        continue;
                    }
                }
                if let Some(m) = p.whitespace.find_from_pos(source, pos).map_err(regex_error)? {
                    last = Some("whitespace");
                    pos = m.end();
                } else if let Some(m) = p.float.find_from_pos(source, pos).map_err(regex_error)? {
                    last = Some("float");
                    pos = m.end();
                } else if let Some(m) = p.integer.find_from_pos(source, pos).map_err(regex_error)? {
                    last = Some("integer");
                    pos = m.end();
                } else if let Some(m) = p.name.find_from_pos(source, pos).map_err(regex_error)? {
                    last = Some("name");
                    pos = m.end();
                } else if let Some(m) = p.string.find_from_pos(source, pos).map_err(regex_error)? {
                    last = Some("string");
                    pos = m.end();
                } else if let Some(m) = p.operator.find_from_pos(source, pos).map_err(regex_error)? {
                    let op = m.as_str();
                    match op {
                        "{" => balancing.push('}'),
                        "(" => balancing.push(')'),
                        "[" => balancing.push(']'),
                        "}" | ")" | "]" => {
                            let Some(expected) = balancing.pop() else {
                                return Err(syntax_error(format!("unexpected '{op}'")));
                            };
                            if !op.starts_with(expected) {
                                return Err(syntax_error(format!("unexpected '{op}', expected '{expected}'")));
                            }
                        }
                        _ => {}
                    }
                    last = Some(operator_name(op));
                    pos = m.end();
                } else if pos >= source.len() {
                    break;
                } else {
                    let c = source[pos..].chars().next().expect("a character at pos");
                    return Err(syntax_error(format!("unexpected char {:?} at {}", c, source[..pos].chars().count())));
                }
            }
        }
    }
    Ok(last.unwrap_or("data"))
}

fn operator_name(op: &str) -> &'static str {
    match op {
        "+" => "add",
        "-" => "sub",
        "/" => "div",
        "//" => "floordiv",
        "*" => "mul",
        "%" => "mod",
        "**" => "pow",
        "~" => "tilde",
        "[" => "lbracket",
        "]" => "rbracket",
        "(" => "lparen",
        ")" => "rparen",
        "{" => "lbrace",
        "}" => "rbrace",
        "==" => "eq",
        "!=" => "ne",
        ">" => "gt",
        ">=" => "gteq",
        "<" => "lt",
        "<=" => "lteq",
        "=" => "assign",
        "." => "dot",
        ":" => "colon",
        "|" => "pipe",
        "," => "comma",
        _ => "semicolon",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_expression_ends_with_variable_end() {
        assert_eq!(final_token_type("{{ foo }}").unwrap(), "variable_end");
        assert_eq!(final_token_type("{{ {'a': {'b': 1}}").unwrap(), "rbrace");
        assert_eq!(final_token_type("{{ {'a': {'b': 1}} }}").unwrap(), "variable_end");
        assert_eq!(final_token_type("{{- foo -}}").unwrap(), "variable_end");
    }

    #[test]
    fn untokenizable_expressions_fail() {
        assert!(final_token_type("{{ foo) }}").is_err());
        assert!(final_token_type("{{ a & b }}").is_err());
        assert!(final_token_type("{{ \"}}").is_err());
    }
}
