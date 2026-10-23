//! Compiles sqlfmt's lexing patterns for `fancy_regex`.
//!
//! sqlfmt's grammar is written as backtracking regular expressions. The patterns are
//! kept as written; this module adapts the few constructs whose meaning differs in
//! `fancy_regex` so that a pattern matches exactly the same text:
//!
//! * `\s`/`\S` also cover the information separators U+001C..U+001F;
//! * `\w`/`\W` cover letters, numbers and the underscore (and not combining marks or
//!   other connector punctuation);
//! * `$` outside a character class matches at the end of the text or before a final
//!   newline;
//! * a `{` that does not start a counted repetition is a literal brace.

use fancy_regex::Regex;

/// Backtracking budget per match: large enough that no realistic input reaches it.
const BACKTRACK_LIMIT: usize = 1 << 40;

const WORD: &str = r"\p{L}\p{N}_";
const SPACE: &str = r"\s\x1c-\x1f";

/// Rewrites `pattern` into the `fancy_regex` dialect (see the module docs).
pub(crate) fn translate(pattern: &str) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut i = 0;
    let mut in_class = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            let n = chars[i + 1];
            match (n, in_class) {
                ('s', false) => out.push_str(&format!("[{SPACE}]")),
                ('S', false) => out.push_str(&format!("[^{SPACE}]")),
                ('w', false) => out.push_str(&format!("[{WORD}]")),
                ('W', false) => out.push_str(&format!("[^{WORD}]")),
                ('s', true) => out.push_str(SPACE),
                ('w', true) => out.push_str(WORD),
                _ => {
                    out.push(c);
                    out.push(n);
                }
            }
            i += 2;
            continue;
        }
        if in_class {
            if c == ']' {
                in_class = false;
            }
            if c == '[' {
                out.push_str("\\[");
            } else {
                out.push(c);
            }
            i += 1;
            continue;
        }
        match c {
            '[' => {
                // `[^\S\n]`: whitespace other than a newline
                let rest: String = chars[i..].iter().take(7).collect();
                if rest == "[^\\S\\n]" {
                    out.push_str(&format!("[{SPACE}&&[^\\n]]"));
                    i += 7;
                    continue;
                }
                in_class = true;
                out.push('[');
                i += 1;
                // a leading `^` and a leading `]` belong to the class
                if i < chars.len() && chars[i] == '^' {
                    out.push('^');
                    i += 1;
                }
                if i < chars.len() && chars[i] == ']' {
                    out.push_str("\\]");
                    i += 1;
                }
            }
            '$' => {
                out.push_str(r"(?=\n?\z)");
                i += 1;
            }
            '{' => {
                if counted_repetition(&chars[i..]) {
                    out.push('{');
                } else {
                    out.push_str("\\{");
                }
                i += 1;
            }
            '}' => {
                // a closing brace that belongs to a counted repetition was pushed with it
                if closes_repetition(&out) {
                    out.push('}');
                } else {
                    out.push_str("\\}");
                }
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

fn counted_repetition(chars: &[char]) -> bool {
    // `{n}`, `{n,}`, `{n,m}` (and `{,m}`)
    let mut j = 1;
    let mut digits = 0;
    while j < chars.len() && chars[j].is_ascii_digit() {
        j += 1;
        digits += 1;
    }
    if j < chars.len() && chars[j] == '}' {
        return digits > 0;
    }
    if j < chars.len() && chars[j] == ',' {
        j += 1;
        let mut digits2 = 0;
        while j < chars.len() && chars[j].is_ascii_digit() {
            j += 1;
            digits2 += 1;
        }
        return j < chars.len() && chars[j] == '}' && (digits > 0 || digits2 > 0);
    }
    false
}

fn closes_repetition(out: &str) -> bool {
    // true when the text after the last unescaped `{` is `n`, `n,`, `n,m` or `,m`
    let Some(open) = out.rfind('{') else { return false };
    if open > 0 && out[..open].ends_with('\\') {
        return false;
    }
    let body = &out[open + 1..];
    !body.is_empty() && body.chars().all(|c| c.is_ascii_digit() || c == ',')
}

/// Compiles a pattern with the given inline flags (for example `"i"` or `"is"`).
pub(crate) fn compile_with_flags(pattern: &str, flags: &str) -> Regex {
    let translated = translate(pattern);
    let source = if flags.is_empty() { translated } else { format!("(?{flags}){translated}") };
    fancy_regex::RegexBuilder::new(&source)
        .backtrack_limit(BACKTRACK_LIMIT)
        .build()
        .unwrap_or_else(|e| panic!("invalid built-in pattern {pattern:?}: {e}"))
}

/// Compiles a sqlfmt pattern that must match at a given position (the position
/// itself is passed when matching).
pub(crate) fn compile_anchored(pattern: &str, flags: &str) -> Regex {
    compile_with_flags(&format!(r"\G(?:{pattern})"), flags)
}

/// Compiles a sqlfmt pattern without lookaround or backreferences for the `regex`
/// engine, whose matching cannot fail. The pattern is anchored at the start of the
/// text; a `$` may only end it.
pub(crate) fn compile_plain(pattern: &str, flags: &str) -> regex::Regex {
    let translated = translate(pattern);
    let translated = match translated.strip_suffix(r"(?=\n?\z)") {
        Some(body) => format!(r"{body}\n?\z"),
        None => translated,
    };
    let source = format!("(?{flags}u)^(?:{translated})");
    regex::Regex::new(&source).unwrap_or_else(|e| panic!("invalid built-in pattern {pattern:?}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_python_constructs() {
        assert_eq!(translate(r"a$"), r"a(?=\n?\z)");
        assert_eq!(translate(r"[^\S\n]*"), r"[\s\x1c-\x1f&&[^\n]]*");
        assert_eq!(translate(r"~{1,2}"), r"~{1,2}");
        assert_eq!(translate(r"{{.*?}}"), r"\{\{.*?\}\}");
        assert_eq!(translate(r"\$\w+"), r"\$[\p{L}\p{N}_]+");
        assert_eq!(translate(r"[{}%\-\s]"), r"[{}%\-\s\x1c-\x1f]");
    }

    #[test]
    fn dollar_matches_before_final_newline() {
        let re = compile_anchored(r"(end)(\W|$)", "is");
        assert!(re.is_match("end\n").unwrap());
        assert!(re.is_match("end").unwrap());
    }

    #[test]
    fn word_excludes_combining_marks() {
        let re = compile_anchored(r"\w+", "is");
        let m = re.find("cafe\u{301}").unwrap().unwrap();
        assert_eq!(m.as_str(), "cafe");
    }
}
