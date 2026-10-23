//! Text encodings for reading and writing SQL files.
//!
//! Files are read with the encoding named by `--encoding` (by its usual names and
//! aliases), line endings are normalised to `\n` on reading, and files are written back
//! with the same encoding.

/// The encodings sqlfmt can read and write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Codec {
    Utf8,
    Utf8Sig,
    Latin1,
    Ascii,
    Cp1252,
    Utf16,
    Utf16Le,
    Utf16Be,
    Utf32,
    Utf32Le,
    Utf32Be,
}

/// Normalises an encoding name: ASCII letters lowercased, runs of other punctuation
/// and spaces replaced by a single underscore.
fn normalize(name: &str) -> String {
    let lowered: String = name.to_lowercase().replace([' ', '-'], "_");
    let mut out = String::new();
    let mut punct = false;
    for c in lowered.chars() {
        if c.is_alphanumeric() || c == '.' {
            if punct && !out.is_empty() {
                out.push('_');
            }
            if c.is_ascii() {
                out.push(c);
            }
            punct = false;
        } else {
            punct = true;
        }
    }
    out
}

impl Codec {
    /// Looks up a codec by name or alias.
    pub(crate) fn lookup(name: &str) -> Option<Codec> {
        let norm = normalize(name);
        let key = norm.as_str();
        let codec = match key {
            "utf_8" | "u8" | "utf" | "utf8" | "utf8_ucs2" | "utf8_ucs4" | "cp65001" => Codec::Utf8,
            "utf_8_sig" => Codec::Utf8Sig,
            "latin_1" | "8859" | "cp819" | "csisolatin1" | "ibm819" | "iso8859" | "iso8859_1" | "iso_8859_1"
            | "iso_8859_1_1987" | "iso_ir_100" | "l1" | "latin" | "latin1" => Codec::Latin1,
            "ascii" | "646" | "ansi_x3.4_1968" | "ansi_x3.4_1986" | "ansi_x3_4_1968" | "cp367" | "csascii"
            | "ibm367" | "iso646_us" | "iso_646.irv_1991" | "iso_ir_6" | "us" | "us_ascii" => Codec::Ascii,
            "cp1252" | "1252" | "windows_1252" => Codec::Cp1252,
            "utf_16" | "u16" | "utf16" => Codec::Utf16,
            "utf_16_le" | "utf_16le" | "unicodelittleunmarked" => Codec::Utf16Le,
            "utf_16_be" | "utf_16be" | "unicodebigunmarked" => Codec::Utf16Be,
            "utf_32" | "u32" | "utf32" => Codec::Utf32,
            "utf_32_le" | "utf_32le" => Codec::Utf32Le,
            "utf_32_be" | "utf_32be" => Codec::Utf32Be,
            _ => return None,
        };
        Some(codec)
    }

    /// Decodes bytes; on failure returns the position of the first undecodable byte
    /// and the reason.
    pub(crate) fn decode(self, bytes: &[u8]) -> Result<String, (usize, &'static str)> {
        match self {
            Codec::Utf8 => decode_utf8(bytes),
            Codec::Utf8Sig => decode_utf8(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes)),
            Codec::Latin1 => Ok(bytes.iter().map(|&b| b as char).collect()),
            Codec::Ascii => match bytes.iter().position(|&b| b >= 0x80) {
                Some(p) => Err((p, "ordinal not in range(128)")),
                None => Ok(bytes.iter().map(|&b| b as char).collect()),
            },
            Codec::Cp1252 => bytes
                .iter()
                .enumerate()
                .map(|(i, &b)| cp1252_char(b).ok_or((i, "character maps to <undefined>")))
                .collect(),
            Codec::Utf16 => {
                if let Some(rest) = bytes.strip_prefix(b"\xff\xfe") {
                    decode_utf16(rest, false)
                } else if let Some(rest) = bytes.strip_prefix(b"\xfe\xff") {
                    decode_utf16(rest, true)
                } else if bytes.len() < 2 {
                    decode_utf16(bytes, false)
                } else {
                    Err((0, "UTF-16 stream does not start with BOM"))
                }
            }
            Codec::Utf16Le => decode_utf16(bytes, false),
            Codec::Utf16Be => decode_utf16(bytes, true),
            Codec::Utf32 => {
                if let Some(rest) = bytes.strip_prefix(b"\xff\xfe\x00\x00") {
                    decode_utf32(rest, false)
                } else if let Some(rest) = bytes.strip_prefix(b"\x00\x00\xfe\xff") {
                    decode_utf32(rest, true)
                } else if bytes.len() < 4 {
                    decode_utf32(bytes, false)
                } else {
                    Err((0, "UTF-32 stream does not start with BOM"))
                }
            }
            Codec::Utf32Le => decode_utf32(bytes, false),
            Codec::Utf32Be => decode_utf32(bytes, true),
        }
    }

    /// Encodes text; fails for characters the encoding cannot represent.
    pub(crate) fn encode(self, text: &str) -> Result<Vec<u8>, char> {
        match self {
            Codec::Utf8 => Ok(text.as_bytes().to_vec()),
            Codec::Utf8Sig => {
                let mut out = b"\xef\xbb\xbf".to_vec();
                out.extend_from_slice(text.as_bytes());
                Ok(out)
            }
            Codec::Latin1 => text.chars().map(|c| u8::try_from(u32::from(c)).map_err(|_| c)).collect(),
            Codec::Ascii => text.chars().map(|c| if c.is_ascii() { Ok(c as u8) } else { Err(c) }).collect(),
            Codec::Cp1252 => text.chars().map(|c| cp1252_byte(c).ok_or(c)).collect(),
            Codec::Utf16 => {
                let mut out = vec![0xff, 0xfe];
                out.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
                Ok(out)
            }
            Codec::Utf16Le => Ok(text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()),
            Codec::Utf16Be => Ok(text.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()),
            Codec::Utf32 => {
                let mut out = vec![0xff, 0xfe, 0, 0];
                out.extend(text.chars().flat_map(|c| u32::from(c).to_le_bytes()));
                Ok(out)
            }
            Codec::Utf32Le => Ok(text.chars().flat_map(|c| u32::from(c).to_le_bytes()).collect()),
            Codec::Utf32Be => Ok(text.chars().flat_map(|c| u32::from(c).to_be_bytes()).collect()),
        }
    }
}

fn decode_utf8(bytes: &[u8]) -> Result<String, (usize, &'static str)> {
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok(s.to_string()),
        Err(e) => {
            let pos = e.valid_up_to();
            let reason = match e.error_len() {
                None => "unexpected end of data",
                Some(_) => {
                    let b = bytes[pos];
                    if (0x80..0xc2).contains(&b) || b > 0xf4 {
                        "invalid start byte"
                    } else {
                        "invalid continuation byte"
                    }
                }
            };
            Err((pos, reason))
        }
    }
}

fn decode_utf16(bytes: &[u8], big_endian: bool) -> Result<String, (usize, &'static str)> {
    let units: Vec<u16> = bytes
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| if big_endian { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) })
        .collect();
    let mut out = String::new();
    for (i, r) in char::decode_utf16(units.iter().copied()).enumerate() {
        match r {
            Ok(c) => out.push(c),
            Err(_) => return Err((i * 2, "illegal encoding")),
        }
    }
    if bytes.len() % 2 == 1 {
        return Err((bytes.len() - 1, "truncated data"));
    }
    Ok(out)
}

fn decode_utf32(bytes: &[u8], big_endian: bool) -> Result<String, (usize, &'static str)> {
    let mut out = String::new();
    for (i, c) in bytes.chunks(4).enumerate() {
        if c.len() < 4 {
            return Err((i * 4, "truncated data"));
        }
        let v = if big_endian {
            u32::from_be_bytes([c[0], c[1], c[2], c[3]])
        } else {
            u32::from_le_bytes([c[0], c[1], c[2], c[3]])
        };
        match char::from_u32(v) {
            Some(ch) => out.push(ch),
            None => return Err((i * 4, "code point not in range(0x110000)")),
        }
    }
    Ok(out)
}

const CP1252_HIGH: [Option<char>; 32] = [
    Some('\u{20ac}'),
    None,
    Some('\u{201a}'),
    Some('\u{0192}'),
    Some('\u{201e}'),
    Some('\u{2026}'),
    Some('\u{2020}'),
    Some('\u{2021}'),
    Some('\u{02c6}'),
    Some('\u{2030}'),
    Some('\u{0160}'),
    Some('\u{2039}'),
    Some('\u{0152}'),
    None,
    Some('\u{017d}'),
    None,
    None,
    Some('\u{2018}'),
    Some('\u{2019}'),
    Some('\u{201c}'),
    Some('\u{201d}'),
    Some('\u{2022}'),
    Some('\u{2013}'),
    Some('\u{2014}'),
    Some('\u{02dc}'),
    Some('\u{2122}'),
    Some('\u{0161}'),
    Some('\u{203a}'),
    Some('\u{0153}'),
    None,
    Some('\u{017e}'),
    Some('\u{0178}'),
];

fn cp1252_char(b: u8) -> Option<char> {
    if (0x80..0xa0).contains(&b) {
        CP1252_HIGH[(b - 0x80) as usize]
    } else {
        Some(b as char)
    }
}

fn cp1252_byte(c: char) -> Option<u8> {
    let v = u32::from(c);
    if v < 0x80 || (0xa0..0x100).contains(&v) {
        return Some(v as u8);
    }
    CP1252_HIGH.iter().position(|&x| x == Some(c)).map(|i| 0x80 + i as u8)
}

/// Text-mode reading: `\r\n` and lone `\r` become `\n`.
pub(crate) fn translate_newlines(text: &str) -> String {
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_aliases() {
        assert_eq!(Codec::lookup("UTF-8"), Some(Codec::Utf8));
        assert_eq!(Codec::lookup("latin-1"), Some(Codec::Latin1));
        assert_eq!(Codec::lookup("utf_8_sig"), Some(Codec::Utf8Sig));
        assert_eq!(Codec::lookup("bogus"), None);
    }

    #[test]
    fn round_trips() {
        let text = "select 'é' from t\n";
        for codec in [Codec::Utf8, Codec::Latin1, Codec::Cp1252, Codec::Utf16, Codec::Utf16Be, Codec::Utf32] {
            let bytes = codec.encode(text).unwrap();
            assert_eq!(codec.decode(&bytes).unwrap(), text, "{codec:?}");
        }
        assert!(Codec::Ascii.encode(text).is_err());
        assert_eq!(Codec::Utf8.decode(b"a\xff"), Err((1, "invalid start byte")));
    }

    #[test]
    fn newlines_are_translated() {
        assert_eq!(translate_newlines("a\r\nb\rc\n"), "a\nb\nc\n");
    }
}
