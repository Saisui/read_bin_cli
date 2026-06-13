/// UTF-8 byte classification per utf8.rb categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteClass {
    Ascii,   // 0x00-0x7F
    Tail,    // 0x80-0xBF (continuation)
    Invalid, // 0xC0-0xC1, 0xF8-0xFF
    Duo,     // 0xC2-0xDF (2-byte head)
    Trio,    // 0xE0-0xEF (3-byte head)
    Quo,     // 0xF0-0xF7 (4-byte head)
}

pub fn byte_class(b: u8) -> ByteClass {
    match b {
        0x00..=0x7F => ByteClass::Ascii,
        0x80..=0xBF => ByteClass::Tail,
        0xC0 | 0xC1 => ByteClass::Invalid,
        0xC2..=0xDF => ByteClass::Duo,
        0xE0..=0xEF => ByteClass::Trio,
        0xF0..=0xF7 => ByteClass::Quo,
        _ => ByteClass::Invalid,
    }
}

/// A decoded segment in a 16-byte row.
#[derive(Debug, Clone)]
pub enum Utf8Segment {
    /// Valid character: (row_pos, char, byte_len)
    Char { pos: usize, ch: char, len: usize },
    /// Invalid byte: (row_pos)
    Invalid { pos: usize },
}

/// Pre-scan a row of bytes for UTF-8 sequences.
pub fn decode_row(data: &[u8], offset: usize, count: usize) -> Vec<Utf8Segment> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < count {
        let b = data[offset + i];
        let seq_len = match b {
            0x00..=0x7F => 1,
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            _ => {
                result.push(Utf8Segment::Invalid { pos: i });
                i += 1;
                continue;
            }
        };

        if i + seq_len > count {
            result.push(Utf8Segment::Invalid { pos: i });
            i += 1;
            continue;
        }

        // Validate continuation bytes
        let mut valid = true;
        for j in 1..seq_len {
            let cb = data[offset + i + j];
            if !(0x80..=0xBF).contains(&cb) {
                valid = false;
                break;
            }
        }
        if !valid {
            result.push(Utf8Segment::Invalid { pos: i });
            i += 1;
            continue;
        }

        // Additional range checks
        if seq_len == 2 && b < 0xC2 {
            result.push(Utf8Segment::Invalid { pos: i });
            i += 1;
            continue;
        }
        if seq_len == 3 {
            let cb1 = data[offset + i + 1];
            if b == 0xE0 && cb1 < 0xA0 {
                result.push(Utf8Segment::Invalid { pos: i });
                i += 1;
                continue;
            }
            if b == 0xED && cb1 > 0x9F {
                result.push(Utf8Segment::Invalid { pos: i });
                i += 1;
                continue;
            }
        }
        if seq_len == 4 {
            let cb1 = data[offset + i + 1];
            if b == 0xF0 && cb1 < 0x90 {
                result.push(Utf8Segment::Invalid { pos: i });
                i += 1;
                continue;
            }
            if b == 0xF4 && cb1 > 0x8F {
                result.push(Utf8Segment::Invalid { pos: i });
                i += 1;
                continue;
            }
        }

        // Decode
        let raw = &data[offset + i..offset + i + seq_len];
        match std::str::from_utf8(raw) {
            Ok(s) => {
                let ch = s.chars().next().unwrap();
                // Reject control chars except \n \r \t
                if (ch as u32) < 0x20 && ch != '\n' && ch != '\r' && ch != '\t' {
                    result.push(Utf8Segment::Invalid { pos: i });
                    i += 1;
                    continue;
                }
                result.push(Utf8Segment::Char { pos: i, ch, len: seq_len });
            }
            Err(_) => {
                result.push(Utf8Segment::Invalid { pos: i });
                i += 1;
                continue;
            }
        }
        i += seq_len;
    }
    result
}

/// Display width of a character (CJK = 2, others = 1).
pub fn display_width(ch: char) -> usize {
    let cp = ch as u32;
    if (0x1100..=0x115F).contains(&cp)
        || cp == 0x2329
        || cp == 0x232A
        || (0x2E80..=0x33FF).contains(&cp)
        || (0x3400..=0x4DBF).contains(&cp)
        || (0x4E00..=0x9FFF).contains(&cp)
        || (0xA000..=0xA4CF).contains(&cp)
        || (0xAC00..=0xD7AF).contains(&cp)
        || (0xF900..=0xFAFF).contains(&cp)
        || (0xFE30..=0xFE6F).contains(&cp)
        || (0xFF00..=0xFF60).contains(&cp)
        || (0xFFE0..=0xFFE6).contains(&cp)
        || (0x1F000..=0x1F9FF).contains(&cp)
        || (0x20000..=0x2FA1F).contains(&cp)
    {
        2
    } else {
        1
    }
}
