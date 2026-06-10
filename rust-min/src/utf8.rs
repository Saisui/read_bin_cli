/// UTF-8 byte classification per utf8.rb
#[derive(Debug, Clone, Copy)]
pub enum ByteClass {
    Ascii,
    Tail,
    Invalid,
    Duo,
    Trio,
    Quo,
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

pub enum Segment {
    Char { pos: usize, ch: char, len: usize },
    Invalid { pos: usize },
}

pub fn decode_row(data: &[u8], offset: usize, count: usize, start_pos: usize) -> Vec<Segment> {
    let total = data.len().saturating_sub(offset);
    let mut out = Vec::new();
    let mut i = start_pos;
    while i < count {
        let b = data[offset + i];
        let seq_len = match b {
            0x00..=0x7F => 1,
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            _ => {
                out.push(Segment::Invalid { pos: i });
                i += 1;
                continue;
            }
        };
        if i + seq_len > total {
            out.push(Segment::Invalid { pos: i });
            i += 1;
            continue;
        }
        let mut ok = true;
        for j in 1..seq_len {
            if !(0x80..=0xBF).contains(&data[offset + i + j]) {
                ok = false;
                break;
            }
        }
        if !ok {
            out.push(Segment::Invalid { pos: i });
            i += 1;
            continue;
        }
        if seq_len == 2 && b < 0xC2 {
            out.push(Segment::Invalid { pos: i });
            i += 1;
            continue;
        }
        if seq_len == 3 {
            let c1 = data[offset + i + 1];
            if (b == 0xE0 && c1 < 0xA0) || (b == 0xED && c1 > 0x9F) {
                out.push(Segment::Invalid { pos: i });
                i += 1;
                continue;
            }
        }
        if seq_len == 4 {
            let c1 = data[offset + i + 1];
            if (b == 0xF0 && c1 < 0x90) || (b == 0xF4 && c1 > 0x8F) {
                out.push(Segment::Invalid { pos: i });
                i += 1;
                continue;
            }
        }
        match std::str::from_utf8(&data[offset + i..offset + i + seq_len]) {
            Ok(s) => {
                let ch = s.chars().next().unwrap();
                out.push(Segment::Char { pos: i, ch, len: seq_len });
            }
            Err(_) => out.push(Segment::Invalid { pos: i }),
        }
        i += seq_len;
    }
    out
}

pub fn display_width(ch: char) -> usize {
    let cp = ch as u32;
    if (0x1100..=0x115F).contains(&cp) || cp == 0x2329 || cp == 0x232A
        || (0x2E80..=0x33FF).contains(&cp) || (0x3400..=0x4DBF).contains(&cp)
        || (0x4E00..=0x9FFF).contains(&cp) || (0xA000..=0xA4CF).contains(&cp)
        || (0xAC00..=0xD7AF).contains(&cp) || (0xF900..=0xFAFF).contains(&cp)
        || (0xFE30..=0xFE6F).contains(&cp) || (0xFF00..=0xFF60).contains(&cp)
        || (0xFFE0..=0xFFE6).contains(&cp) || (0x1F000..=0x1F9FF).contains(&cp)
        || (0x20000..=0x2FA1F).contains(&cp)
    { 2 } else { 1 }
}
