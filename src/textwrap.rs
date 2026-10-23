//! Paragraph wrapping for the command-line help.
//!
//! Text is split into chunks (runs of whitespace, words, and the parts of hyphenated
//! words), then chunks are packed greedily into lines of at most `width` characters;
//! words longer than a line are broken.

use std::sync::OnceLock;

use fancy_regex::Regex;

use crate::pyre;

fn wordsep() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let ws = r"[\t\n\x0b\x0c\r ]";
        let nws = r"[^\t\n\x0b\x0c\r ]";
        let wp = r#"[\w!"'&.,?]"#;
        let lt = r"[^\d\W]";
        let pattern = format!(
            r"({ws}+|(?<={wp})-{{2,}}(?=\w)|{nws}+?(?:-(?:(?<={lt}{{2}}-)|(?<={lt}-{lt}-))(?={lt}-?{lt})|(?={ws}|\z)|(?<={wp})(?=-{{2,}}\w)))"
        );
        pyre::compile_with_flags(&pattern, "")
    })
}

fn is_blank(s: &str) -> bool {
    s.chars().all(|c| matches!(c, '\t' | '\n' | '\x0b' | '\x0c' | '\r' | ' '))
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Splits text into wrappable chunks.
fn split_chunks(text: &str) -> Vec<String> {
    let re = wordsep();
    let mut chunks = Vec::new();
    let mut last = 0;
    let mut pos = 0;
    while pos <= text.len() {
        let Ok(Some(m)) = re.find_from_pos(text, pos) else { break };
        if m.end() == m.start() {
            pos = m.end() + text[m.end()..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        if m.start() > last {
            chunks.push(text[last..m.start()].to_string());
        }
        chunks.push(m.as_str().to_string());
        last = m.end();
        pos = m.end();
    }
    if last < text.len() {
        chunks.push(text[last..].to_string());
    }
    chunks.retain(|c| !c.is_empty());
    chunks
}

/// A text wrapper with an indent for the first line and one for the others.
#[derive(Debug, Clone)]
pub(crate) struct TextWrapper {
    pub(crate) width: usize,
    pub(crate) initial_indent: String,
    pub(crate) subsequent_indent: String,
}

impl TextWrapper {
    pub(crate) fn new(width: usize, initial_indent: &str, subsequent_indent: &str) -> TextWrapper {
        TextWrapper {
            width,
            initial_indent: initial_indent.to_string(),
            subsequent_indent: subsequent_indent.to_string(),
        }
    }

    /// The wrapped lines of `text` (tabs expanded to 8 columns).
    pub(crate) fn wrap(&self, text: &str) -> Vec<String> {
        let text = expand_tabs(text);
        let mut chunks = split_chunks(&text);
        chunks.reverse();
        let mut lines: Vec<String> = Vec::new();
        while !chunks.is_empty() {
            let mut cur_line: Vec<String> = Vec::new();
            let mut cur_len = 0;
            let indent = if lines.is_empty() { &self.initial_indent } else { &self.subsequent_indent };
            let width = self.width.saturating_sub(char_len(indent));
            if chunks.last().is_some_and(|c| is_blank(c)) && !lines.is_empty() {
                chunks.pop();
            }
            while let Some(chunk) = chunks.last() {
                let n = char_len(chunk);
                if cur_len + n <= width {
                    cur_len += n;
                    cur_line.push(chunks.pop().expect("a chunk"));
                } else {
                    break;
                }
            }
            if chunks.last().is_some_and(|c| char_len(c) > width) {
                let space_left = width.saturating_sub(cur_len).max(1);
                let last = chunks.pop().expect("a chunk");
                let cut: String = last.chars().take(space_left).collect();
                let rest: String = last.chars().skip(space_left).collect();
                cur_line.push(cut);
                chunks.push(rest);
            }
            if cur_line.last().is_some_and(|c| is_blank(c)) {
                cur_line.pop();
            }
            if !cur_line.is_empty() {
                lines.push(format!("{indent}{}", cur_line.concat()));
            }
        }
        lines
    }

    pub(crate) fn fill(&self, text: &str) -> String {
        self.wrap(text).join("\n")
    }

    /// Indents every line of `text` without re-wrapping it.
    pub(crate) fn indent_only(&self, text: &str) -> String {
        text.lines()
            .enumerate()
            .map(|(i, line)| format!("{}{line}", if i == 0 { &self.initial_indent } else { &self.subsequent_indent }))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn expand_tabs(text: &str) -> String {
    let mut out = String::new();
    let mut col = 0;
    for c in text.chars() {
        match c {
            '\t' => {
                let n = 8 - col % 8;
                out.push_str(&" ".repeat(n));
                col += n;
            }
            '\n' | '\r' => {
                out.push(c);
                col = 0;
            }
            _ => {
                out.push(c);
                col += 1;
            }
        }
    }
    out
}

/// Wraps text to `width`; with `preserve_paragraphs`, blank lines separate paragraphs
/// that are wrapped separately (keeping each paragraph's own indentation), and a
/// paragraph whose first line is `\b` is kept as written.
pub(crate) fn wrap_text(
    text: &str,
    width: usize,
    initial_indent: &str,
    subsequent_indent: &str,
    preserve_paragraphs: bool,
) -> String {
    let text = expand_tabs(text);
    let wrapper = TextWrapper::new(width, initial_indent, subsequent_indent);
    if !preserve_paragraphs {
        return wrapper.fill(&text);
    }
    let mut paragraphs: Vec<(usize, bool, String)> = Vec::new();
    let mut buf: Vec<String> = Vec::new();
    let mut indent: Option<usize> = None;
    let flush = |buf: &mut Vec<String>, indent: Option<usize>, paragraphs: &mut Vec<(usize, bool, String)>| {
        if buf.is_empty() {
            return;
        }
        if buf[0].trim() == "\u{8}" {
            paragraphs.push((indent.unwrap_or(0), true, buf[1..].join("\n")));
        } else {
            paragraphs.push((indent.unwrap_or(0), false, buf.join(" ")));
        }
        buf.clear();
    };
    for line in text.lines() {
        if line.is_empty() {
            flush(&mut buf, indent, &mut paragraphs);
            indent = None;
        } else {
            let mut line = line.to_string();
            if indent.is_none() {
                let stripped = line.trim_start().to_string();
                indent = Some(char_len(&line) - char_len(&stripped));
                line = stripped;
            }
            buf.push(line);
        }
    }
    flush(&mut buf, indent, &mut paragraphs);
    paragraphs
        .into_iter()
        .map(|(indent, raw, text)| {
            let extra = " ".repeat(indent);
            let w =
                TextWrapper::new(width, &format!("{initial_indent}{extra}"), &format!("{subsequent_indent}{extra}"));
            if raw {
                w.indent_only(&text)
            } else {
                w.fill(&text)
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_words_and_hyphens() {
        assert_eq!(split_chunks("re-processes the 15-20%"), vec!["re-", "processes", " ", "the", " ", "15-20%"]);
    }

    #[test]
    fn wraps_greedily() {
        let w = TextWrapper::new(10, "", "");
        assert_eq!(w.wrap("aaa bbb ccc ddd"), vec!["aaa bbb", "ccc ddd"]);
        assert_eq!(w.wrap("abcdefghijklmno"), vec!["abcdefghij", "klmno"]);
    }
}
