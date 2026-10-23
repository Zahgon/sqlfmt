//! Integer parsing and case folding for command-line values.
//!
//! Integers accept surrounding whitespace, a sign, decimal digits of any script and
//! single underscores between digits ("1_000"). Case folding maps a value to the form
//! used for case-insensitive comparison.

use crate::pystr;

/// The first code point of each run of ten decimal digits (Unicode 15.0, category Nd).
const DIGIT_ZEROS: [u32; 68] = [
    0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66, 0xde6, 0xe50, 0xed0,
    0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90, 0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620,
    0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0, 0xff10, 0x104a0, 0x10d30, 0x11066, 0x110f0, 0x11136, 0x111d0,
    0x112f0, 0x11450, 0x114d0, 0x11650, 0x116c0, 0x11730, 0x118e0, 0x11950, 0x11c50, 0x11d50, 0x11da0, 0x11f50,
    0x16a60, 0x16ac0, 0x16b50, 0x1d7ce, 0x1d7d8, 0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0, 0x1e950,
    0x1fbf0,
];

/// The value of a decimal digit of any script.
pub(crate) fn decimal_value(c: char) -> Option<u32> {
    let v = u32::from(c);
    DIGIT_ZEROS.iter().find(|&&z| (z..z + 10).contains(&v)).map(|z| v - z)
}

/// Parses a base-10 integer; values beyond the 64-bit range saturate.
pub(crate) fn parse(text: &str) -> Option<i64> {
    let s = pystr::strip(text);
    let (negative, digits) = match s.chars().next()? {
        '-' => (true, &s[1..]),
        '+' => (false, &s[1..]),
        _ => (false, s),
    };
    let mut value: i128 = 0;
    let mut previous_underscore = true;
    let mut any_digit = false;
    for c in digits.chars() {
        if c == '_' {
            if previous_underscore {
                return None;
            }
            previous_underscore = true;
            continue;
        }
        let d = decimal_value(c)?;
        value = (value * 10 + i128::from(d)).min(i128::from(i64::MAX) + 1);
        previous_underscore = false;
        any_digit = true;
    }
    if !any_digit || previous_underscore {
        return None;
    }
    let value = if negative { -value } else { value };
    Some(value.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64)
}

/// Case-folds text for case-insensitive comparison.
pub(crate) fn casefold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\u{df}' => out.push_str("ss"),
            '\u{17f}' => out.push('s'),
            '\u{212a}' => out.push('k'),
            _ => out.extend(c.to_lowercase()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_like_the_command_line() {
        assert_eq!(parse(" 80 "), Some(80));
        assert_eq!(parse("+1_000"), Some(1000));
        assert_eq!(parse("-5"), Some(-5));
        assert_eq!(parse("\u{668}\u{660}"), Some(80));
        assert_eq!(parse("1__0"), None);
        assert_eq!(parse("_1"), None);
        assert_eq!(parse("1e3"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn folds_case() {
        assert_eq!(casefold("ClickHouse"), "clickhouse");
        assert_eq!(casefold("\u{212a}"), "k");
    }
}
