//! String helpers with the semantics sqlfmt relies on.
//!
//! sqlfmt measures, slices, splits and strips text by Unicode code point and treats
//! the same set of characters as whitespace and line breaks everywhere. These helpers
//! keep that behaviour in one place, so lengths and positions never depend on the
//! UTF-8 encoding of the text.

/// Returns true for the characters sqlfmt treats as whitespace: the Unicode
/// `White_Space` characters plus the four information separators U+001C..U+001F.
pub(crate) fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Number of code points in `s`.
pub(crate) fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Code-point index of the byte offset `byte` in `s`.
pub(crate) fn char_index(s: &str, byte: usize) -> usize {
    s[..byte].chars().count()
}

/// Byte offset of the code-point index `idx` in `s` (clamped to the end).
pub(crate) fn byte_index(s: &str, idx: usize) -> usize {
    s.char_indices().nth(idx).map_or(s.len(), |(b, _)| b)
}

/// The substring between two code-point indices, clamped like a slice `s[start:end]`.
pub(crate) fn char_slice(s: &str, start: usize, end: usize) -> &str {
    let b0 = byte_index(s, start);
    let b1 = byte_index(s, end.max(start));
    &s[b0..b1]
}

/// Strips leading and trailing whitespace.
pub(crate) fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// Strips trailing whitespace.
pub(crate) fn rstrip(s: &str) -> &str {
    s.trim_end_matches(is_space)
}

/// Splits on runs of whitespace, dropping empty parts.
pub(crate) fn split_whitespace(s: &str) -> Vec<&str> {
    s.split(is_space).filter(|p| !p.is_empty()).collect()
}

/// Joins the whitespace-separated words of `s` with single spaces.
pub(crate) fn collapse_whitespace(s: &str) -> String {
    split_whitespace(s).join(" ")
}

/// Removes every whitespace character from `s`.
pub(crate) fn remove_whitespace(s: &str) -> String {
    s.chars().filter(|c| !is_space(*c)).collect()
}

fn is_line_break(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// Splits `s` into lines at every line boundary sqlfmt recognises (`\r\n` counts as
/// one boundary). With `keepends`, each line keeps its terminator.
pub(crate) fn splitlines(s: &str, keepends: bool) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut iter = s.char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        if is_line_break(c) {
            let mut end = i + c.len_utf8();
            if c == '\r' {
                if let Some(&(_, '\n')) = iter.peek() {
                    iter.next();
                    end += 1;
                }
            }
            lines.push(if keepends { &s[start..end] } else { &s[start..i] });
            start = end;
        }
    }
    if start < s.len() {
        lines.push(&s[start..]);
    }
    lines
}

/// Lowercases `s` with full Unicode case mapping.
pub(crate) fn lower(s: &str) -> String {
    s.to_lowercase()
}

/// `repr()` of a text value: single quotes unless the text contains a single quote
/// and no double quote, with backslash escapes for control characters.
pub(crate) fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if is_printable(c) => out.push(c),
            c => {
                let v = c as u32;
                if v < 0x100 {
                    out.push_str(&format!("\\x{v:02x}"));
                } else if v < 0x10000 {
                    out.push_str(&format!("\\u{v:04x}"));
                } else {
                    out.push_str(&format!("\\U{v:08x}"));
                }
            }
        }
    }
    out.push(quote);
    out
}

/// `repr()` of a byte string (`b'...'`).
pub(crate) fn repr_bytes(b: &[u8]) -> String {
    let quote = if b.contains(&b'\'') && !b.contains(&b'"') { b'"' } else { b'\'' };
    let mut out = String::with_capacity(b.len() + 3);
    out.push('b');
    out.push(quote as char);
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c as char);
            }
            0x20..=0x7e => out.push(c as char),
            c => out.push_str(&format!("\\x{c:02x}")),
        }
    }
    out.push(quote as char);
    out
}

/// Printable characters for `repr()`: everything except the categories Cc, Cf, Cs,
/// Co, Cn, Zl, Zp and Zs, with the ASCII space as the one printable separator.
fn is_printable(c: char) -> bool {
    if c == ' ' {
        return true;
    }
    if c.is_control() {
        return false;
    }
    let v = c as u32;
    // separators (Zs, Zl, Zp)
    if matches!(v, 0xa0 | 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000) {
        return false;
    }
    // format characters (Cf) that commonly appear in text
    if matches!(
        v,
        0xad | 0x600..=0x605
            | 0x61c
            | 0x6dd
            | 0x70f
            | 0x180e
            | 0x200b..=0x200f
            | 0x202a..=0x202e
            | 0x2060..=0x2064
            | 0x2066..=0x206f
            | 0xfeff
            | 0xfff9..=0xfffb
    ) {
        return false;
    }
    // private use and noncharacters
    if matches!(v, 0xe000..=0xf8ff | 0xf0000..=0x10ffff | 0xfdd0..=0xfdef) || (v & 0xfffe) == 0xfffe {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_includes_information_separators() {
        assert!(is_space('\u{1c}'));
        assert!(is_space('\u{a0}'));
        assert!(!is_space('a'));
        assert_eq!(split_whitespace(" a\u{1f}b  c "), vec!["a", "b", "c"]);
    }

    #[test]
    fn splitlines_matches_every_boundary() {
        assert_eq!(splitlines("a\r\nb\rc\x0cd", true), vec!["a\r\n", "b\r", "c\x0c", "d"]);
        assert_eq!(splitlines("a\n\n", false), vec!["a", ""]);
        assert!(splitlines("", true).is_empty());
    }

    #[test]
    fn char_slicing_counts_code_points() {
        assert_eq!(char_slice("héllo", 1, 3), "él");
        assert_eq!(char_index("héllo", 3), 2);
        assert_eq!(char_slice("ab", 1, 50), "b");
    }

    #[test]
    fn reprs() {
        assert_eq!(repr_str("it's"), "\"it's\"");
        assert_eq!(repr_bytes(b"select \xff\n"), "b'select \\xff\\n'");
    }
}
