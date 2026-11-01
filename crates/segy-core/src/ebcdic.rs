use crate::error::{Result, SegyError};

/// Length in bytes of a standard SEG-Y textual file header.
pub const TEXTUAL_HEADER_LEN: usize = 3200;
/// Number of 80-byte card lines in a textual file header.
pub const TEXTUAL_HEADER_LINES: usize = 40;
/// Number of columns per card line in a textual file header.
pub const TEXTUAL_HEADER_COLS: usize = 80;

/// CP037 (EBCDIC US/Canada) to printable ASCII lookup table.
/// Non-printable control bytes map to `' '` (`0x20`).
#[rustfmt::skip]
const CP037_TO_ASCII: [u8; 256] = [
    // 0x00..0x0F
    b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ',
    // 0x10..0x1F
    b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ',
    // 0x20..0x2F
    b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ',
    // 0x30..0x3F
    b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ',
    // 0x40..0x4F: 0x40=' ', 0x4A='[', 0x4B='.', 0x4C='<', 0x4D='(', 0x4E='+', 0x4F='|'
    b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b'[', b'.', b'<', b'(', b'+', b'|',
    // 0x50..0x5F: 0x50='&', 0x5A='!', 0x5B='$', 0x5C='*', 0x5D=')', 0x5E=';', 0x5F='^'
    b'&', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b'!', b'$', b'*', b')', b';', b'^',
    // 0x60..0x6F: 0x60='-', 0x61='/', 0x6A='|', 0x6B=',', 0x6C='%', 0x6D='_', 0x6E='>', 0x6F='?'
    b'-', b'/', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b'|', b',', b'%', b'_', b'>', b'?',
    // 0x70..0x7F: 0x79='`', 0x7A=':', 0x7B='#', 0x7C='@', 0x7D='\'', 0x7E='=', 0x7F='"'
    b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b'`', b':', b'#', b'@', b'\'', b'=', b'"',
    // 0x80..0x8F: 0x81..0x89 = 'a'..'i'
    b' ', b'a', b'b', b'c', b'd', b'e', b'f', b'g', b'h', b'i', b' ', b' ', b' ', b' ', b' ', b' ',
    // 0x90..0x9F: 0x91..0x99 = 'j'..'r'
    b' ', b'j', b'k', b'l', b'm', b'n', b'o', b'p', b'q', b'r', b' ', b' ', b' ', b' ', b' ', b' ',
    // 0xA0..0xAF: 0xA1='~', 0xA2..0xA9 = 's'..'z'
    b' ', b'~', b's', b't', b'u', b'v', b'w', b'x', b'y', b'z', b' ', b' ', b' ', b' ', b' ', b' ',
    // 0xB0..0xBF: 0xBA='[', 0xBB=']'
    b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b' ', b'[', b']', b' ', b' ', b' ', b' ',
    // 0xC0..0xCF: 0xC0='{', 0xC1..0xC9 = 'A'..'I'
    b'{', b'A', b'B', b'C', b'D', b'E', b'F', b'G', b'H', b'I', b' ', b' ', b' ', b' ', b' ', b' ',
    // 0xD0..0xDF: 0xD0='}', 0xD1..0xD9 = 'J'..'R'
    b'}', b'J', b'K', b'L', b'M', b'N', b'O', b'P', b'Q', b'R', b' ', b' ', b' ', b' ', b' ', b' ',
    // 0xE0..0xEF: 0xE0='\\', 0xE2..0xE9 = 'S'..'Z'
    b'\\', b' ', b'S', b'T', b'U', b'V', b'W', b'X', b'Y', b'Z', b' ', b' ', b' ', b' ', b' ', b' ',
    // 0xF0..0xFF: 0xF0..0xF9 = '0'..'9'
    b'0', b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b' ', b' ', b' ', b' ', b' ', b' ',
];

/// Converts a single CP037 EBCDIC byte to printable ASCII (`0x20..=0x7E`).
#[inline]
pub const fn ebcdic_to_ascii_byte(byte: u8) -> u8 {
    CP037_TO_ASCII[byte as usize]
}

/// Converts a single printable ASCII byte to CP037 EBCDIC.
/// Non-mapped bytes become EBCDIC space (`0x40`).
pub const fn ascii_to_ebcdic_byte(byte: u8) -> u8 {
    match byte {
        b' ' => 0x40,
        b'.' => 0x4B,
        b'<' => 0x4C,
        b'(' => 0x4D,
        b'+' => 0x4E,
        b'|' => 0x4F,
        b'&' => 0x50,
        b'!' => 0x5A,
        b'$' => 0x5B,
        b'*' => 0x5C,
        b')' => 0x5D,
        b';' => 0x5E,
        b'^' => 0x5F,
        b'-' => 0x60,
        b'/' => 0x61,
        b',' => 0x6B,
        b'%' => 0x6C,
        b'_' => 0x6D,
        b'>' => 0x6E,
        b'?' => 0x6F,
        b'`' => 0x79,
        b':' => 0x7A,
        b'#' => 0x7B,
        b'@' => 0x7C,
        b'\'' => 0x7D,
        b'=' => 0x7E,
        b'"' => 0x7F,
        b'a'..=b'i' => 0x81 + (byte - b'a'),
        b'j'..=b'r' => 0x91 + (byte - b'j'),
        b'~' => 0xA1,
        b's'..=b'z' => 0xA2 + (byte - b's'),
        b'[' => 0xBA,
        b']' => 0xBB,
        b'{' => 0xC0,
        b'A'..=b'I' => 0xC1 + (byte - b'A'),
        b'}' => 0xD0,
        b'J'..=b'R' => 0xD1 + (byte - b'J'),
        b'\\' => 0xE0,
        b'S'..=b'Z' => 0xE2 + (byte - b'S'),
        b'0'..=b'9' => 0xF0 + (byte - b'0'),
        _ => 0x40,
    }
}

/// Converts an EBCDIC byte slice into an ASCII `String`.
pub fn ebcdic_to_ascii(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        out.push(char::from(ebcdic_to_ascii_byte(b)));
    }
    out
}

/// Sanitizes an ASCII byte slice into printable ASCII (`0x20..=0x7E`).
pub fn sanitize_ascii(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        let ch = if (0x20..=0x7E).contains(&b) { b } else { b' ' };
        out.push(char::from(ch));
    }
    out
}

/// Character encoding detected for the 3200-byte textual file header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    /// Standard EBCDIC (CP037) encoding.
    Ebcdic,
    /// ASCII encoding (common in SEG-Y rev 1+ files).
    Ascii,
}

/// Heuristic check for whether a textual header byte slice is encoded in EBCDIC.
pub fn detect_text_encoding(bytes: &[u8]) -> TextEncoding {
    let mut ebcdic_score: usize = 0;
    let mut ascii_score: usize = 0;
    for &b in bytes {
        if b == 0x40 || (0xC1..=0xF9).contains(&b) {
            ebcdic_score = ebcdic_score.saturating_add(1);
        }
        if (0x20..=0x7E).contains(&b) && b != 0x40 {
            ascii_score = ascii_score.saturating_add(1);
        }
    }
    if ebcdic_score >= ascii_score {
        TextEncoding::Ebcdic
    } else {
        TextEncoding::Ascii
    }
}

/// Parsed 3200-byte SEG-Y textual file header (40 lines × 80 characters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextualHeader {
    encoding: TextEncoding,
    lines: Vec<String>,
}

impl TextualHeader {
    /// Parses a 3200-byte textual header, auto-detecting EBCDIC vs ASCII.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < TEXTUAL_HEADER_LEN {
            return Err(SegyError::FileTooSmall {
                actual_bytes: bytes.len(),
                min_bytes: TEXTUAL_HEADER_LEN,
            });
        }
        let header_bytes = &bytes[..TEXTUAL_HEADER_LEN];
        let encoding = detect_text_encoding(header_bytes);
        Self::parse_with_encoding(header_bytes, encoding)
    }

    /// Parses a 3200-byte textual header using an explicit character encoding.
    pub fn parse_with_encoding(bytes: &[u8], encoding: TextEncoding) -> Result<Self> {
        if bytes.len() < TEXTUAL_HEADER_LEN {
            return Err(SegyError::FileTooSmall {
                actual_bytes: bytes.len(),
                min_bytes: TEXTUAL_HEADER_LEN,
            });
        }
        let header_bytes = &bytes[..TEXTUAL_HEADER_LEN];
        let mut lines = Vec::with_capacity(TEXTUAL_HEADER_LINES);
        for chunk in header_bytes.as_chunks::<TEXTUAL_HEADER_COLS>().0 {
            let line = match encoding {
                TextEncoding::Ebcdic => ebcdic_to_ascii(chunk),
                TextEncoding::Ascii => sanitize_ascii(chunk),
            };
            lines.push(line);
        }
        Ok(Self { encoding, lines })
    }

    /// Returns the detected or specified text encoding.
    #[inline]
    pub fn encoding(&self) -> TextEncoding {
        self.encoding
    }

    /// Returns the 40 decoded 80-character card lines.
    #[inline]
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Formats the 40 lines as a newline-separated string with trailing spaces trimmed per line.
    pub fn to_trimmed_text(&self) -> String {
        let mut out = String::new();
        for (idx, line) in self.lines.iter().enumerate() {
            if idx > 0 {
                out.push('\n');
            }
            out.push_str(line.trim_end());
        }
        out
    }
}
